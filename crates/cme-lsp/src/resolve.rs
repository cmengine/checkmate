//! Cursor resolution ("what is under the cursor") and best-effort
//! expression typing, built on the [`Analysis`] symbol table.
//!
//! Two evaluators cooperate:
//!
//! - The **AST evaluator** types whole initializer expressions (crystallizing
//!   `infer` locals, §2.16) and any expression node the parser recovered.
//! - The **token evaluator** types the receiver in front of a `.` while the
//!   user is still typing — the AST is momentarily broken there (`player.`),
//!   so the receiver is reconstructed from tokens with bracket matching.
//!
//! Both share the same struct-field core with generic substitution, so they
//! never disagree.

use cme_core::Span;
use cme_core::ast::{BinaryOp, Expr, ExprKind, PrimitiveType, Stmt, StmtKind, Type};
use cme_core::schema::{SchemaContract, SchemaMember};

use crate::analysis::{
    Analysis, EnumSymbol, LocalKind, LocalSymbol, StructSymbol, Tok, render_type,
};

/// What the cursor points at.
#[derive(Debug)]
pub enum Resolved<'a> {
    /// A parameter, variable, `for` binding, or `match` binding.
    Local(&'a LocalSymbol),
    /// A function declaration. Impl members report their target path.
    Function {
        function: &'a crate::analysis::FunctionSymbol,
        impl_target: Option<&'a [(String, Span)]>,
    },
    Struct(&'a StructSymbol),
    Enum(&'a EnumSymbol),
    /// A variant of an enum (§2.7): qualified (`gameEvent.Damage(…)`) or
    /// bare in a `match` arm pattern (§2.15).
    Variant {
        enum_type: &'a EnumSymbol,
        variant: &'a crate::analysis::VariantSymbol,
    },
    /// A struct field accessed through a value (`position.x`). `resolved`
    /// carries the field's type after generic substitution when the
    /// receiver pins concrete type arguments (`pair<int, str>`'s `first`
    /// resolves to `int`).
    Field {
        struct_type: &'a StructSymbol,
        index: usize,
        substituted: Option<Type>,
    },
    /// One segment of an `import` path (§2.3).
    ImportSegment {
        import: &'a crate::analysis::ImportSymbol,
        segment: usize,
    },
    /// A §9.3 boundary type declared in a schema file: the shape lives in
    /// the contract, not this document, so go-to-definition has no local
    /// span for it.
    SchemaStruct(&'a StructSymbol),
    SchemaEnum(&'a EnumSymbol),
    /// A capability or interface contract reached as a host path
    /// (`game.window` — §9.1).
    SchemaContract {
        namespace: &'a str,
        contract: &'a SchemaContract,
    },
    /// A capability/interface member reached through a host path
    /// (`game.window.OpenWindow` — §9.1).
    SchemaMember {
        namespace: &'a str,
        contract: &'a SchemaContract,
        member: &'a SchemaMember,
    },
    /// A built-in type the language owns (§2.4, §2.8, §11).
    BuiltinType {
        name: String,
        description: &'static str,
    },
    /// A built-in constructor (`Ok`, `Err`, `Some`, `None` — §2.8).
    BuiltinConstructor {
        name: String,
        description: &'static str,
    },
    /// The array `.length` property (§11).
    ArrayLength,
}

/// The built-in constructors no user declaration may take (checker rule).
pub const BUILTIN_CONSTRUCTORS: [(&str, &str); 4] = [
    ("Ok", "constructor of `result<T, E>`: `Ok(T value)`"),
    ("Err", "constructor of `result<T, E>`: `Err(E error)`"),
    ("Some", "constructor of `option<T>`: `Some(T value)`"),
    ("None", "constructor of `option<T>`: `None()`"),
];

/// The built-in type names reachable as identifiers (`int` and friends are
/// keywords; `map` lexes as an identifier and resolves through this table).
const BUILTIN_TYPE_NAMES: [(&str, &str); 3] = [
    ("option", "`option<T>`: `Some(T value)` or `None()` (§2.8)"),
    (
        "result",
        "`result<T, E>`: `Ok(T value)` or `Err(E error)` (§2.8)",
    ),
    ("map", "`map<K, V>` keyed collection (§11)"),
];

impl<'a> Analysis<'a> {
    /// The index of the function whose body contains `offset`, if any.
    pub fn enclosing_function_index(&'a self, offset: usize) -> Option<usize> {
        self.functions.iter().position(|function| {
            function.body_span.start <= offset && offset <= function.body_span.end
        })
    }

    /// The function whose body contains `offset`, if any.
    pub fn enclosing_function(
        &'a self,
        offset: usize,
    ) -> Option<&'a crate::analysis::FunctionSymbol> {
        self.enclosing_function_index(offset)
            .map(|index| &self.functions[index])
    }

    /// The token containing `offset`. Strict containment wins; at a token
    /// boundary the preference is an identifier STARTING there (so the
    /// first character of `p.hp` hovers the field, and a cursor right
    /// after a word still resolves the word) over the token ending there.
    fn token_at(&self, offset: usize) -> Option<usize> {
        if let Some(index) = self
            .tokens
            .iter()
            .position(|token| token.span.start < offset && offset < token.span.end)
        {
            return Some(index);
        }
        let ends_here = self
            .tokens
            .iter()
            .position(|token| token.span.start < offset && offset <= token.span.end);
        let starts_here = self
            .tokens
            .iter()
            .position(|token| token.span.start == offset && offset < token.span.end);
        match (ends_here, starts_here) {
            (Some(ending), Some(starting)) => {
                let prefer_starting = matches!(self.tokens[starting].kind, Tok::Ident(_))
                    && !matches!(self.tokens[ending].kind, Tok::Ident(_));
                if prefer_starting {
                    Some(starting)
                } else {
                    Some(ending)
                }
            }
            (Some(ending), None) => Some(ending),
            (None, Some(starting)) => Some(starting),
            (None, None) => None,
        }
    }

    /// Resolves the identifier (or type keyword) under `offset`.
    pub fn resolve(&'a self, offset: usize) -> Option<Resolved<'a>> {
        let token_index = self.token_at(offset)?;
        let token = &self.tokens[token_index];

        // Built-in types surface through their keywords (`int`, `str`, ...).
        if let Some(builtin) = builtin_type_from_keyword(&token.kind) {
            return Some(builtin);
        }
        let Tok::Ident(name) = &token.kind else {
            return None;
        };

        // Import segments resolve within their statement (§2.3).
        if let Some((import, segment)) = self.import_segment_at(token.span) {
            return Some(Resolved::ImportSegment { import, segment });
        }

        // A declaration name resolves to its own declaration.
        if let Some(own) = self.own_declaration(name, token.span) {
            return Some(own);
        }

        // Member access: whatever sits before the preceding dot.
        if token_index > 0 && self.tokens[token_index - 1].kind == Tok::Dot {
            return self.resolve_member(name, token_index);
        }

        // Plain references: locals shadow top-level declarations.
        if let Some(local) = self.resolve_local(name, token.span.start) {
            return Some(Resolved::Local(local));
        }
        if let Some(function) = self
            .functions
            .iter()
            .find(|function| &function.name == name)
        {
            let impl_target = function
                .impl_index
                .and_then(|index| self.impls.get(index))
                .map(|imp| imp.target.as_slice());
            return Some(Resolved::Function {
                function,
                impl_target,
            });
        }
        if let Some(struct_type) = self.structs.iter().find(|s| &s.name == name) {
            return Some(Resolved::Struct(struct_type));
        }
        if let Some(enum_type) = self.enums.iter().find(|e| &e.name == name) {
            return Some(Resolved::Enum(enum_type));
        }
        // §9.3 boundary types the mod's schema declares (§2.5: they are
        // PascalCase, so they never collide with locals).
        if let Some(struct_type) = self.schema_struct(name) {
            return Some(Resolved::SchemaStruct(struct_type));
        }
        if let Some(enum_type) = self.schema_enum(name) {
            return Some(Resolved::SchemaEnum(enum_type));
        }
        // A bare variant name in a `match` arm pattern (§2.15).
        if let Some((enum_type, variant)) = self.find_variant(name) {
            return Some(Resolved::Variant { enum_type, variant });
        }
        if let Some((builtin_name, description)) = BUILTIN_CONSTRUCTORS
            .iter()
            .find(|(candidate, _)| candidate == name)
        {
            return Some(Resolved::BuiltinConstructor {
                name: (*builtin_name).to_string(),
                description,
            });
        }
        if let Some((builtin_name, description)) = BUILTIN_TYPE_NAMES
            .iter()
            .find(|(candidate, _)| candidate == name)
        {
            return Some(Resolved::BuiltinType {
                name: (*builtin_name).to_string(),
                description,
            });
        }
        None
    }

    fn import_segment_at(
        &'a self,
        span: Span,
    ) -> Option<(&'a crate::analysis::ImportSymbol, usize)> {
        for import in &self.imports {
            if import.span.start <= span.start && span.end <= import.span.end {
                let segment = import
                    .segments
                    .iter()
                    .position(|(_, segment_span)| *segment_span == span)?;
                return Some((import, segment));
            }
        }
        None
    }

    /// Is `span` the exact name span of a declaration?
    fn own_declaration(&'a self, name: &str, span: Span) -> Option<Resolved<'a>> {
        for function in &self.functions {
            if function.name_span == span && function.name == name {
                let impl_target = function
                    .impl_index
                    .and_then(|index| self.impls.get(index))
                    .map(|imp| imp.target.as_slice());
                return Some(Resolved::Function {
                    function,
                    impl_target,
                });
            }
            for param in &function.params {
                if param.span == span && param.name == name {
                    return Some(Resolved::Local(param));
                }
            }
        }
        for local in &self.locals {
            if local.span == span && local.name == name {
                return Some(Resolved::Local(local));
            }
        }
        for struct_type in &self.structs {
            if struct_type.name_span == span && struct_type.name == name {
                return Some(Resolved::Struct(struct_type));
            }
            for (index, (field_name, _, field_span)) in struct_type.fields.iter().enumerate() {
                if *field_span == span && field_name == name {
                    // The declaration site has no concrete receiver, so
                    // the declared (unsubstituted) type is the truth.
                    return Some(Resolved::Field {
                        struct_type,
                        index,
                        substituted: None,
                    });
                }
            }
        }
        for enum_type in &self.enums {
            if enum_type.name_span == span && enum_type.name == name {
                return Some(Resolved::Enum(enum_type));
            }
            for variant in &enum_type.variants {
                if variant.name_span == span && variant.name == name {
                    return Some(Resolved::Variant { enum_type, variant });
                }
            }
        }
        None
    }

    /// Resolves `name` right after a dot: a struct field, an enum variant,
    /// the array `.length` property (§11), or a §9 capability member
    /// reached through a host path (`game.window.OpenWindow`).
    fn resolve_member(&'a self, name: &str, dot_token_index: usize) -> Option<Resolved<'a>> {
        // The dot is at dot_token_index - 1; resolve the receiver in front.
        let receiver = self.receiver_token_range(dot_token_index - 1)?;
        let ty = self.type_of_token_range(
            receiver,
            self.tokens
                .get(dot_token_index)
                .map(|token| token.span.start)
                .unwrap_or(usize::MAX),
        );
        match ty.as_ref() {
            Some(Type::Named {
                name: type_name,
                args,
            }) => {
                // A struct receiver that is not a field, or an enum
                // receiver that is not a variant, falls through to the
                // impl-member check below.
                if let Some(struct_type) = self.structs.iter().find(|s| &s.name == type_name)
                    && let Some(index) = struct_type
                        .fields
                        .iter()
                        .position(|(field_name, _, _)| field_name == name)
                {
                    let substituted =
                        substitute(&struct_type.fields[index].1, &struct_type.type_params, args);
                    return Some(Resolved::Field {
                        struct_type,
                        index,
                        substituted: (substituted != struct_type.fields[index].1)
                            .then_some(substituted),
                    });
                }
                if let Some(enum_type) = self.enums.iter().find(|e| &e.name == type_name)
                    && let Some(variant) = enum_type.variants.iter().find(|v| v.name == name)
                {
                    return Some(Resolved::Variant { enum_type, variant });
                }
                // An impl member call (`vec2.dot(v)` — §10.4): the member
                // names a function of an impl block whose target's last
                // segment is this type.
                if let Some(function) = self.functions.iter().find(|function| {
                    function.name == *name
                        && function.impl_index.is_some()
                        && function
                            .impl_index
                            .and_then(|index| self.impls.get(index))
                            .and_then(|impl_block| impl_block.target.last())
                            .map(|(segment, _)| segment == type_name)
                            .unwrap_or(false)
                }) {
                    let impl_target = function
                        .impl_index
                        .and_then(|index| self.impls.get(index))
                        .map(|impl_block| impl_block.target.as_slice());
                    return Some(Resolved::Function {
                        function,
                        impl_target,
                    });
                }
                None
            }
            Some(Type::Array(_)) if name == "length" => Some(Resolved::ArrayLength),
            _ => {
                // A dotted host path rooted at a granted namespace
                // (`game.window.OpenWindow`): the member resolves against
                // the schema contract (§9.1), visible at the mod's target
                // version (§9.5).
                if let Some(receiver) = self.receiver_token_range(dot_token_index - 1)
                    && let Some((file, segments)) = self.host_path(receiver)
                    && let [contract_name] = segments.as_slice()
                    && let Some(contract) =
                        file.contracts().find(|decl| decl.name == *contract_name)
                    && let Some(target) = self
                        .schema
                        .as_ref()
                        .and_then(|schema| schema.target(&file.namespace))
                    && let Some(member) = contract
                        .members
                        .iter()
                        .find(|decl| decl.name == name && decl.visible_at(target))
                {
                    return Some(Resolved::SchemaMember {
                        namespace: file.namespace.as_str(),
                        contract,
                        member,
                    });
                }
                None
            }
        }
    }

    /// The receiver token range in front of a dot: a maximal primary chain
    /// closed by bracket matching (`vec2(x, y).pos`, `table["k"].x`,
    /// `engine.graphics.LoadTexture(p)`). Only identifiers, dots, and
    /// balanced `(`/`[` groups participate; everything else ends the chain,
    /// so a binary operator on the left never leaks into the receiver.
    fn receiver_token_range(&self, dot_index: usize) -> Option<(usize, usize)> {
        let mut cursor = dot_index; // exclusive end; walks leftward
        while cursor > 0 {
            match &self.tokens[cursor - 1].kind {
                Tok::RParen | Tok::RBracket => {
                    // Jump over the balanced group, landing on its opener.
                    let (close, open) = if self.tokens[cursor - 1].kind == Tok::RParen {
                        (Tok::RParen, Tok::LParen)
                    } else {
                        (Tok::RBracket, Tok::LBracket)
                    };
                    let mut scan = cursor - 1;
                    let mut depth = 0;
                    loop {
                        if scan == 0 {
                            return None; // unbalanced; no receiver here
                        }
                        scan -= 1;
                        if self.tokens[scan].kind == close {
                            depth += 1;
                        } else if self.tokens[scan].kind == open {
                            if depth == 0 {
                                break;
                            }
                            depth -= 1;
                        }
                    }
                    cursor = scan;
                }
                Tok::Ident(_) | Tok::Dot => {
                    cursor -= 1;
                }
                _ => break,
            }
        }
        if cursor == dot_index {
            None
        } else {
            Some((cursor, dot_index))
        }
    }

    /// Public receiver/type pair used by completion after a dot.
    pub fn receiver_range_for_completion(&self, dot_index: usize) -> Option<(usize, usize)> {
        self.receiver_token_range(dot_index)
    }

    /// Types the receiver range found by
    /// [`Analysis::receiver_range_for_completion`].
    pub fn type_of_receiver(&self, range: (usize, usize), offset: usize) -> Option<Type> {
        self.type_of_token_range(range, offset)
    }

    /// Types a token range with the token evaluator (see module docs).
    /// `offset` is the use site that anchors local lookups.
    fn type_of_token_range(&self, range: (usize, usize), offset: usize) -> Option<Type> {
        let (start, end) = range;
        if start >= end {
            return None;
        }
        // Split at top-level dots: head + member segments.
        let mut depth: usize = 0;
        let mut segments: Vec<(usize, usize)> = Vec::new();
        let mut segment_start = start;
        for index in start..end {
            match &self.tokens[index].kind {
                Tok::LParen | Tok::LBracket | Tok::Lt => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::Gt => depth = depth.saturating_sub(1),
                Tok::Dot if depth == 0 => {
                    segments.push((segment_start, index));
                    segment_start = index + 1;
                }
                _ => {}
            }
        }
        segments.push((segment_start, end));

        let mut ty = self.type_of_head(segments[0], offset)?;
        for segment in &segments[1..] {
            ty = self.type_of_member(&ty, *segment)?;
        }
        Some(ty)
    }

    /// Types the head of a member chain: a call, an index, a literal, or a
    /// bare identifier.
    fn type_of_head(&self, range: (usize, usize), offset: usize) -> Option<Type> {
        let (start, end) = range;
        let first = self.tokens.get(start)?;
        match &first.kind {
            Tok::StrLit => return Some(Type::Prim(PrimitiveType::Str)),
            Tok::LParen => {
                // ( expr ): type the inside.
                if end >= start + 2 {
                    return self.type_of_token_range((start + 1, end - 1), offset);
                }
                return None;
            }
            _ => {}
        }
        let Tok::Ident(name) = &first.kind else {
            return None;
        };
        match self.tokens.get(start + 1).map(|t| &t.kind) {
            Some(Tok::LParen) if end > start + 2 => {
                // Call: function return type or struct construction.
                if let Some(function) = self
                    .functions
                    .iter()
                    .find(|function| &function.name == name && function.impl_index.is_none())
                {
                    return Some(function.return_ty.clone());
                }
                if self.structs.iter().any(|s| &s.name == name) {
                    return Some(Type::Named {
                        name: name.clone(),
                        args: Vec::new(),
                    });
                }
                None
            }
            Some(Tok::LBracket) => {
                // Indexing: element type of the indexed value.
                let inner = self.type_of_bare_ident(name, offset)?;
                match inner {
                    Type::Array(elem) => Some(*elem),
                    Type::Map { value, .. } => Some(*value),
                    _ => None,
                }
            }
            _ => self.type_of_bare_ident(name, offset),
        }
    }

    /// Types a bare identifier: a local, a struct, or an enum name. §9.3
    /// schema boundary types type the same way — the checker registers
    /// them into the flat type space.
    fn type_of_bare_ident(&self, name: &str, offset: usize) -> Option<Type> {
        if let Some(local) = self.resolve_local(name, offset) {
            return Some(local.ty.clone());
        }
        if self.structs.iter().any(|s| s.name == name)
            || self.enums.iter().any(|e| e.name == name)
            || self.schema_struct(name).is_some()
            || self.schema_enum(name).is_some()
        {
            return Some(Type::Named {
                name: name.to_string(),
                args: Vec::new(),
            });
        }
        if let Some(function) = self
            .functions
            .iter()
            .find(|function| function.name == name && function.impl_index.is_none())
        {
            return Some(function.return_ty.clone());
        }
        None
    }

    /// Types one member segment: field access on a struct, or `.length` on
    /// an array (§11).
    fn type_of_member(&self, ty: &Type, range: (usize, usize)) -> Option<Type> {
        let (start, _end) = range;
        let token = self.tokens.get(start)?;
        let Tok::Ident(name) = &token.kind else {
            return None;
        };
        match ty {
            Type::Named {
                name: type_name,
                args,
            } => {
                let struct_type = self.structs.iter().find(|s| &s.name == type_name)?;
                let index = struct_type
                    .fields
                    .iter()
                    .position(|(field_name, _, _)| field_name == name)?;
                let field_ty = &struct_type.fields[index].1;
                Some(substitute(field_ty, &struct_type.type_params, args))
            }
            Type::Array(_) if name == "length" => Some(Type::Prim(PrimitiveType::Int)),
            _ => None,
        }
    }

    /// The visible declaration of `name` at `offset`: the latest matching
    /// local of the enclosing function declared before the use site and
    /// scoped to a block containing it (the checker scopes locals to their
    /// block, so a local of a finished `if` body is not visible after it);
    /// a use before any declaration falls back to the first in-scope match
    /// so hover still answers.
    fn resolve_local(&'a self, name: &str, offset: usize) -> Option<&'a LocalSymbol> {
        let function_index = self.enclosing_function_index(offset)?;
        let mut best_before: Option<&LocalSymbol> = None;
        let mut first: Option<&LocalSymbol> = None;
        for local in &self.locals {
            if local.name != name || local.function != function_index {
                continue;
            }
            if !local.contains(offset) {
                continue;
            }
            if first.is_none() {
                first = Some(local);
            }
            if local.span.start <= offset {
                // Keeps the LAST declaration at or before the use site,
                // which is what shadowing resolves to.
                best_before = Some(local);
            }
        }
        best_before.or(first)
    }

    /// The enum variant with this name, across all enums — the script's
    /// own and the schema's §9.3 boundary enums (variant names are unique
    /// in the flat type space the checker enforces).
    fn find_variant(
        &'a self,
        name: &str,
    ) -> Option<(&'a EnumSymbol, &'a crate::analysis::VariantSymbol)> {
        for enum_type in self.enums.iter().chain(self.schema_enums.iter()) {
            if let Some(variant) = enum_type.variants.iter().find(|v| v.name == name) {
                return Some((enum_type, variant));
            }
        }
        None
    }

    /// The innermost AST expression containing `offset`.
    pub fn expr_at(&'a self, offset: usize) -> Option<&'a Expr> {
        for statement in self.statements {
            if let Some(expr) = self.expr_in_stmt(statement, offset) {
                return Some(expr);
            }
        }
        None
    }

    fn expr_in_stmt(&'a self, statement: &'a Stmt, offset: usize) -> Option<&'a Expr> {
        match &statement.kind {
            StmtKind::VarDecl { expr, .. }
            | StmtKind::Assign { expr, .. }
            | StmtKind::CompoundAssign { expr, .. }
            | StmtKind::Expression { expr } => self.expr_in_expr(expr, offset),
            StmtKind::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => self
                .expr_in_expr(cond, offset)
                .or_else(|| self.expr_in_block(then_branch, offset))
                .or_else(|| {
                    else_branch
                        .as_deref()
                        .and_then(|branch| self.expr_in_stmt(branch, offset))
                }),
            StmtKind::While { cond, body } => self
                .expr_in_expr(cond, offset)
                .or_else(|| self.expr_in_block(body, offset)),
            StmtKind::For { iterable, body, .. } => self
                .expr_in_expr(iterable, offset)
                .or_else(|| self.expr_in_block(body, offset)),
            StmtKind::Match { scrutinee, arms } => {
                if let Some(expr) = self.expr_in_expr(scrutinee, offset) {
                    return Some(expr);
                }
                arms.iter()
                    .find_map(|arm| self.expr_in_block(&arm.body, offset))
            }
            StmtKind::Return { value } => value
                .as_ref()
                .and_then(|expr| self.expr_in_expr(expr, offset)),
            _ => None,
        }
    }

    fn expr_in_block(&'a self, block: &'a cme_core::ast::Block, offset: usize) -> Option<&'a Expr> {
        if !(block.span.start <= offset && offset <= block.span.end) {
            return None;
        }
        block
            .stmts
            .iter()
            .find_map(|statement| self.expr_in_stmt(statement, offset))
    }

    /// The deepest expression whose span contains `offset`.
    fn expr_in_expr(&'a self, expr: &'a Expr, offset: usize) -> Option<&'a Expr> {
        if !(expr.span.start <= offset && offset <= expr.span.end) {
            return None;
        }
        // Descend first so the innermost node wins.
        let inner = match &expr.kind {
            ExprKind::Paren { expr } | ExprKind::Unary { expr, .. } | ExprKind::Try { expr } => {
                self.expr_in_expr(expr, offset)
            }
            ExprKind::Binary { lhs, rhs, .. } => self
                .expr_in_expr(lhs, offset)
                .or_else(|| self.expr_in_expr(rhs, offset)),
            ExprKind::Call { args, .. } | ExprKind::VariantCall { args, .. } => {
                args.iter().find_map(|arg| match arg {
                    cme_core::ast::CallArg::Positional(expr) => self.expr_in_expr(expr, offset),
                    cme_core::ast::CallArg::Named { expr, .. } => self.expr_in_expr(expr, offset),
                })
            }
            ExprKind::PathCall { args, .. } => args.iter().find_map(|arg| match arg {
                cme_core::ast::CallArg::Positional(expr) => self.expr_in_expr(expr, offset),
                cme_core::ast::CallArg::Named { expr, .. } => self.expr_in_expr(expr, offset),
            }),
            ExprKind::Field { obj, .. } => self.expr_in_expr(obj, offset),
            ExprKind::Index { obj, index } => self
                .expr_in_expr(obj, offset)
                .or_else(|| self.expr_in_expr(index, offset)),
            ExprKind::Match { scrutinee, arms } => {
                self.expr_in_expr(scrutinee, offset).or_else(|| {
                    arms.iter()
                        .find_map(|arm| self.expr_in_expr(&arm.body, offset))
                })
            }
            ExprKind::ArrayLit { elements } => elements
                .iter()
                .find_map(|element| self.expr_in_expr(element, offset)),
            ExprKind::MapLit { entries } => entries.iter().find_map(|(key, value)| {
                self.expr_in_expr(key, offset)
                    .or_else(|| self.expr_in_expr(value, offset))
            }),
            ExprKind::Interpolated { parts } => parts.iter().find_map(|part| match part {
                cme_core::ast::InterpPart::Expr(expr) => self.expr_in_expr(expr, offset),
                _ => None,
            }),
            _ => None,
        };
        Some(inner.unwrap_or(expr))
    }
}

/// `int`-style keywords resolve as built-in types for hover.
fn builtin_type_from_keyword(kind: &Tok) -> Option<Resolved<'static>> {
    let (name, description) = match kind {
        Tok::KwInt => ("int", "signed 64-bit integer (§2.4)"),
        Tok::KwFloat => ("float", "64-bit IEEE 754 float (§2.4)"),
        Tok::KwBool => ("bool", "`true` or `false` (§2.4)"),
        Tok::KwStr => ("str", "immutable UTF-8 string (§2.4)"),
        Tok::KwVoid => ("void", "function returning no value (§2.4)"),
        Tok::KwMap => ("map", "`map<K, V>` keyed collection (§11)"),
        _ => return None,
    };
    Some(Resolved::BuiltinType {
        name: name.to_string(),
        description,
    })
}

/// Substitutes generic parameters with concrete arguments (`pair<A, B>`'
/// `A first` with args `[int, str]` → `int first`).
pub fn substitute(ty: &Type, params: &[String], args: &[Type]) -> Type {
    match ty {
        Type::Named { name, args: inner } => {
            // A bare reference to one of the parameters crystallizes to
            // its concrete argument; a parametric use (`pair<A, B> next`)
            // substitutes recursively.
            if inner.is_empty()
                && let Some(position) = params.iter().position(|param| param == name)
                && let Some(concrete) = args.get(position)
            {
                return concrete.clone();
            }
            Type::Named {
                name: name.clone(),
                args: inner
                    .iter()
                    .map(|arg| substitute(arg, params, args))
                    .collect(),
            }
        }
        Type::Array(elem) => Type::Array(Box::new(substitute(elem, params, args))),
        Type::Map { key, value } => Type::Map {
            key: Box::new(substitute(key, params, args)),
            value: Box::new(substitute(value, params, args)),
        },
        other => other.clone(),
    }
}

/// The AST evaluator: the static type of an expression when it is
/// unambiguous (§2.16). Ambiguous shapes return `None` rather than guess.
pub fn infer_expr_type(
    expr: &Expr,
    structs: &[StructSymbol],
    functions: &[crate::analysis::FunctionSymbol],
    locals: &[LocalSymbol],
) -> Option<Type> {
    match &expr.kind {
        ExprKind::IntLit(_) => Some(Type::Prim(PrimitiveType::Int)),
        ExprKind::FloatLit(_) => Some(Type::Prim(PrimitiveType::Float)),
        ExprKind::StrLit(_) => Some(Type::Prim(PrimitiveType::Str)),
        ExprKind::BoolLit(_) => Some(Type::Prim(PrimitiveType::Bool)),
        ExprKind::Interpolated { .. } => Some(Type::Prim(PrimitiveType::Str)),
        ExprKind::Paren { expr } => infer_expr_type(expr, structs, functions, locals),
        ExprKind::Ident(name) => locals
            .iter()
            .rev()
            .find(|local| &local.name == name)
            .map(|local| local.ty.clone()),
        ExprKind::Call { name, .. } => {
            if structs.iter().any(|s| &s.name == name) {
                return Some(Type::Named {
                    name: name.clone(),
                    args: Vec::new(),
                });
            }
            functions
                .iter()
                .find(|function| &function.name == name)
                .map(|function| function.return_ty.clone())
        }
        ExprKind::VariantCall { enum_name, .. } => Some(Type::Named {
            name: enum_name.clone(),
            args: Vec::new(),
        }),
        ExprKind::Field { obj, name } => {
            let obj_ty = infer_expr_type(obj, structs, functions, locals)?;
            match &obj_ty {
                Type::Named {
                    name: type_name,
                    args,
                } => {
                    let struct_type = structs.iter().find(|s| &s.name == type_name)?;
                    let index = struct_type
                        .fields
                        .iter()
                        .position(|(field_name, _, _)| field_name == name)?;
                    Some(substitute(
                        &struct_type.fields[index].1,
                        &struct_type.type_params,
                        args,
                    ))
                }
                Type::Array(_) if name == "length" => Some(Type::Prim(PrimitiveType::Int)),
                _ => None,
            }
        }
        ExprKind::Index { obj, .. } => {
            let obj_ty = infer_expr_type(obj, structs, functions, locals)?;
            match obj_ty {
                Type::Array(elem) => Some(*elem),
                Type::Map { value, .. } => Some(*value),
                _ => None,
            }
        }
        ExprKind::Binary { op, lhs, rhs } => match op {
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::And
            | BinaryOp::Or => Some(Type::Prim(PrimitiveType::Bool)),
            BinaryOp::Add => {
                let lhs_ty = infer_expr_type(lhs, structs, functions, locals)?;
                let rhs_ty = infer_expr_type(rhs, structs, functions, locals)?;
                match (&lhs_ty, &rhs_ty) {
                    (Type::Prim(PrimitiveType::Str), _) | (_, Type::Prim(PrimitiveType::Str)) => {
                        Some(Type::Prim(PrimitiveType::Str))
                    }
                    _ => Some(lhs_ty),
                }
            }
            BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                infer_expr_type(lhs, structs, functions, locals)
            }
        },
        ExprKind::Unary { op, expr } => match op {
            cme_core::ast::UnaryOp::Not => Some(Type::Prim(PrimitiveType::Bool)),
            cme_core::ast::UnaryOp::Neg => infer_expr_type(expr, structs, functions, locals),
        },
        ExprKind::Match { arms, .. } => arms
            .first()
            .and_then(|arm| infer_expr_type(&arm.body, structs, functions, locals)),
        ExprKind::ArrayLit { elements } => elements.first().and_then(|element| {
            infer_expr_type(element, structs, functions, locals)
                .map(|elem| Type::Array(Box::new(elem)))
        }),
        ExprKind::Try { expr } => {
            // result<T, E> unwraps to T (§2.8).
            let inner = infer_expr_type(expr, structs, functions, locals)?;
            match inner {
                Type::Named { name, args } if name == "result" && !args.is_empty() => {
                    Some(args[0].clone())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Renders any [`Resolved`] as hover markdown.
pub fn hover_markdown(resolved: &Resolved<'_>) -> String {
    match resolved {
        Resolved::Local(local) => {
            let qualifier = match local.kind {
                LocalKind::Param => "parameter",
                LocalKind::Variable => "variable",
                LocalKind::ForBinding => "for binding",
                LocalKind::MatchBinding => "match binding",
            };
            format!(
                "```checkmate\n{}: {}\n```\n*{}*",
                local.name,
                render_type(&local.ty),
                qualifier
            )
        }
        Resolved::Function {
            function,
            impl_target,
        } => {
            let params: Vec<String> = function
                .params
                .iter()
                .map(|param| format!("{} {}", render_type(&param.ty), param.name))
                .collect();
            let prefix = match impl_target {
                Some(target) => {
                    let path: Vec<&str> =
                        target.iter().map(|(segment, _)| segment.as_str()).collect();
                    format!("impl {}.{}", path.join("."), function.name)
                }
                None => function.name.clone(),
            };
            format!(
                "```checkmate\n{} {}({})\n```",
                render_type(&function.return_ty),
                prefix,
                params.join(", ")
            )
        }
        Resolved::Struct(struct_type) => {
            let params = generics(&struct_type.type_params);
            let fields: Vec<String> = struct_type
                .fields
                .iter()
                .map(|(name, ty, _)| format!("    {}: {}", name, render_type(ty)))
                .collect();
            format!(
                "```checkmate\nstruct {}{} {{\n{}\n}}\n```",
                struct_type.name,
                params,
                fields.join("\n")
            )
        }
        Resolved::Enum(enum_type) => {
            let params = generics(&enum_type.type_params);
            let variants: Vec<String> = enum_type
                .variants
                .iter()
                .map(|variant| {
                    let fields: Vec<String> = variant
                        .fields
                        .iter()
                        .map(|(name, ty)| format!("{}: {}", name, render_type(ty)))
                        .collect();
                    if fields.is_empty() {
                        format!("    {}()", variant.name)
                    } else {
                        format!("    {}({})", variant.name, fields.join(", "))
                    }
                })
                .collect();
            format!(
                "```checkmate\nenum {}{} {{\n{}\n}}\n```",
                enum_type.name,
                params,
                variants.join("\n")
            )
        }
        Resolved::Variant { enum_type, variant } => {
            let fields: Vec<String> = variant
                .fields
                .iter()
                .map(|(name, ty)| format!("{}: {}", name, render_type(ty)))
                .collect();
            format!(
                "```checkmate\n{}.{}({})\n```",
                enum_type.name,
                variant.name,
                fields.join(", ")
            )
        }
        Resolved::Field {
            struct_type,
            index,
            substituted,
        } => {
            let (name, ty, _) = &struct_type.fields[*index];
            let rendered = substituted
                .as_ref()
                .map(render_type)
                .unwrap_or_else(|| render_type(ty));
            format!(
                "```checkmate\n{}: {}\n```\n*field of* `{}`",
                name, rendered, struct_type.name
            )
        }
        Resolved::ImportSegment { import, segment } => {
            let path: Vec<&str> = import
                .segments
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            format!(
                "```checkmate\nimport {}\n```\n*path segment {}*",
                path.join("."),
                segment
            )
        }
        Resolved::SchemaStruct(struct_type) => {
            let fields: Vec<String> = struct_type
                .fields
                .iter()
                .map(|(name, ty, _)| format!("    {}: {}", name, render_type(ty)))
                .collect();
            format!(
                "```checkmate\nstruct {} {{\n{}\n}}\n```\n*§9.3 boundary type — declared in the schema*",
                struct_type.name,
                fields.join("\n")
            )
        }
        Resolved::SchemaEnum(enum_type) => {
            let variants: Vec<String> = enum_type
                .variants
                .iter()
                .map(|variant| {
                    let fields: Vec<String> = variant
                        .fields
                        .iter()
                        .map(|(name, ty)| format!("{}: {}", name, render_type(ty)))
                        .collect();
                    if fields.is_empty() {
                        format!("    {}()", variant.name)
                    } else {
                        format!("    {}({})", variant.name, fields.join(", "))
                    }
                })
                .collect();
            format!(
                "```checkmate\nenum {} {{\n{}\n}}\n```\n*§9.3 boundary type — declared in the schema*",
                enum_type.name,
                variants.join("\n")
            )
        }
        Resolved::SchemaContract {
            namespace,
            contract,
        } => {
            let kind = contract.kind.keyword();
            let members: Vec<String> = contract.members.iter().map(schema_member_line).collect();
            format!(
                "```checkmate\n{} {}.{}\n```\n```checkmate\n{}\n```\n*{} member(s) — §9.1*",
                kind,
                namespace,
                contract.name,
                if members.is_empty() {
                    "    // no members".to_string()
                } else {
                    members.join("\n")
                },
                contract.members.len()
            )
        }
        Resolved::SchemaMember {
            namespace,
            contract,
            member,
        } => {
            let params: Vec<String> = member
                .params
                .iter()
                .map(|param| format!("{} {}", render_type(&param.ty), param.name))
                .collect();
            let mut notes = Vec::new();
            if member.since != cme_core::schema::Version::ZERO {
                notes.push(format!("since {}", member.since));
            }
            if member.requirement == cme_core::schema::MemberRequirement::Optional {
                notes.push("optional".to_string());
            }
            let note = if notes.is_empty() {
                String::new()
            } else {
                format!("\n*{}*", notes.join(", "))
            };
            format!(
                "```checkmate\n{} {}.{}.{}({})\n```\n*{} member* (§9.1){}",
                render_type(&member.return_ty),
                namespace,
                contract.name,
                member.name,
                params.join(", "),
                contract.kind.keyword(),
                note
            )
        }
        Resolved::BuiltinType { name, description } => {
            format!("```checkmate\n{}\n```\n{description}", name)
        }
        Resolved::BuiltinConstructor { name, description } => {
            format!("```checkmate\n{}\n```\n{description}", name)
        }
        Resolved::ArrayLength => {
            "```checkmate\narray.length\n```\n*int* — the number of elements (§11)".to_string()
        }
    }
}

fn generics(type_params: &[String]) -> String {
    if type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", type_params.join(", "))
    }
}

/// One schema member rendered as a signature line: `Sprite OpenWindow(str title)`.
fn schema_member_line(member: &SchemaMember) -> String {
    let params: Vec<String> = member
        .params
        .iter()
        .map(|param| format!("{} {}", render_type(&param.ty), param.name))
        .collect();
    format!(
        "    {} {}({})",
        render_type(&member.return_ty),
        member.name,
        params.join(", ")
    )
}
