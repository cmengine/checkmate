//! The symbol table and name-resolution layer for the language server.
//!
//! The compiler front end ships a recovered AST whose spans cover whole
//! statements, but tooling needs *name* positions (the `hp` in
//! `int hp = 100`, the `i` of a `for` binding). This module derives them
//! from the token stream: it lexes the file once, walks the AST, and pins
//! every declaration name to its exact tokens. On top of those symbols,
//! [`resolve`] (see `resolve.rs`) answers "what is under the cursor".
//!
//! Resolution is deliberately best-effort and conservative: when a shape is
//! ambiguous or the parser recovered past a broken region, the analysis
//! answers "unknown" instead of guessing. It must never disagree with the
//! checker — it only surfaces what the checker accepts.

use cme_compiler::lexer::{Token, lex_with_errors};
use cme_core::Span;
use cme_core::ast::{Block, Expr, Stmt, StmtKind, Type};

use crate::resolve::infer_expr_type;

/// A stripped-down owned copy of the lexed token. Only the shapes the
/// symbol walk and the resolver care about are kept distinct; literals and
/// operators collapse into `Other`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    Newline,
    StrLit,
    Dot,
    Comma,
    Assign,
    Colon,
    Question,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Lt,
    Gt,
    KwInt,
    KwFloat,
    KwBool,
    KwStr,
    KwVoid,
    KwMap,
    KwIn,
    KwStruct,
    KwEnum,
    Other,
}

#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub kind: Tok,
    pub span: Span,
}

/// A parameter, local variable, `for` binding, or `match` binding.
#[derive(Debug, Clone)]
pub struct LocalSymbol {
    pub name: String,
    /// Declared type. `infer` locals crystallize best-effort at build time
    /// (§2.16); unresolved ones keep `Type::Infer`.
    pub ty: Type,
    /// The name token's exact span.
    pub span: Span,
    pub kind: LocalKind,
    /// Index into [`Analysis::functions`] of the enclosing function.
    pub function: usize,
    /// The block the declaration lives in. The checker scopes locals to
    /// their enclosing block (`check_block` pushes one scope per block), so
    /// resolution and completion must not surface a local outside it.
    pub scope: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    Param,
    Variable,
    ForBinding,
    MatchBinding,
}

impl LocalSymbol {
    /// True when `offset` is inside this local's block scope (§2.10): the
    /// checker declares locals per block, so a local of a finished `if`
    /// body is invisible after it.
    pub fn contains(&self, offset: usize) -> bool {
        self.scope.start <= offset && offset <= self.scope.end
    }
}

#[derive(Debug, Clone)]
pub struct FunctionSymbol {
    pub name: String,
    pub name_span: Span,
    /// The whole declaration, including the body.
    pub span: Span,
    /// The body block, the region where the function's locals are in scope.
    pub body_span: Span,
    pub params: Vec<LocalSymbol>,
    pub return_ty: Type,
    /// `Some(impl index)` for §10.4 impl members.
    pub impl_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct StructSymbol {
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    pub type_params: Vec<String>,
    /// `(name, type, name span)` per field.
    pub fields: Vec<(String, Type, Span)>,
}

#[derive(Debug, Clone)]
pub struct VariantSymbol {
    pub name: String,
    pub name_span: Span,
    /// `(name, type)` per payload field (§2.7).
    pub fields: Vec<(String, Type)>,
}

#[derive(Debug, Clone)]
pub struct EnumSymbol {
    pub name: String,
    pub name_span: Span,
    pub span: Span,
    pub type_params: Vec<String>,
    pub variants: Vec<VariantSymbol>,
}

#[derive(Debug, Clone)]
pub struct ImplSymbol {
    /// Dotted target path with each segment's span (§10.4).
    pub target: Vec<(String, Span)>,
    pub span: Span,
    /// Indices into [`Analysis::functions`] of the unioned members.
    pub member_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct ImportSymbol {
    /// Dot-separated path segments with their spans (§2.3).
    pub segments: Vec<(String, Span)>,
    pub span: Span,
}

/// The symbol table of one document revision.
#[derive(Debug, Default)]
pub struct Analysis<'a> {
    pub statements: &'a [Stmt],
    pub tokens: Vec<TokenInfo>,
    pub functions: Vec<FunctionSymbol>,
    pub structs: Vec<StructSymbol>,
    pub enums: Vec<EnumSymbol>,
    pub impls: Vec<ImplSymbol>,
    pub imports: Vec<ImportSymbol>,
    /// All locals, in source order.
    pub locals: Vec<LocalSymbol>,
    /// `(local index, initializer)` for every `infer` declaration, recorded
    /// during the walk so crystallization can run after registration.
    infer_initializers: Vec<(usize, &'a Expr)>,
}

impl<'a> Analysis<'a> {
    /// Builds the symbol table for one document revision.
    pub fn build(source: &'a str, statements: &'a [Stmt]) -> Analysis<'a> {
        let (lexed, _) = lex_with_errors(source);
        let tokens: Vec<TokenInfo> = lexed
            .into_iter()
            .map(|token| TokenInfo {
                kind: Tok::from(&token.token),
                span: token.span,
            })
            .collect();
        let mut analysis = Analysis {
            statements,
            tokens,
            ..Analysis::default()
        };

        // Top-level: only declarations are legal (§2), and the checker
        // rejects anything else, so the symbol walk mirrors that shape.
        for statement in statements {
            match &statement.kind {
                StmtKind::FuncDecl { .. } => {
                    analysis.add_function(statement, None);
                }
                StmtKind::StructDecl { .. } => analysis.add_struct(statement),
                StmtKind::EnumDecl { .. } => analysis.add_enum(statement),
                StmtKind::ImplDecl { .. } => analysis.add_impl(statement),
                StmtKind::Import { path } => analysis.add_import(statement, path),
                _ => {}
            }
        }

        // Second pass: crystallize `infer` locals now that every
        // declaration is registered (§2.16, best effort).
        analysis.resolve_infer_locals();
        analysis
    }

    /// The index range of tokens covering `span`.
    fn token_range(&self, span: Span) -> (usize, usize) {
        let start = self
            .tokens
            .partition_point(|token| token.span.end <= span.start);
        let end = self
            .tokens
            .partition_point(|token| token.span.start < span.end);
        (start, end)
    }

    fn token(&self, index: usize) -> Option<&TokenInfo> {
        self.tokens.get(index)
    }

    /// The Ident token immediately before the first occurrence of `pred`
    /// inside `span` (used for function names: `ret name(`).
    fn ident_before<F>(&self, span: Span, mut pred: F) -> Option<Span>
    where
        F: FnMut(&Tok) -> bool,
    {
        let (start, end) = self.token_range(span);
        for index in start..end {
            if pred(&self.tokens[index].kind) {
                return match index.checked_sub(1).map(|prev| self.token(prev)) {
                    Some(Some(TokenInfo {
                        kind: Tok::Ident(_),
                        span,
                    })) => Some(*span),
                    _ => None,
                };
            }
        }
        None
    }

    /// The Ident token immediately after the first occurrence of `pred`
    /// inside `span` (used for `struct Name`, `enum Name`).
    fn ident_after<F>(&self, span: Span, mut pred: F) -> Option<Span>
    where
        F: FnMut(&Tok) -> bool,
    {
        let (start, end) = self.token_range(span);
        for index in start..end {
            if pred(&self.tokens[index].kind) {
                return match self.token(index + 1) {
                    Some(TokenInfo {
                        kind: Tok::Ident(_),
                        span,
                    }) => Some(*span),
                    _ => None,
                };
            }
        }
        None
    }

    /// The name span of a variable declaration: the last identifier before
    /// the `=` (`vec2 pos = ...` — the type name is a keyword or an earlier
    /// ident, the binding is the one adjacent to the assignment).
    fn var_decl_name(&self, span: Span) -> Option<Span> {
        let (start, end) = self.token_range(span);
        let mut last_ident = None;
        for index in start..end {
            match &self.tokens[index].kind {
                Tok::Assign => return last_ident,
                Tok::Ident(_) => last_ident = Some(self.tokens[index].span),
                _ => {}
            }
        }
        None
    }

    /// The name span of a `for (ty elem in xs)` binding: the last
    /// identifier before the `in` keyword.
    fn for_binding_name(&self, span: Span) -> Option<Span> {
        self.ident_before(span, |kind| matches!(kind, Tok::KwIn))
    }

    /// Registers a function declaration (top level or impl member) and
    /// walks its body for locals.
    fn add_function(&mut self, statement: &'a Stmt, impl_index: Option<usize>) {
        let StmtKind::FuncDecl {
            name,
            params,
            return_ty,
            body,
        } = &statement.kind
        else {
            return;
        };
        let name_span = self
            .ident_before(statement.span, |kind| matches!(kind, Tok::LParen))
            .unwrap_or(statement.span);
        let function_index = self.functions.len();
        let mut symbol = FunctionSymbol {
            name: name.clone(),
            name_span,
            span: statement.span,
            body_span: body.span,
            params: Vec::new(),
            return_ty: return_ty.clone(),
            impl_index,
        };

        // Parameters live between the declaration's first `(` and its
        // matching `)`: `ty name` pairs separated by commas (§2.11).
        let (start, end) = self.token_range(statement.span);
        if let Some(paren) = self.tokens[start..end]
            .iter()
            .position(|token| token.kind == Tok::LParen)
        {
            let params_span = Span::new(self.tokens[start + paren].span.end, statement.span.end);
            let names = self.param_names(params_span);
            for (param, name_span) in params.iter().zip(names) {
                symbol.params.push(LocalSymbol {
                    name: param.name.clone(),
                    ty: param.ty.clone(),
                    span: name_span,
                    kind: LocalKind::Param,
                    function: function_index,
                    scope: body.span,
                });
            }
        }
        self.functions.push(symbol);
        self.locals
            .extend(self.functions[function_index].params.clone());

        // Walk the body for locals.
        self.collect_locals(body, function_index);
    }

    /// Extracts parameter name spans: after each type in the comma list,
    /// the next Ident is the name (§2.11).
    fn param_names(&self, span: Span) -> Vec<Span> {
        let mut names = Vec::new();
        let (mut index, end) = self.token_range(span);
        while index < end {
            if !self.skip_type(index, end, &mut index) {
                break;
            }
            match self.token(index) {
                Some(TokenInfo {
                    kind: Tok::Ident(_),
                    span,
                }) => {
                    names.push(*span);
                    index += 1;
                }
                _ => break,
            }
            // Skip the comma between parameters.
            while matches!(
                self.token(index).map(|t| &t.kind),
                Some(Tok::Comma) | Some(Tok::Newline)
            ) {
                index += 1;
            }
        }
        names
    }

    /// Skips one type expression starting at `index`, advancing `index`.
    /// Handles `int`, named types with generics (`pair<int, str>`), arrays
    /// (`int[]`), and maps (`map<str, int>`), including nesting.
    fn skip_type(&self, index: usize, end: usize, advance: &mut usize) -> bool {
        let Some(first) = self.token(index) else {
            return false;
        };
        *advance = index + 1;
        match &first.kind {
            Tok::KwInt | Tok::KwFloat | Tok::KwBool | Tok::KwStr | Tok::KwVoid => {}
            Tok::KwMap => {
                // map<K, V> — the comma inside generics must not end the
                // parameter.
                if !matches!(self.token(*advance).map(|t| &t.kind), Some(Tok::Lt)) {
                    return true;
                }
                *advance += 1;
                if !self.skip_type(*advance, end, advance) {
                    return false;
                }
                if matches!(self.token(*advance).map(|t| &t.kind), Some(Tok::Comma)) {
                    *advance += 1;
                    if !self.skip_type(*advance, end, advance) {
                        return false;
                    }
                }
                if matches!(self.token(*advance).map(|t| &t.kind), Some(Tok::Gt)) {
                    *advance += 1;
                }
            }
            Tok::Ident(_) => {
                // Optional generic arguments: name<T, U>.
                if matches!(self.token(*advance).map(|t| &t.kind), Some(Tok::Lt)) {
                    let mut depth = 0;
                    while *advance < end {
                        match self.token(*advance).map(|t| &t.kind) {
                            Some(Tok::Lt) => {
                                depth += 1;
                                *advance += 1;
                            }
                            Some(Tok::Gt) => {
                                depth -= 1;
                                *advance += 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            Some(_) => *advance += 1,
                            None => break,
                        }
                    }
                }
            }
            _ => return false,
        }
        // Array suffixes: T[] (repeated for T[][]).
        while matches!(self.token(*advance).map(|t| &t.kind), Some(Tok::LBracket)) {
            *advance += 2;
        }
        true
    }

    /// Registers a struct declaration and its fields (§2.6).
    fn add_struct(&mut self, statement: &'a Stmt) {
        let StmtKind::StructDecl {
            name,
            type_params,
            fields,
        } = &statement.kind
        else {
            return;
        };
        let name_span = self
            .ident_after(statement.span, |kind| matches!(kind, Tok::KwStruct))
            .unwrap_or(statement.span);
        let field_spans = self.member_name_spans(statement.span);
        let mut symbol = StructSymbol {
            name: name.clone(),
            name_span,
            span: statement.span,
            type_params: type_params.clone(),
            fields: Vec::new(),
        };
        for (field, name_span) in fields.iter().zip(field_spans) {
            symbol
                .fields
                .push((field.name.clone(), field.ty.clone(), name_span));
        }
        self.structs.push(symbol);
    }

    /// Registers an enum declaration and its variants (§2.7).
    fn add_enum(&mut self, statement: &'a Stmt) {
        let StmtKind::EnumDecl {
            name,
            type_params,
            variants,
        } = &statement.kind
        else {
            return;
        };
        let name_span = self
            .ident_after(statement.span, |kind| matches!(kind, Tok::KwEnum))
            .unwrap_or(statement.span);
        let mut symbol = EnumSymbol {
            name: name.clone(),
            name_span,
            span: statement.span,
            type_params: type_params.clone(),
            variants: Vec::new(),
        };

        // Variants sit between the braces: `Name` or `Name(payload...)`
        // (§2.7). The variant NAME spans come from the token stream; the
        // payloads are aligned with the AST declarations below (the source
        // of truth for spelling and field types). Payload identifiers are
        // NOT variant names: every Ident inside a parenthesized payload
        // (`Damage(int amount)`) is skipped, so only top-level identifiers
        // of the enum body register.
        let (start, end) = self.token_range(statement.span);
        let mut brace = 0i32;
        let mut paren = 0i32;
        for index in start..end {
            match &self.tokens[index].kind {
                Tok::LBrace => brace += 1,
                Tok::RBrace => brace -= 1,
                Tok::LParen => paren += 1,
                Tok::RParen => paren -= 1,
                Tok::Ident(variant) if brace > 0 && paren == 0 => {
                    symbol.variants.push(VariantSymbol {
                        name: variant.clone(),
                        name_span: self.tokens[index].span,
                        fields: Vec::new(),
                    });
                }
                _ => {}
            }
        }
        // Align payloads with the AST's variant declarations.
        for variant in &mut symbol.variants {
            if let Some(decl) = variants.iter().find(|decl| decl.name == variant.name) {
                variant.fields = decl
                    .fields
                    .iter()
                    .map(|field| (field.name.clone(), field.ty.clone()))
                    .collect();
            }
        }
        self.enums.push(symbol);
    }

    /// Field name spans between the braces of a declaration: repeated
    /// `Type name` pairs (§2.6 struct fields).
    fn member_name_spans(&self, span: Span) -> Vec<Span> {
        let mut names = Vec::new();
        let (mut index, end) = self.token_range(span);
        // Enter the braces.
        while index < end && !matches!(self.token(index).map(|t| &t.kind), Some(Tok::LBrace)) {
            index += 1;
        }
        index += 1;
        while index < end {
            match self.token(index).map(|t| &t.kind) {
                Some(Tok::RBrace) | None => break,
                Some(Tok::Newline) | Some(Tok::Comma) => index += 1,
                Some(_) => {
                    let mut advance = index;
                    if !self.skip_type(index, end, &mut advance) {
                        break;
                    }
                    match self.token(advance) {
                        Some(TokenInfo {
                            kind: Tok::Ident(_),
                            span,
                        }) => {
                            names.push(*span);
                            index = advance + 1;
                        }
                        _ => break,
                    }
                }
            }
        }
        names
    }

    /// Registers an impl block (§10.4): the dotted target plus its member
    /// functions.
    fn add_impl(&mut self, statement: &'a Stmt) {
        let StmtKind::ImplDecl { target, members } = &statement.kind else {
            return;
        };
        let impl_index = self.impls.len();
        let mut symbol = ImplSymbol {
            target: Vec::new(),
            span: statement.span,
            member_indices: Vec::new(),
        };

        // Target segments: idents and dots between `impl` and `{`.
        let (start, end) = self.token_range(statement.span);
        let mut brace = 0;
        for index in start..end {
            match &self.tokens[index].kind {
                Tok::LBrace => {
                    brace += 1;
                    break;
                }
                Tok::Ident(segment) => symbol
                    .target
                    .push((segment.clone(), self.tokens[index].span)),
                _ => {}
            }
        }
        let _ = brace;
        // Align target names with the AST (source of truth for spelling).
        symbol.target = target
            .iter()
            .cloned()
            .zip(symbol.target.iter().map(|(_, span)| *span))
            .collect();

        self.impls.push(symbol);
        for member in members {
            if matches!(member.kind, StmtKind::FuncDecl { .. }) {
                let member_index = self.functions.len();
                self.add_function(member, Some(impl_index));
                self.impls[impl_index].member_indices.push(member_index);
            }
        }
    }

    fn add_import(&mut self, statement: &'a Stmt, path: &[String]) {
        // Segments: idents between `import` and the newline.
        let (start, end) = self.token_range(statement.span);
        let mut segments = Vec::new();
        for index in start..end {
            if let Tok::Ident(segment) = &self.tokens[index].kind {
                segments.push((segment.clone(), self.tokens[index].span));
            }
        }
        // Trust the AST for spelling; the tokens only contributed spans.
        let segments = path
            .iter()
            .cloned()
            .zip(segments.into_iter().map(|(_, span)| span))
            .collect();
        self.imports.push(ImportSymbol {
            segments,
            span: statement.span,
        });
    }

    /// Collects locals in a statement list, recursing into every nested
    /// block (if/else, while, for, match arms). A declaration's scope is
    /// the block that directly contains it (§2.10 block scoping).
    fn collect_locals(&mut self, block: &'a Block, function: usize) {
        let scope = block.span;
        for statement in &block.stmts {
            match &statement.kind {
                StmtKind::VarDecl { name, ty, expr } => {
                    let span = self.var_decl_name(statement.span).unwrap_or(statement.span);
                    let is_infer = *ty == Type::Infer;
                    self.locals.push(LocalSymbol {
                        name: name.clone(),
                        ty: ty.clone(),
                        span,
                        kind: LocalKind::Variable,
                        function,
                        scope,
                    });
                    if is_infer {
                        self.infer_initializers.push((self.locals.len() - 1, expr));
                    }
                }
                StmtKind::For {
                    elem_name,
                    elem_ty,
                    body,
                    ..
                } => {
                    let span = self
                        .for_binding_name(statement.span)
                        .unwrap_or(statement.span);
                    self.locals.push(LocalSymbol {
                        name: elem_name.clone(),
                        ty: elem_ty.clone(),
                        span,
                        kind: LocalKind::ForBinding,
                        function,
                        scope: body.span,
                    });
                    self.collect_locals(body, function);
                }
                StmtKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.collect_locals(then_branch, function);
                    if let Some(else_branch) = else_branch {
                        // Else chains reuse the If node; a plain block is
                        // the only other shape.
                        if let StmtKind::Block(block) = &else_branch.kind {
                            self.collect_locals(block, function);
                        } else if let StmtKind::If { .. } = &else_branch.kind {
                            self.collect_if_chain(else_branch, function);
                        }
                    }
                }
                StmtKind::While { body, .. } => self.collect_locals(body, function),
                StmtKind::Match { arms, .. } => {
                    self.collect_match_bindings(statement, arms, function);
                    for arm in arms {
                        self.collect_locals(&arm.body, function);
                    }
                }
                StmtKind::Block(block) => self.collect_locals(block, function),
                _ => {}
            }
        }
    }

    /// `else if` chains reusing If nodes.
    fn collect_if_chain(&mut self, statement: &'a Stmt, function: usize) {
        if let StmtKind::If {
            then_branch,
            else_branch,
            ..
        } = &statement.kind
        {
            self.collect_locals(then_branch, function);
            if let Some(else_branch) = else_branch {
                match &else_branch.kind {
                    StmtKind::Block(block) => self.collect_locals(block, function),
                    StmtKind::If { .. } => self.collect_if_chain(else_branch, function),
                    _ => {}
                }
            }
        }
    }

    /// Match arm payload bindings (`Damage(int amount) => ...`) are
    /// declarations in scope of the arm (§2.15). The AST does not carry
    /// pattern spans, so the region between the previous arm's body end
    /// (or the match start) and this arm's body start is searched for the
    /// binding name.
    fn collect_match_bindings(
        &mut self,
        statement: &'a Stmt,
        arms: &[cme_core::ast::MatchArmStmt],
        function: usize,
    ) {
        let mut region_start = statement.span.start;
        for arm in arms {
            let region = Span::new(region_start, arm.body.span.start);
            region_start = arm.body.span.end;
            let cme_core::ast::Pattern::Variant { bindings, .. } = &arm.pattern else {
                continue;
            };
            for binding in bindings {
                if let Some(span) = self.ident_named_in(binding.name.as_str(), region) {
                    self.locals.push(LocalSymbol {
                        name: binding.name.clone(),
                        ty: binding.ty.clone(),
                        span,
                        kind: LocalKind::MatchBinding,
                        function,
                        scope: arm.body.span,
                    });
                }
            }
        }
    }

    /// The first Ident token spelling `name` inside `span`.
    fn ident_named_in(&self, name: &str, span: Span) -> Option<Span> {
        let (start, end) = self.token_range(span);
        for index in start..end {
            if let Tok::Ident(ident) = &self.tokens[index].kind
                && ident == name
            {
                return Some(self.tokens[index].span);
            }
        }
        None
    }

    /// Crystallizes `infer` locals after the table is complete (§2.16).
    /// The initializer expression is evaluated against everything
    /// registered so far; an ambiguous initializer keeps `Type::Infer`,
    /// exactly like the checker's behavior.
    fn resolve_infer_locals(&mut self) {
        let initializers = std::mem::take(&mut self.infer_initializers);
        let mut inferred: Vec<(usize, Type)> = Vec::new();
        for (index, expr) in &initializers {
            if let Some(ty) = infer_expr_type(expr, &self.structs, &self.functions, &self.locals) {
                inferred.push((*index, ty));
            }
        }
        for (index, ty) in inferred {
            self.locals[index].ty = ty;
        }
    }
}

impl Tok {
    fn from(token: &Token<'_>) -> Tok {
        match token {
            Token::Ident(name) => Tok::Ident(name.to_string()),
            Token::Newline => Tok::Newline,
            Token::StrLit(_) | Token::InterpStrLit(_) => Tok::StrLit,
            Token::Dot => Tok::Dot,
            Token::Comma => Tok::Comma,
            Token::Assign => Tok::Assign,
            Token::Colon => Tok::Colon,
            Token::Question => Tok::Question,
            Token::LParen => Tok::LParen,
            Token::RParen => Tok::RParen,
            Token::LBrace => Tok::LBrace,
            Token::RBrace => Tok::RBrace,
            Token::LBracket => Tok::LBracket,
            Token::RBracket => Tok::RBracket,
            Token::Lt => Tok::Lt,
            Token::Gt => Tok::Gt,
            Token::KwInt => Tok::KwInt,
            Token::KwFloat => Tok::KwFloat,
            Token::KwBool => Tok::KwBool,
            Token::KwStr => Tok::KwStr,
            Token::KwVoid => Tok::KwVoid,
            Token::KwIn => Tok::KwIn,
            Token::KwStruct => Tok::KwStruct,
            Token::KwEnum => Tok::KwEnum,
            _ => Tok::Other,
        }
    }
}

/// Renders a declared type back to Checkmate source form (§2.4, §2.6–§2.9,
/// §11).
pub fn render_type(ty: &Type) -> String {
    match ty {
        Type::Infer => "infer".to_string(),
        Type::Prim(cme_core::ast::PrimitiveType::Int) => "int".to_string(),
        Type::Prim(cme_core::ast::PrimitiveType::Float) => "float".to_string(),
        Type::Prim(cme_core::ast::PrimitiveType::Bool) => "bool".to_string(),
        Type::Prim(cme_core::ast::PrimitiveType::Str) => "str".to_string(),
        Type::Void => "void".to_string(),
        Type::Named { name, args } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = args.iter().map(render_type).collect();
                format!("{name}<{}>", inner.join(", "))
            }
        }
        Type::Array(elem) => format!("{}[]", render_type(elem)),
        Type::Map { key, value } => {
            format!("map<{}, {}>", render_type(key), render_type(value))
        }
    }
}
