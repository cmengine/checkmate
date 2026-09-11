use crate::diagnostics::Diagnostic;
use crate::lexer::{SpannedToken, Token};
use crate::validate;

use cme_core::Span;
use cme_core::ast::{
    BinaryOp, Block, CallArg, CompoundOp, ErrorId, Expr, ExprKind, FieldDef, InterpPart, LValue,
    MatchArmExpr, MatchArmStmt, Param, Pattern, PrimitiveType, Stmt, StmtKind, Type, UnaryOp,
    VariantDecl,
};

/// The maximum recursion depth of one parse: parenthesized and unary
/// expression nesting, statement (block) nesting, and generic type nesting
/// all count. Each level costs kilobytes of stack, so the cap is sized to
/// keep the whole parse inside even a 2 MiB worker thread — the Rust
/// default for spawned threads — while remaining far beyond any
/// hand-written or generated program's needs.
pub const MAX_NESTING_DEPTH: usize = 128;

/// The maximum number of binary operators in one statement's expressions.
/// Real statements use a handful; a left-deep chain of this size is already
/// a tree of this depth for every downstream recursive pass.
pub const MAX_EXPR_OPERATORS: usize = 2048;

/// The maximum number of type nodes (generic arguments + array suffixes)
/// in one statement's types.
pub const MAX_TYPE_NODES: usize = 4096;

pub struct Parser<'a, 'src> {
    tokens: &'a [SpannedToken<'src>],
    pos: usize,
    /// Diagnostics recorded so far. Recoverable errors are pushed here while
    /// an `Invalid` node is planted into the AST at the failure site, so the
    /// parser never stops and always produces the fullest possible tree.
    errors: Vec<Diagnostic>,
    /// Current recursion depth (parenthesized/unary expression nesting,
    /// statement nesting, generic type nesting). Adversarially deep source
    /// must fail with a clean diagnostic instead of overflowing the stack —
    /// the parser never panics (boom.cm contract, §7 DoS defense).
    depth: usize,
    /// Binary operators built for the current statement's expressions.
    /// A left-deep chain of N operators is an N-deep AST subtree, which the
    /// validator, checker, and interpreter later recurse over — so the
    /// parser bounds the total per statement, islands included.
    expr_ops: usize,
    /// Type nodes built for the current statement (generic arguments and
    /// array suffixes). Bounds the `Type` tree depth the checker recurses.
    type_nodes: usize,
}

impl<'a, 'src> Parser<'a, 'src> {
    pub fn new(tokens: &'a [SpannedToken<'src>]) -> Self {
        Self::with_budget(tokens, 0, 0, 0)
    }

    /// A parser whose depth and complexity budgets continue the caller's.
    /// Interpolation islands parse their text with a fresh token stream but
    /// the same nesting stack — otherwise adversarially nested `$"..."`
    /// strings would reset the depth guard per island.
    fn with_budget(
        tokens: &'a [SpannedToken<'src>],
        depth: usize,
        expr_ops: usize,
        type_nodes: usize,
    ) -> Self {
        debug_assert!(
            matches!(tokens.last(), Some(token) if token.token == Token::Eof),
            "token stream must end with a synthetic Eof"
        );
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            depth,
            expr_ops,
            type_nodes,
        }
    }

    /// Enters one level of recursion. Exceeding the nesting limit is a
    /// clean parse error, never a stack overflow.
    fn enter(&mut self) -> Result<(), Diagnostic> {
        self.depth += 1;
        if self.depth > MAX_NESTING_DEPTH {
            let span = self.peek().span;
            return Err(Diagnostic::parse("expression nesting is too deep", span));
        }
        Ok(())
    }

    /// Counts one more binary operator for the current statement.
    fn count_expr_op(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.expr_ops += 1;
        if self.expr_ops > MAX_EXPR_OPERATORS {
            return Err(Diagnostic::parse("expression is too complex", span));
        }
        Ok(())
    }

    /// Counts one more type node for the current statement; `false` when
    /// the type has grown past its budget.
    fn count_type_node(&mut self) -> bool {
        self.type_nodes += 1;
        self.type_nodes <= MAX_TYPE_NODES
    }

    /// Records a recoverable diagnostic and returns its index for embedding
    /// into an `Invalid` AST node.
    fn record(&mut self, message: impl Into<String>, span: Span) -> ErrorId {
        let id = self.errors.len();
        self.errors.push(Diagnostic::parse(message, span));
        ErrorId(id)
    }

    /// Drains the diagnostics recorded so far. Useful alongside
    /// [`Parser::parse_statement`] when parsing a single statement directly.
    pub fn take_errors(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.errors)
    }

    /// Current token. Always valid: the stream ends with `Eof`.
    fn peek(&self) -> &SpannedToken<'src> {
        &self.tokens[self.pos]
    }

    /// The end-of-file span (zero-width, at end of input).
    fn eof_span(&self) -> Span {
        self.tokens
            .last()
            .map_or(Span::new(0, 0), |token| token.span)
    }

    /// True when the current token is `kind` (use with unit variants).
    fn at(&self, kind: Token<'src>) -> bool {
        self.peek().token == kind
    }

    /// Consumes and returns the current token; saturates at `Eof`.
    fn advance(&mut self) -> SpannedToken<'src> {
        let token = self.tokens[self.pos];
        if token.token != Token::Eof {
            self.pos += 1;
        }
        token
    }

    fn at_eof(&self) -> bool {
        self.at(Token::Eof)
    }

    fn skip_newlines(&mut self) {
        while self.at(Token::Newline) {
            self.pos += 1;
        }
    }

    fn expected(what: &str, found: &Token<'_>, span: Span) -> Diagnostic {
        Diagnostic::parse(
            format!("expected {what}, but found {}", found.describe()),
            span,
        )
    }

    /// Fails fast on the first diagnostic. For tolerant parsing, prefer
    /// [`crate::parse_source`].
    pub fn parse_program(&mut self) -> Result<Vec<Stmt>, Diagnostic> {
        let (stmts, errors) = self.parse_program_with_errors();
        match errors.into_iter().next() {
            Some(error) => Err(error),
            None => Ok(stmts),
        }
    }

    /// Parses a whole program with error recovery. Statements that cannot be
    /// parsed become [`Stmt::Invalid`]; broken expressions inside otherwise
    /// recognizable statements become [`Expr::Invalid`], so declarations stay
    /// visible to tooling (a future LSP can still see `int i` in
    /// `int i = <broken>`). The returned diagnostics are the canonical list
    /// for reporting; execution consumers should refuse to run while it is
    /// non-empty.
    pub fn parse_program_with_errors(&mut self) -> (Vec<Stmt>, Vec<Diagnostic>) {
        let (stmts, mut errors) = self.parse_statement_inner();
        errors.extend(validate::validate_statements(&stmts));
        (stmts, errors)
    }

    fn parse_statement_inner(&mut self) -> (Vec<Stmt>, Vec<Diagnostic>) {
        let mut stmts = Vec::new();

        loop {
            self.skip_newlines();
            if self.at_eof() {
                break;
            }

            stmts.push(self.parse_statement());

            let token = self.peek().token;
            match token {
                Token::Newline => self.pos += 1,
                Token::Eof => {}
                // Recovery inside a declaration body (struct/enum) leaves the
                // cursor on the next declaration head with no intervening
                // newline: that is a valid boundary, not a missing separator.
                _ if token.starts_statement() => {}
                _ => {
                    let span = self.peek().span;
                    self.errors
                        .push(Self::expected("end of statement", &token, span));
                    self.skip_to_next_statement();
                }
            }
        }

        let errors = std::mem::take(&mut self.errors);
        (stmts, errors)
    }

    pub fn strip_insignificant_newlines(
        tokens: Vec<SpannedToken>,
    ) -> Result<Vec<SpannedToken>, Diagnostic> {
        let (tokens, errors) = Self::strip_insignificant_newlines_with_errors(tokens);
        match errors.into_iter().next() {
            Some(error) => Err(error),
            None => Ok(tokens),
        }
    }

    pub fn strip_insignificant_newlines_with_errors(
        tokens: Vec<SpannedToken>,
    ) -> (Vec<SpannedToken>, Vec<Diagnostic>) {
        // Stack of currently open brackets. A newline is insignificant only
        // inside parentheses (§A.8): expressions and argument lists wrap
        // freely there. Inside brackets and braces a newline is always
        // significant — array elements, map entries, match arms, struct
        // fields, and block statements are newline-delimited (§2.6, §2.12,
        // §2.15, §11.1).
        let mut stack: Vec<BracketKind> = Vec::new();
        let mut out = Vec::with_capacity(tokens.len());
        let mut errors = Vec::new();
        let mut prev_can_end = false;
        let mut prev_was_type_kw = false;
        let mut prev_was_return = false;

        let mut index = 0usize;
        while index < tokens.len() {
            let SpannedToken { token: tok, span } = tokens[index];
            match tok {
                Token::Newline => {
                    // A newline survives at the top of the stack when it
                    // genuinely ends a statement — or when it follows a
                    // region the parser must still see as broken on its own
                    // line:
                    //   - a dangling type keyword (`float` alone) can never
                    //     continue onto the next line, and
                    //   - a declaration head can only START a statement, so
                    //     a line ending in a dangling operator (`int count =`)
                    //     must not swallow the declaration typed below it.
                    // Without this, recovery would fuse the broken line with
                    // the next one and eat the very declaration an LSP needs.
                    let next_starts_statement = tokens
                        .get(index + 1)
                        .is_some_and(|next| next.token.starts_statement());
                    // §A.8: a line that cannot end a statement continues
                    // onto the next line — a trailing binary operator or an
                    // open call joins across the break, inside braces and
                    // brackets as much as at the top level. `return` is the
                    // restricted production: a bare `return` ends at its
                    // line even though the keyword cannot end an expression,
                    // so `return` + newline + next statement stays two
                    // statements.
                    let significant = match stack.last() {
                        Some(BracketKind::Paren) => false,
                        _ => {
                            prev_can_end
                                || prev_was_type_kw
                                || prev_was_return
                                || next_starts_statement
                        }
                    };
                    if significant {
                        out.push(SpannedToken { token: tok, span });
                        prev_can_end = false;
                        prev_was_type_kw = false;
                    }
                }
                Token::LParen => {
                    stack.push(BracketKind::Paren);
                    prev_can_end = false;
                    prev_was_type_kw = false;
                    prev_was_return = false;
                    out.push(SpannedToken { token: tok, span });
                }
                Token::LBracket => {
                    stack.push(BracketKind::Bracket);
                    prev_can_end = false;
                    prev_was_type_kw = false;
                    prev_was_return = false;
                    out.push(SpannedToken { token: tok, span });
                }
                Token::LBrace => {
                    stack.push(BracketKind::Brace);
                    prev_can_end = false;
                    prev_was_type_kw = false;
                    prev_was_return = false;
                    out.push(SpannedToken { token: tok, span });
                }
                Token::RParen | Token::RBracket | Token::RBrace => {
                    let expected = match tok {
                        Token::RParen => BracketKind::Paren,
                        Token::RBracket => BracketKind::Bracket,
                        _ => BracketKind::Brace,
                    };
                    if stack.last() == Some(&expected) {
                        stack.pop();
                        prev_can_end = true;
                        prev_was_type_kw = false;
                        prev_was_return = false;
                        out.push(SpannedToken { token: tok, span });
                    } else if let Some(depth) = stack.iter().rposition(|kind| *kind == expected) {
                        // An inner bracket was left open by broken source
                        // (an unclosed payload list, call, or index). Close at
                        // this match instead of failing: the parser's own
                        // recovery reports the real damage with a precise
                        // diagnostic, and the enclosing declaration survives.
                        stack.truncate(depth);
                        prev_can_end = true;
                        prev_was_type_kw = false;
                        prev_was_return = false;
                        out.push(SpannedToken { token: tok, span });
                    } else if stack.is_empty() && matches!(tok, Token::RBracket | Token::RBrace) {
                        // A stray `]` or `}` at the top of the stream is kept
                        // as a plain token: the parser reports it as an
                        // unrecognizable statement (boom.cm pins this).
                        prev_can_end = can_end_statement(&tok);
                        prev_was_type_kw = false;
                        prev_was_return = false;
                        out.push(SpannedToken { token: tok, span });
                    } else {
                        // A mismatched or dangling closing bracket: report,
                        // resynchronize at the next statement boundary, and
                        // reset the stack.
                        let what = match tok {
                            Token::RParen => "closing parenthesis",
                            Token::RBracket => "closing bracket",
                            _ => "closing brace",
                        };
                        errors.push(Diagnostic::parse(format!("unbalanced {what}"), span));
                        if let Some(token) = skip_to_next_statement(&tokens, &mut index) {
                            out.push(token);
                        }
                        stack.clear();
                        prev_can_end = false;
                        prev_was_type_kw = false;
                        continue;
                    }
                }
                _ => {
                    prev_can_end = can_end_statement(&tok);
                    prev_was_type_kw = tok.is_type_keyword();
                    prev_was_return = tok == Token::KwReturn;
                    out.push(SpannedToken { token: tok, span });
                }
            }
            index += 1;
        }

        if let Some(kind) = stack.last() {
            let eof_span = tokens
                .last()
                .filter(|token| token.token == Token::Eof)
                .map_or(Span::new(0, 0), |token| token.span);
            let what = match kind {
                BracketKind::Paren => "unbalanced opening parenthesis",
                BracketKind::Bracket => "unbalanced opening bracket",
                BracketKind::Brace => "unbalanced opening brace",
            };
            errors.push(Diagnostic::parse(what, eof_span));
        }

        (out, errors)
    }

    fn skip_to_next_statement(&mut self) {
        skip_to_next_statement(self.tokens, &mut self.pos);
    }

    /// Advances past every token up to (but not including) the next newline
    /// token or end of file, leaving the parser at that boundary. Returns the
    /// end offset of the consumed region, never less than `end`.
    fn skip_to_statement_end(&mut self, end: usize) -> usize {
        let mut end = end;
        loop {
            let token = *self.peek();
            if matches!(token.token, Token::Newline | Token::Eof) {
                break;
            }
            end = end.max(token.span.end);
            self.advance();
        }
        end
    }

    /// Advances past the failed statement, stopping before the next line that
    /// begins a declaration or control-flow construct. This lets sibling
    /// functions and statements survive a broken header.
    fn recover_to_next_statement(&mut self, end: usize) -> usize {
        let end = self.skip_to_statement_end(end);
        if self.peek().token == Token::Newline {
            self.pos += 1;
        }
        end
    }

    /// Parses an initializer or assignment right-hand side with recovery.
    ///
    /// On failure the diagnostic is recorded, tokens up to the statement
    /// boundary are skipped, and an `Invalid` expression is planted covering
    /// the broken region — zero-width when nothing was written at all (a
    /// "missing node"), so the enclosing statement always survives.
    fn parse_recovered_expression(&mut self) -> Expr {
        let start_pos = self.pos;
        match self.parse_expression() {
            Ok(expr) => expr,
            Err(diagnostic) => {
                let start = self.tokens[start_pos].span.start;
                let mut end = start;
                if self.pos > start_pos {
                    end = self.tokens[self.pos - 1].span.end;
                }
                let end = self.skip_to_statement_end(end);
                self.errors.push(diagnostic);
                Expr {
                    span: Span::new(start, end),
                    kind: ExprKind::Invalid {
                        error: ErrorId(self.errors.len() - 1),
                    },
                }
            }
        }
    }

    /// Plants a zero-width `Invalid` expression at `offset`, marking source
    /// text that is missing entirely (for example an initializer the user has
    /// not typed yet). The diagnostic is recorded like any other error.
    fn missing_expression(
        &mut self,
        message: impl Into<String>,
        span: Span,
        offset: usize,
    ) -> Expr {
        let error = self.record(message, span);
        Expr {
            span: Span::missing(offset),
            kind: ExprKind::Invalid { error },
        }
    }

    fn require_token(&mut self, message: &str) -> Result<SpannedToken<'src>, Diagnostic> {
        let token = self.advance();
        if token.token == Token::Eof {
            return Err(Diagnostic::parse(message, token.span));
        }
        Ok(token)
    }

    /// Parses one statement. Infallible: a statement whose structure cannot
    /// be recognized becomes [`Stmt::Invalid`] covering the skipped region,
    /// and its diagnostic is recorded for [`Self::take_errors`] or
    /// [`Self::parse_program_with_errors`]. Expression-level validation
    /// (Appendix A §A.3) is deliberately NOT run here: it runs once over the
    /// finished program in [`Self::parse_program_with_errors`], so a
    /// violation nested anywhere is reported exactly once.
    ///
    /// The guard wrapper tracks recursion depth (statement/blocks nest via
    /// this entry point) and re-arms the per-statement complexity budgets;
    /// the parsing itself lives in [`Self::parse_statement_at`].
    pub fn parse_statement(&mut self) -> Stmt {
        if self.depth >= MAX_NESTING_DEPTH {
            let span = self.peek().span;
            let end = self.skip_to_statement_end(span.end);
            let error = self.record("statement nesting is too deep", span);
            return Stmt {
                span: Span::new(span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }
        self.depth += 1;
        self.expr_ops = 0;
        self.type_nodes = 0;
        let statement = self.parse_statement_at();
        self.depth -= 1;
        statement
    }

    fn parse_statement_at(&mut self) -> Stmt {
        if self.at_eof() {
            let eof_span = self.eof_span();
            let error = self.record("unexpected end of file", eof_span);
            return Stmt {
                span: Span::missing(eof_span.start),
                kind: StmtKind::Invalid { error },
            };
        }
        let first = self.advance();

        if first.token == Token::KwIf {
            return self.parse_if_statement(first);
        }
        if first.token == Token::KwWhile {
            return self.parse_while_statement(first);
        }
        if first.token == Token::KwReturn {
            return self.parse_return_statement(first);
        }
        if first.token == Token::KwStruct {
            return self.parse_struct_declaration(first);
        }
        if first.token == Token::KwMatch {
            return self.parse_match_statement(first);
        }
        if first.token == Token::KwFor {
            return self.parse_for_statement(first);
        }
        if first.token == Token::KwEnum {
            return self.parse_enum_declaration(first);
        }
        if first.token == Token::KwImpl {
            return self.parse_impl_declaration(first);
        }
        if first.token == Token::KwImport {
            return self.parse_import_statement(first);
        }
        if first.token == Token::KwVoid {
            if matches!(self.peek().token, Token::Ident(_))
                && !matches!(
                    self.tokens.get(self.pos + 1).map(|t| t.token),
                    Some(Token::LParen)
                )
            {
                return self.parse_void_misuse(first);
            }
            return self.parse_type_declaration(first, "a function name");
        }

        if first.token.is_type_keyword() {
            return self.parse_type_declaration(first, "a variable name");
        }

        if let Token::Ident(_) = &first.token {
            return self.parse_ident_led_statement(first);
        }

        let message = format!(
            "expected a type or assignment target, but found {}",
            first.token.describe()
        );
        let end = self.skip_to_statement_end(first.span.end);
        let error = self.record(message, first.span);
        Stmt {
            span: Span::new(first.span.start, end),
            kind: StmtKind::Invalid { error },
        }
    }

    fn parse_type_from_token(token: &Token<'_>) -> Option<Type> {
        match token {
            Token::KwInt => Some(Type::Prim(PrimitiveType::Int)),
            Token::KwFloat => Some(Type::Prim(PrimitiveType::Float)),
            Token::KwStr => Some(Type::Prim(PrimitiveType::Str)),
            Token::KwBool => Some(Type::Prim(PrimitiveType::Bool)),
            Token::KwInfer => Some(Type::Infer),
            Token::KwVoid => Some(Type::Void),
            _ => None,
        }
    }

    /// Parses a type starting at the current token: a scalar keyword, a
    /// named struct/enum type (with optional generic arguments, §2.9), a
    /// `map<K, V>` (§11), each optionally followed by `[]` array suffixes
    /// (§11). Consumes only on success; `None` leaves the caller to report
    /// and recover. Never records diagnostics, so it is also safe for
    /// speculative parses (callers save/restore `pos` themselves).
    fn parse_type(&mut self) -> Option<Type> {
        if self.depth >= MAX_NESTING_DEPTH {
            return None;
        }
        self.depth += 1;
        let result = self.parse_type_guarded();
        self.depth -= 1;
        result
    }

    fn parse_type_guarded(&mut self) -> Option<Type> {
        let token = *self.peek();
        let base = match token.token {
            Token::KwInt
            | Token::KwFloat
            | Token::KwStr
            | Token::KwBool
            | Token::KwInfer
            | Token::KwVoid => {
                let ty = Self::parse_type_from_token(&token.token)?;
                self.advance();
                ty
            }
            Token::Ident(name) => {
                self.advance();
                if self.at(Token::Lt) {
                    self.advance();
                    return self.parse_type_generic_tail(name.to_string());
                }
                if !self.count_type_node() {
                    return None;
                }
                Type::Named {
                    name: name.to_string(),
                    args: Vec::new(),
                }
            }
            _ => return None,
        };
        self.parse_type_array_suffixes(base)
    }

    /// After `Name <` (consumed): generic arguments for a named type or the
    /// `map<K, V>` builtin (§2.9, §11).
    fn parse_type_generic_tail(&mut self, name: String) -> Option<Type> {
        let first = self.parse_type()?;
        if name == "map" {
            if !self.at(Token::Comma) {
                return None;
            }
            self.advance();
            let value = self.parse_type()?;
            if !self.at(Token::Gt) {
                return None;
            }
            self.advance();
            if !self.count_type_node() {
                return None;
            }
            return self.parse_type_array_suffixes(Type::Map {
                key: Box::new(first),
                value: Box::new(value),
            });
        }
        let mut args = vec![first];
        while self.at(Token::Comma) {
            self.advance();
            args.push(self.parse_type()?);
        }
        if !self.at(Token::Gt) {
            return None;
        }
        self.advance();
        if !self.count_type_node() {
            return None;
        }
        self.parse_type_array_suffixes(Type::Named { name, args })
    }

    /// Zero or more `[]` suffixes on an already-parsed base type (§11).
    /// `None` when the suffix run would exceed the statement's type budget.
    fn parse_type_array_suffixes(&mut self, ty: Type) -> Option<Type> {
        let mut ty = ty;
        while self.at(Token::LBracket)
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| t.token),
                Some(Token::RBracket)
            )
        {
            self.advance();
            self.advance();
            if !self.count_type_node() {
                return None;
            }
            ty = Type::Array(Box::new(ty));
        }
        Some(ty)
    }

    /// A type declaration led by a scalar keyword (or `void`): the full
    /// declared type (with `[]` suffixes), the declared name, then either a
    /// function (followed by `(`) or a variable (followed by `=`, a newline,
    /// or end of file). `name_kind` selects the diagnostic for a missing
    /// name, preserving the historical split between the `void`-led
    /// function path and the variable path.
    fn parse_type_declaration(&mut self, type_token: SpannedToken<'src>, name_kind: &str) -> Stmt {
        let mut ty = Self::parse_type_from_token(&type_token.token).unwrap_or(Type::Infer);
        // Suffixes past the budget leave the base type in place; the
        // declaration keeps parsing and the checker reports the leftovers.
        if let Some(suffixed) = self.parse_type_array_suffixes(ty.clone()) {
            ty = suffixed;
        }

        let name = match *self.peek() {
            SpannedToken {
                token: Token::Ident(name),
                span,
            } => {
                self.advance();
                (name.to_string(), span)
            }
            other => {
                let end = self.skip_to_statement_end(other.span.end);
                let error = self.record(
                    format!("expected {name_kind}, but found {}", other.token.describe()),
                    other.span,
                );
                return Stmt {
                    span: Span::new(type_token.span.start, end),
                    kind: StmtKind::Invalid { error },
                };
            }
        };

        match self.peek().token {
            Token::LParen => self.parse_function_declaration_rest(type_token.span, ty, name),
            Token::Assign | Token::Newline | Token::Eof => {
                self.parse_variable_declaration_rest(type_token.span, ty, name)
            }
            other => {
                let span = self.peek().span;
                let end = self.skip_to_statement_end(span.end);
                let error = self.record(
                    format!("expected `=`, but found {}", other.describe()),
                    span,
                );
                Stmt {
                    span: Span::new(type_token.span.start, end),
                    kind: StmtKind::Invalid { error },
                }
            }
        }
    }

    /// The function declaration after its return type and name are parsed:
    /// the parameter list, then the braced body. `type_span` covers the
    /// declared return type's first token (for the `infer` diagnostic).
    fn parse_function_declaration_rest(
        &mut self,
        type_span: Span,
        return_ty: Type,
        (name, _name_span): (String, Span),
    ) -> Stmt {
        // §2.16: `infer` marks local declarations only; a return type must
        // be explicit. The error is recorded while the declaration keeps
        // parsing so tooling still sees the function.
        if return_ty == Type::Infer {
            self.record("`infer` is only valid for local declarations", type_span);
        }

        let start = type_span.start;

        let (params, params_ok) = self.parse_parameter_list();
        if !params_ok {
            let end = self.skip_to_statement_end(self.peek().span.end);
            let error = ErrorId(self.errors.len().saturating_sub(1));
            return Stmt {
                span: Span::new(start, end),
                kind: StmtKind::Invalid { error },
            };
        }

        if !self.at(Token::LBrace) {
            let other = *self.peek();
            let end = self.recover_to_next_statement(other.span.end);
            let error = self.record(
                format!("expected `{{`, but found {}", other.token.describe()),
                other.span,
            );
            return Stmt {
                span: Span::new(start, end),
                kind: StmtKind::Invalid { error },
            };
        }
        let open_brace = self.advance().span.start;

        let body = self.parse_block_body(open_brace);
        Stmt::new(
            StmtKind::FuncDecl {
                name,
                params,
                return_ty,
                body,
            },
            Span::new(start, self.tokens[self.pos - 1].span.end),
        )
    }

    fn parse_parameter_list(&mut self) -> (Vec<Param>, bool) {
        let mut params = Vec::new();

        if !self.at(Token::LParen) {
            let other = *self.peek();
            let error = self.record(
                format!("expected `(`, but found {}", other.token.describe()),
                other.span,
            );
            let _ = error;
            return (params, false);
        }
        self.advance();

        if self.at(Token::RParen) {
            self.advance();
            return (params, true);
        }

        loop {
            let type_token = *self.peek();
            let Some(ty) = self.parse_type() else {
                let _ = self.record(
                    format!(
                        "expected a parameter type, but found {}",
                        type_token.token.describe()
                    ),
                    type_token.span,
                );
                return (params, false);
            };
            // §1: parameters are explicitly typed — `infer` (§2.16, local
            // declarations only) and `void` (§2.4, no value) are placement
            // errors. The parameter is kept so the declaration keeps parsing.
            match type_token.token {
                Token::KwInfer => {
                    let _ = self.record(
                        "`infer` is only valid for local declarations",
                        type_token.span,
                    );
                }
                Token::KwVoid => {
                    let _ = self.record(
                        "`void` is only valid as a function return type",
                        type_token.span,
                    );
                }
                _ => {}
            }

            let name = match *self.peek() {
                SpannedToken {
                    token: Token::Ident(name),
                    ..
                } => {
                    self.advance();
                    name.to_string()
                }
                other => {
                    let _ = self.record(
                        format!(
                            "expected a parameter name, but found {}",
                            other.token.describe()
                        ),
                        other.span,
                    );
                    return (params, false);
                }
            };

            params.push(Param { ty, name });

            if self.at(Token::Comma) {
                self.advance();
                continue;
            }
            if self.at(Token::RParen) {
                self.advance();
                return (params, true);
            }
            let other = *self.peek();
            let _ = self.record(
                format!("expected `,` or `)`, but found {}", other.token.describe()),
                other.span,
            );
            return (params, false);
        }
    }

    /// Parses the statements of a block whose opening `{` (at byte
    /// `open_brace`) has already been consumed. The block's span covers from
    /// that `{` through the closing `}` — or to the end of file when the
    /// closing brace is missing.
    fn parse_block_body(&mut self, open_brace: usize) -> Block {
        let mut stmts = Vec::new();

        loop {
            self.skip_newlines();
            if self.at(Token::RBrace) {
                let closing = self.advance();
                return Block {
                    span: Span::new(open_brace, closing.span.end),
                    stmts,
                };
            }
            if self.at_eof() {
                let eof_span = self.eof_span();
                let _ = self.record("expected `}` before end of file", eof_span);
                return Block {
                    span: Span::new(open_brace, eof_span.end),
                    stmts,
                };
            }

            let inner = self.parse_statement();
            stmts.push(inner);

            match self.peek().token {
                Token::Newline => {
                    self.pos += 1;
                }
                Token::RBrace | Token::Eof => {}
                _ => {
                    let token = self.peek().token;
                    let span = self.peek().span;
                    self.errors
                        .push(Self::expected("end of statement", &token, span));
                    self.skip_to_next_statement();
                }
            }
        }
    }

    fn parse_if_statement(&mut self, if_token: SpannedToken<'src>) -> Stmt {
        let (cond, cond_ok) = self.parse_condition_parens();
        if !cond_ok {
            let error = ErrorId(self.errors.len().saturating_sub(1));
            let end = self.recover_to_next_statement(self.peek().span.end);
            return Stmt {
                span: Span::new(if_token.span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }

        if !self.at(Token::LBrace) {
            let other = *self.peek();
            let end = self.skip_to_statement_end(other.span.end);
            let error = self.record(
                format!("expected `{{`, but found {}", other.token.describe()),
                other.span,
            );
            return Stmt {
                span: Span::new(if_token.span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }
        let open_brace = self.advance().span.start;
        let then_branch = self.parse_block_body(open_brace);

        let else_branch = if self.at(Token::KwElse) {
            let else_token = self.advance();
            if self.at(Token::KwIf) {
                let else_if = self.advance();
                Some(Box::new(self.parse_if_statement(else_if)))
            } else if self.at(Token::LBrace) {
                let open_brace = self.advance().span.start;
                let block = self.parse_block_body(open_brace);
                // The wrapping statement covers the `else` keyword through
                // the block's closing brace.
                let span = Span::new(else_token.span.start, block.span.end);
                Some(Box::new(Stmt::new(StmtKind::Block(block), span)))
            } else {
                let other = *self.peek();
                let end = self.skip_to_statement_end(other.span.end);
                let error = self.record(
                    format!(
                        "expected `{{` or `if` after `else`, but found {}",
                        other.token.describe()
                    ),
                    other.span,
                );
                Some(Box::new(Stmt {
                    span: Span::new(other.span.start, end),
                    kind: StmtKind::Invalid { error },
                }))
            }
        } else {
            None
        };

        let end = self.tokens[self.pos - 1].span.end;
        Stmt::new(
            StmtKind::If {
                cond,
                then_branch,
                else_branch,
            },
            Span::new(if_token.span.start, end),
        )
    }

    fn parse_while_statement(&mut self, while_token: SpannedToken<'src>) -> Stmt {
        let (cond, cond_ok) = self.parse_condition_parens();
        if !cond_ok {
            let error = ErrorId(self.errors.len().saturating_sub(1));
            let end = self.recover_to_next_statement(self.peek().span.end);
            return Stmt {
                span: Span::new(while_token.span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }

        if !self.at(Token::LBrace) {
            let other = *self.peek();
            let end = self.skip_to_statement_end(other.span.end);
            let error = self.record(
                format!("expected `{{`, but found {}", other.token.describe()),
                other.span,
            );
            return Stmt {
                span: Span::new(while_token.span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }
        let open_brace = self.advance().span.start;

        let body = self.parse_block_body(open_brace);
        let end = self.tokens[self.pos - 1].span.end;
        Stmt::new(
            StmtKind::While { cond, body },
            Span::new(while_token.span.start, end),
        )
    }

    fn parse_condition_parens(&mut self) -> (Expr, bool) {
        if !self.at(Token::LParen) {
            let other = *self.peek();
            let error = self.record(
                format!("expected `(`, but found {}", other.token.describe()),
                other.span,
            );
            let _ = error;
            let missing = Expr {
                span: Span::missing(other.span.start),
                kind: ExprKind::Invalid {
                    error: ErrorId(self.errors.len() - 1),
                },
            };
            return (missing, false);
        }
        self.advance();
        self.skip_newlines();

        match self.parse_expression() {
            Ok(expr) => {
                self.skip_newlines();
                if self.at(Token::RParen) {
                    self.advance();
                    (expr, true)
                } else {
                    let other = *self.peek();
                    let _ = self.record(
                        format!("expected `)`, but found {}", other.token.describe()),
                        other.span,
                    );
                    (expr, false)
                }
            }
            Err(diagnostic) => {
                self.errors.push(diagnostic);
                let missing = Expr {
                    span: Span::missing(self.peek().span.start),
                    kind: ExprKind::Invalid {
                        error: ErrorId(self.errors.len() - 1),
                    },
                };
                (missing, false)
            }
        }
    }

    fn parse_return_statement(&mut self, return_token: SpannedToken<'src>) -> Stmt {
        let value = if matches!(
            self.peek().token,
            Token::Newline | Token::RBrace | Token::Eof
        ) {
            None
        } else {
            Some(self.parse_recovered_expression())
        };

        let end = self.tokens[self.pos - 1].span.end;
        Stmt::new(
            StmtKind::Return { value },
            Span::new(return_token.span.start, end),
        )
    }

    fn parse_void_misuse(&mut self, void_token: SpannedToken<'src>) -> Stmt {
        let end = self.skip_to_statement_end(void_token.span.end);
        let error = self.record(
            "`void` is only valid as a function return type",
            void_token.span,
        );
        Stmt {
            span: Span::new(void_token.span.start, end),
            kind: StmtKind::Invalid { error },
        }
    }

    /// The optional `< A, B >` type-parameter list of a struct/enum
    /// declaration (§2.9): `Err` only when a list is present but broken.
    fn parse_decl_type_params(&mut self) -> Result<Vec<String>, Diagnostic> {
        if !self.at(Token::Lt) {
            return Ok(Vec::new());
        }
        self.parse_type_params()
    }

    /// Parses an optional `< A, B, ... >` type-parameter list for struct and
    /// enum declarations (§2.9). Starts at `Lt`.
    fn parse_type_params(&mut self) -> Result<Vec<String>, Diagnostic> {
        let open = *self.peek();
        self.advance();
        let mut params = Vec::new();
        loop {
            let token = *self.peek();
            let Token::Ident(name) = token.token else {
                return Err(Self::expected(
                    "a type parameter name",
                    &token.token,
                    token.span,
                ));
            };
            self.advance();
            params.push(name.to_string());
            if self.at(Token::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        let close = *self.peek();
        if close.token != Token::Gt {
            return Err(Self::expected(
                "`>` after type parameters",
                &close.token,
                close.span,
            ));
        }
        self.advance();
        let _ = open;
        Ok(params)
    }

    /// Parses a struct declaration (§2.6, §2.9): `struct Name < params >? {`
    /// then newline-delimited `Type name` fields. The declaration survives
    /// broken fields: each damaged field is reported and skipped, and the
    /// remaining fields keep their positions for tooling.
    fn parse_struct_declaration(&mut self, struct_token: SpannedToken<'src>) -> Stmt {
        let name = match *self.peek() {
            SpannedToken {
                token: Token::Ident(name),
                ..
            } => {
                self.advance();
                name.to_string()
            }
            other => {
                let end = self.skip_to_statement_end(other.span.end);
                let error = self.record(
                    format!(
                        "expected a struct name, but found {}",
                        other.token.describe()
                    ),
                    other.span,
                );
                return Stmt {
                    span: Span::new(struct_token.span.start, end),
                    kind: StmtKind::Invalid { error },
                };
            }
        };

        let type_params = self.parse_decl_type_params();
        if let Err(recovered) = &type_params {
            // A broken parameter list invalidates the whole declaration.
            let _ = recovered;
            return self.invalid_declaration(struct_token);
        }
        let type_params = type_params.unwrap();

        if !self.at(Token::LBrace) {
            let other = *self.peek();
            let end = self.recover_to_next_statement(other.span.end);
            let error = self.record(
                format!("expected `{{`, but found {}", other.token.describe()),
                other.span,
            );
            return Stmt {
                span: Span::new(struct_token.span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }
        let open_brace_span = self.advance().span;

        let mut fields = Vec::new();
        let end = self.parse_decl_member_list(
            &mut |parser| {
                // One `Type name` field (§2.6); no commas.
                parser.parse_decl_member()
            },
            &mut fields,
            open_brace_span,
        );

        Stmt::new(
            StmtKind::StructDecl {
                name,
                type_params,
                fields,
            },
            Span::new(struct_token.span.start, end),
        )
    }

    /// Parses an enum declaration (§2.7, §2.9): `enum Name < params >? {`
    /// then newline-delimited `Variant(Type name, ...)` declarations.
    fn parse_enum_declaration(&mut self, enum_token: SpannedToken<'src>) -> Stmt {
        let name = match *self.peek() {
            SpannedToken {
                token: Token::Ident(name),
                ..
            } => {
                self.advance();
                name.to_string()
            }
            other => {
                let end = self.skip_to_statement_end(other.span.end);
                let error = self.record(
                    format!(
                        "expected an enum name, but found {}",
                        other.token.describe()
                    ),
                    other.span,
                );
                return Stmt {
                    span: Span::new(enum_token.span.start, end),
                    kind: StmtKind::Invalid { error },
                };
            }
        };

        let type_params = self.parse_decl_type_params();
        if type_params.is_err() {
            return self.invalid_declaration(enum_token);
        }
        let type_params = type_params.unwrap();

        if !self.at(Token::LBrace) {
            let other = *self.peek();
            let end = self.recover_to_next_statement(other.span.end);
            let error = self.record(
                format!("expected `{{`, but found {}", other.token.describe()),
                other.span,
            );
            return Stmt {
                span: Span::new(enum_token.span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }
        let open_brace_span = self.advance().span;

        let mut variants = Vec::new();
        let end = self.parse_decl_member_list(
            &mut |parser| parser.parse_enum_variant(),
            &mut variants,
            open_brace_span,
        );

        Stmt::new(
            StmtKind::EnumDecl {
                name,
                type_params,
                variants,
            },
            Span::new(enum_token.span.start, end),
        )
    }

    /// Parses an import statement (§2.3, §10.3): `import` followed by a
    /// dot-separated path of identifier segments, e.g.
    /// `import self.gamemode.rules` or `import engine.graphics`. The path
    /// needs at least two segments — a bare root names neither a module of
    /// the mod tree nor a capability namespace. Resolution (does
    /// `self.gamemode.rules` exist?) belongs to the mod loader, not the
    /// parser; here only the shape is judged.
    fn parse_import_statement(&mut self, import_token: SpannedToken<'src>) -> Stmt {
        let start = import_token.span.start;

        // The path: one identifier segment, then `.segment` extensions. A
        // keyword segment (`import self.match`) does not lex as an
        // identifier and is rejected like any other non-segment.
        let head = *self.peek();
        let (mut path, mut end) = match head.token {
            Token::Ident(name) => {
                self.advance();
                (vec![name.to_string()], head.span.end)
            }
            other => {
                let end = self.skip_to_statement_end(head.span.end);
                let error = self.record(
                    format!(
                        "expected a module path after `import`, but found {}",
                        other.describe()
                    ),
                    head.span,
                );
                return Stmt {
                    span: Span::new(start, end),
                    kind: StmtKind::Invalid { error },
                };
            }
        };

        while self.at(Token::Dot) {
            let dot_span = self.peek().span;
            let next = self.tokens.get(self.pos + 1).map(|t| (t.token, t.span));
            match next {
                Some((Token::Ident(_), _segment_span)) => {
                    self.advance(); // dot
                    let segment = self.advance();
                    end = segment.span.end;
                    if let Token::Ident(name) = segment.token {
                        path.push(name.to_string());
                    }
                }
                Some((other, other_span)) => {
                    let end = self.skip_to_statement_end(other_span.end);
                    let error = self.record(
                        format!(
                            "expected a path segment after `.`, but found {}",
                            other.describe()
                        ),
                        other_span,
                    );
                    return Stmt {
                        span: Span::new(start, end),
                        kind: StmtKind::Invalid { error },
                    };
                }
                None => {
                    // The dot is the last token before end of file.
                    let error = self.record("expected a path segment after `.`", dot_span);
                    return Stmt {
                        span: Span::new(start, dot_span.end),
                        kind: StmtKind::Invalid { error },
                    };
                }
            }
        }

        if path.len() < 2 {
            let error = self.record(
                "an import path needs at least two segments, like `self.rules` \
                 or `engine.graphics`",
                Span::new(start, end),
            );
            return Stmt {
                span: Span::new(start, end),
                kind: StmtKind::Invalid { error },
            };
        }

        Stmt {
            span: Span::new(start, end),
            kind: StmtKind::Import { path },
        }
    }

    /// An impl block (§10.4): `impl target.path { members }`. The target is
    /// a dotted identifier path — a locally declared struct/enum name or a
    /// host-style namespace (`engine.gamemode`). Members are function
    /// declarations (§2.11 shape); any other statement inside the braces is
    /// reported and skipped. Recovery mirrors [`Self::parse_block_body`]: a
    /// damaged member is reported and the parser resynchronizes at the next
    /// line boundary, so sibling members survive.
    fn parse_impl_declaration(&mut self, impl_token: SpannedToken<'src>) -> Stmt {
        let start = impl_token.span.start;

        // The target path: one identifier, then `.segment` extensions.
        let head = *self.peek();
        let mut target = match head.token {
            Token::Ident(name) => {
                self.advance();
                vec![name.to_string()]
            }
            other => {
                let end = self.skip_to_statement_end(head.span.end);
                let error = self.record(
                    format!("expected an impl target, but found {}", other.describe()),
                    head.span,
                );
                return Stmt {
                    span: Span::new(start, end),
                    kind: StmtKind::Invalid { error },
                };
            }
        };
        while self.at(Token::Dot)
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| t.token),
                Some(Token::Ident(_))
            )
        {
            self.advance(); // dot
            if let Token::Ident(segment) = self.advance().token {
                target.push(segment.to_string());
            }
        }

        if !self.at(Token::LBrace) {
            let other = *self.peek();
            let end = self.recover_to_next_statement(other.span.end);
            let error = self.record(
                format!("expected `{{`, but found {}", other.token.describe()),
                other.span,
            );
            return Stmt {
                span: Span::new(start, end),
                kind: StmtKind::Invalid { error },
            };
        }
        let _open_brace = self.advance().span.start;

        let mut members: Vec<Stmt> = Vec::new();
        let end;
        loop {
            self.skip_newlines();
            if self.at(Token::RBrace) {
                let closing = self.advance();
                end = closing.span.end;
                break;
            }
            if self.at_eof() {
                let eof_span = self.eof_span();
                let _ = self.record("expected `}` before end of file", eof_span);
                end = eof_span.end;
                break;
            }

            let member = self.parse_statement();
            if matches!(&member.kind, StmtKind::FuncDecl { .. }) {
                members.push(member);
            } else if !matches!(member.kind, StmtKind::Invalid { .. }) {
                // A recognizable statement that is not a function
                // declaration: impl members are functions (§10.4). Invalid
                // members were already reported by `parse_statement`.
                let span = member.span;
                let _ = self.record("impl members must be function declarations", span);
            }

            match self.peek().token {
                Token::Newline => self.pos += 1,
                Token::RBrace | Token::Eof => {}
                _ => {
                    let token = self.peek().token;
                    let span = self.peek().span;
                    self.errors
                        .push(Self::expected("end of member", &token, span));
                    self.skip_to_next_statement();
                }
            }
        }

        Stmt::new(
            StmtKind::ImplDecl { target, members },
            Span::new(start, end),
        )
    }

    /// A declaration made unrecoverable by its parameter list: one Invalid
    /// statement covering the skipped region.
    fn invalid_declaration(&mut self, head: SpannedToken<'src>) -> Stmt {
        let end = self.skip_to_statement_end(self.peek().span.end);
        let error = ErrorId(self.errors.len().saturating_sub(1));
        Stmt {
            span: Span::new(head.span.start, end),
            kind: StmtKind::Invalid { error },
        }
    }

    /// The shared newline-delimited body of struct/enum declarations: each
    /// line is one member parsed by `member`; damaged members are reported
    /// and skipped, and commas between members are rejected (§2.6: fields
    /// are newline-delimited, no commas). A declaration-starting token
    /// (`struct`/`enum`/`impl` or a function return-type keyword heading a
    /// `Type name(` signature) closes the broken declaration at that point:
    /// the partial declaration is kept and the token re-dispatches as the
    /// next statement. Returns the body end offset.
    fn parse_decl_member_list<T>(
        &mut self,
        member: &mut dyn FnMut(&mut Self) -> Option<T>,
        out: &mut Vec<T>,
        open_brace: Span,
    ) -> usize {
        loop {
            self.skip_newlines();
            if self.at(Token::RBrace) {
                let closing = self.advance();
                return closing.span.end;
            }
            if self.at_eof() {
                let eof_span = self.eof_span();
                let _ = self.record("expected `}` before end of file", eof_span);
                return eof_span.end;
            }
            if self.at_decl_recovery_boundary() {
                let end = self.peek().span.start;
                let _ = self.record("unbalanced opening brace", open_brace);
                return end;
            }

            if let Some(value) = member(self) {
                out.push(value);
                // A successful member ends at a newline, the closing brace,
                // or end of file; commas are rejected (§2.6).
                match self.peek().token {
                    Token::Newline => {
                        self.pos += 1;
                    }
                    Token::RBrace | Token::Eof => {}
                    Token::Comma => {
                        let other = *self.peek();
                        let _ = self.record(
                            "declaration members are newline-delimited; remove the comma",
                            other.span,
                        );
                        self.advance();
                    }
                    other => {
                        let token = other;
                        let span = self.peek().span;
                        self.errors
                            .push(Self::expected("end of member", &token, span));
                        self.recover_member_position();
                    }
                }
            }
            // A failed member already reported and positioned itself at the
            // next boundary; nothing more to do here.
        }
    }

    /// True when the cursor sits on a token that starts a new declaration
    /// rather than a struct/enum member: `struct`/`enum`/`impl`, or a
    /// function return-type keyword (`int`/`float`/`str`/`bool`/`infer`/
    /// `void`) heading a `Type name(` signature.
    fn at_decl_recovery_boundary(&self) -> bool {
        match self.peek().token {
            Token::KwStruct | Token::KwEnum | Token::KwImpl => true,
            token if token.is_type_keyword() || token == Token::KwVoid => {
                let next = self.tokens.get(self.pos + 1).map(|t| t.token);
                let after = self.tokens.get(self.pos + 2).map(|t| t.token);
                matches!(next, Some(Token::Ident(_))) && after == Some(Token::LParen)
            }
            _ => false,
        }
    }

    /// Positions the parser at the next member boundary: the newline (which
    /// is consumed) or the closing brace / end of file (not consumed).
    fn recover_member_position(&mut self) {
        loop {
            let token = self.peek().token;
            if matches!(token, Token::Newline | Token::RBrace | Token::Eof) {
                if token == Token::Newline {
                    self.pos += 1;
                }
                return;
            }
            self.advance();
        }
    }

    /// One struct field: `Type name` (§2.6).
    fn parse_decl_member(&mut self) -> Option<FieldDef> {
        let type_token = *self.peek();
        let Some(ty) = self.parse_type() else {
            let _ = self.record(
                format!(
                    "expected a field type, but found {}",
                    type_token.token.describe()
                ),
                type_token.span,
            );
            self.recover_member_position();
            return None;
        };
        self.record_placement_error(&type_token, "field");
        let name = match *self.peek() {
            SpannedToken {
                token: Token::Ident(name),
                ..
            } => {
                self.advance();
                name.to_string()
            }
            other => {
                let _ = self.record(
                    format!(
                        "expected a field name, but found {}",
                        other.token.describe()
                    ),
                    other.span,
                );
                self.recover_member_position();
                return None;
            }
        };
        Some(FieldDef { ty, name })
    }

    /// One enum variant: `Name(Type name, ...)` (§2.7). The payload list is
    /// comma-separated, matching the declaration grammar.
    fn parse_enum_variant(&mut self) -> Option<VariantDecl> {
        let name = match *self.peek() {
            SpannedToken {
                token: Token::Ident(name),
                ..
            } => {
                self.advance();
                name.to_string()
            }
            other => {
                let _ = self.record(
                    format!(
                        "expected a variant name, but found {}",
                        other.token.describe()
                    ),
                    other.span,
                );
                self.recover_member_position();
                return None;
            }
        };

        if !self.at(Token::LParen) {
            let other = *self.peek();
            let _ = self.record(
                format!(
                    "expected `(` after variant name, but found {}",
                    other.token.describe()
                ),
                other.span,
            );
            self.recover_member_position();
            return None;
        }
        self.advance();

        let mut fields = Vec::new();
        self.skip_newlines();
        if !self.at(Token::RParen) {
            loop {
                self.skip_newlines();
                let type_token = *self.peek();
                let Some(ty) = self.parse_type() else {
                    let _ = self.record(
                        format!(
                            "expected a payload type, but found {}",
                            type_token.token.describe()
                        ),
                        type_token.span,
                    );
                    self.recover_to_variant_close();
                    return None;
                };
                self.record_placement_error(&type_token, "payload");
                let field_name = match *self.peek() {
                    SpannedToken {
                        token: Token::Ident(field_name),
                        ..
                    } => {
                        self.advance();
                        field_name.to_string()
                    }
                    other => {
                        let _ = self.record(
                            format!(
                                "expected a payload name, but found {}",
                                other.token.describe()
                            ),
                            other.span,
                        );
                        self.recover_to_variant_close();
                        return None;
                    }
                };
                fields.push(FieldDef {
                    ty,
                    name: field_name,
                });
                self.skip_newlines();
                if self.at(Token::Comma) {
                    self.advance();
                    continue;
                }
                if self.at(Token::RParen) {
                    break;
                }
                let other = *self.peek();
                let _ = self.record(
                    format!("expected `,` or `)`, but found {}", other.token.describe()),
                    other.span,
                );
                self.recover_to_variant_close();
                return None;
            }
        }
        self.skip_newlines();
        if !self.at(Token::RParen) {
            let other = *self.peek();
            let _ = self.record(
                format!("expected `)`, but found {}", other.token.describe()),
                other.span,
            );
            self.recover_member_position();
            return None;
        }
        self.advance();
        Some(VariantDecl { name, fields })
    }

    /// After a broken payload: skip to the closing `)` of the variant (and
    /// consume it) so the member list can continue at the next line.
    fn recover_to_variant_close(&mut self) {
        loop {
            let token = self.peek().token;
            if matches!(
                token,
                Token::RParen | Token::RBrace | Token::Eof | Token::Newline
            ) {
                if token == Token::RParen {
                    self.advance();
                }
                return;
            }
            self.advance();
        }
    }

    /// Records the placement errors for `infer`/`void` in member types
    /// (§2.16, §2.4) with the same messages as parameters.
    fn record_placement_error(&mut self, type_token: &SpannedToken<'src>, what: &str) {
        match type_token.token {
            Token::KwInfer => {
                let _ = self.record(
                    "`infer` is only valid for local declarations",
                    type_token.span,
                );
            }
            Token::KwVoid => {
                let _ = self.record(
                    "`void` is only valid as a function return type",
                    type_token.span,
                );
            }
            _ => {
                let _ = what;
            }
        }
    }

    /// The variable declaration after its type and name are parsed (§2.10).
    fn parse_variable_declaration_rest(
        &mut self,
        type_span: Span,
        ty: Type,
        (name, _name_span): (String, Span),
    ) -> Stmt {
        let start = type_span.start;

        // A declaration whose header simply ends (`int i` at a newline or end
        // of file) keeps the declared variable for tooling, with a zero-width
        // invalid initializer marking the missing `= value`.
        match self.peek().token {
            Token::Newline => {
                let span = self.peek().span;
                let expr = self.missing_expression(
                    "expected `=`, but found end of statement",
                    span,
                    span.start,
                );
                let end = expr.span.end;
                return Stmt::new(StmtKind::VarDecl { ty, name, expr }, Span::new(start, end));
            }
            Token::Eof => {
                let span = self.eof_span();
                let expr = self.missing_expression(
                    "expected `=`, but found end of file",
                    span,
                    span.start,
                );
                let end = expr.span.end;
                return Stmt::new(StmtKind::VarDecl { ty, name, expr }, Span::new(start, end));
            }
            Token::Assign => {
                self.advance();
            }
            _ => {
                let other = *self.peek();
                let end = self.skip_to_statement_end(other.span.end);
                let error = self.record(
                    format!("expected `=`, but found {}", other.token.describe()),
                    other.span,
                );
                return Stmt {
                    span: Span::new(start, end),
                    kind: StmtKind::Invalid { error },
                };
            }
        }

        let expr = self.parse_recovered_expression();
        let end = expr.span.end;
        Stmt::new(StmtKind::VarDecl { ty, name, expr }, Span::new(start, end))
    }

    /// A statement led by an identifier: either a declaration led by a
    /// named type (`vec2 pos = ...`, `pair<int, str> p = ...`, `option<int>
    /// findEven(...)` — §2.6, §2.9, §2.11), or an expression statement /
    /// assignment. The type parse is speculative: it rolls back cleanly
    /// when the tokens do not form a declaration header.
    fn parse_ident_led_statement(&mut self, first: SpannedToken<'src>) -> Stmt {
        let save_pos = self.pos;
        let save_errors = self.errors.len();

        if let Some(ty) = self.parse_type_from_ident(first)
            && let Token::Ident(_) = self.peek().token
        {
            let (decl_name, name_span) = match *self.peek() {
                SpannedToken {
                    token: Token::Ident(name),
                    span,
                } => (name.to_string(), span),
                _ => unreachable!(),
            };
            match self.tokens.get(self.pos + 1).map(|t| t.token) {
                Some(Token::LParen) => {
                    self.advance();
                    return self.parse_function_declaration_rest(
                        first.span,
                        ty,
                        (decl_name, name_span),
                    );
                }
                Some(Token::Assign) | Some(Token::Newline) | Some(Token::Eof) | None => {
                    self.advance();
                    return self.parse_variable_declaration_rest(
                        first.span,
                        ty,
                        (decl_name, name_span),
                    );
                }
                _ => {}
            }
        }

        // Not a declaration: rewind the speculation and the first token,
        // then parse an expression statement from the identifier.
        self.pos = save_pos;
        self.errors.truncate(save_errors);
        self.pos -= 1;
        self.parse_expression_statement()
    }

    /// The type whose base name is the already-consumed `first` identifier:
    /// generic arguments, an array suffix, or a plain named type. Consumes
    /// nothing on failure.
    fn parse_type_from_ident(&mut self, first: SpannedToken<'src>) -> Option<Type> {
        let Token::Ident(name) = first.token else {
            return None;
        };
        if self.at(Token::Lt) {
            self.advance();
            return self.parse_type_generic_tail(name.to_string());
        }
        if self.at(Token::LBracket)
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| t.token),
                Some(Token::RBracket)
            )
        {
            self.advance();
            self.advance();
            if !self.count_type_node() {
                return None;
            }
            let array = Type::Array(Box::new(Type::Named {
                name: name.to_string(),
                args: Vec::new(),
            }));
            return self.parse_type_array_suffixes(array);
        }
        Some(Type::Named {
            name: name.to_string(),
            args: Vec::new(),
        })
    }

    /// An assignment or a bare call statement led by an expression: parses
    /// the full expression, then requires an assignment operator (§2.10,
    /// §A.7 — targets are variable/field/index chains) or accepts the
    /// statement as a call (§2.11).
    fn parse_expression_statement(&mut self) -> Stmt {
        // The statement's first token sits at the current position; the
        // expression parse consumes it (and everything it spans).
        let start = self.peek().span;

        let expr = match self.parse_expression() {
            Ok(expr) => expr,
            Err(diagnostic) => {
                self.errors.push(diagnostic);
                let end = self.skip_to_statement_end(self.peek().span.end);
                return Stmt {
                    span: Span::new(start.start, end),
                    kind: StmtKind::Invalid {
                        error: ErrorId(self.errors.len() - 1),
                    },
                };
            }
        };

        // Assignment (plain or compound) to an lvalue chain (§2.10, §A.7).
        let operator = *self.peek();
        let compound = match operator.token {
            Token::Assign => None,
            Token::AddAssign => Some(CompoundOp::Add),
            Token::SubAssign => Some(CompoundOp::Sub),
            Token::MulAssign => Some(CompoundOp::Mul),
            Token::DivAssign => Some(CompoundOp::Div),
            Token::RemAssign => Some(CompoundOp::Rem),
            _ => None,
        };
        if compound.is_some() || operator.token == Token::Assign {
            self.advance();
            let rhs = self.parse_recovered_expression();
            let kind = match expr_to_lvalue(&expr) {
                Some(target) => match compound {
                    None => StmtKind::Assign {
                        target,
                        expr: rhs.clone(),
                    },
                    Some(op) => StmtKind::CompoundAssign {
                        target,
                        op,
                        expr: rhs.clone(),
                    },
                },
                None => {
                    // Not an assignable expression: report and keep the
                    // statement boundary intact.
                    let error = self.record("invalid assignment target", expr.span);
                    return Stmt {
                        span: Span::new(start.start, rhs.span.end),
                        kind: StmtKind::Invalid { error },
                    };
                }
            };
            return Stmt::new(kind, Span::new(start.start, rhs.span.end));
        }

        // Otherwise the expression statement must be a call or a
        // construction (§2.11); anything else is a bare fragment.
        let is_callable = matches!(
            &expr.kind,
            ExprKind::Call { .. } | ExprKind::VariantCall { .. } | ExprKind::PathCall { .. }
        );
        if is_callable {
            let end = expr.span.end;
            return Stmt::new(StmtKind::Expression { expr }, Span::new(start.start, end));
        }

        // A bare fragment: one diagnostic, then the statement boundary.
        let end = self.skip_to_statement_end(operator.span.end);
        let error = self.record(
            format!(
                "expected an assignment operator, but found {}",
                operator.token.describe()
            ),
            operator.span,
        );
        Stmt {
            span: Span::new(start.start, end),
            kind: StmtKind::Invalid { error },
        }
    }

    /// A `match` statement (§2.15): `match (scrutinee) { arms }` where every
    /// arm is `pattern => { block }`. The statement survives broken arms:
    /// each damaged arm is reported and skipped at the next line boundary.
    fn parse_match_statement(&mut self, match_token: SpannedToken<'src>) -> Stmt {
        let (scrutinee, ok) = self.parse_condition_parens();
        if !ok {
            let error = ErrorId(self.errors.len().saturating_sub(1));
            let end = self.recover_to_next_statement(self.peek().span.end);
            return Stmt {
                span: Span::new(match_token.span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }

        if !self.at(Token::LBrace) {
            let other = *self.peek();
            let end = self.recover_to_next_statement(other.span.end);
            let error = self.record(
                format!("expected `{{`, but found {}", other.token.describe()),
                other.span,
            );
            return Stmt {
                span: Span::new(match_token.span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }
        self.advance(); // the match body’s `{`

        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(Token::RBrace) {
                let closing = self.advance();
                return Stmt::new(
                    StmtKind::Match { scrutinee, arms },
                    Span::new(match_token.span.start, closing.span.end),
                );
            }
            if self.at_eof() {
                let eof_span = self.eof_span();
                let _ = self.record("expected `}` before end of file", eof_span);
                return Stmt::new(
                    StmtKind::Match { scrutinee, arms },
                    Span::new(match_token.span.start, eof_span.end),
                );
            }

            let pattern = match self.parse_pattern() {
                Ok(pattern) => pattern,
                Err(diagnostic) => {
                    self.errors.push(diagnostic);
                    self.skip_arm_to_boundary();
                    continue;
                }
            };

            if !self.at(Token::FatArrow) {
                let other = *self.peek();
                let _ = self.record(
                    format!(
                        "expected `=>` after pattern, but found {}",
                        other.token.describe()
                    ),
                    other.span,
                );
                self.skip_arm_to_boundary();
                continue;
            }
            self.advance();

            if !self.at(Token::LBrace) {
                let other = *self.peek();
                let _ = self.record(
                    format!(
                        "expected `{{` for the arm body, but found {}",
                        other.token.describe()
                    ),
                    other.span,
                );
                self.skip_arm_to_boundary();
                continue;
            }
            let arm_open = self.advance().span.start;
            let body = self.parse_block_body(arm_open);
            arms.push(MatchArmStmt { pattern, body });

            match self.peek().token {
                Token::Newline => {
                    self.pos += 1;
                }
                Token::RBrace | Token::Eof => {}
                other => {
                    let _ = self.record(
                        format!(
                            "expected a newline or `}}` between match arms, but found {}",
                            other.describe()
                        ),
                        self.peek().span,
                    );
                    self.skip_arm_to_boundary();
                }
            }
        }
    }

    /// A `match` expression (§2.15): `match (scrutinee) { arms }` where
    /// every arm yields an expression.
    fn parse_match_expression(
        &mut self,
        match_token: SpannedToken<'src>,
    ) -> Result<Expr, Diagnostic> {
        let scrutinee = self.parse_paren_group_expr()?;
        if !self.at(Token::LBrace) {
            let other = *self.peek();
            return Err(Self::expected("`{`", &other.token, other.span));
        }
        self.advance();

        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(Token::RBrace) {
                let closing = self.advance();
                return Ok(Expr::new(
                    ExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        arms,
                    },
                    Span::new(match_token.span.start, closing.span.end),
                ));
            }
            if self.at_eof() {
                let eof_span = self.eof_span();
                return Err(Diagnostic::parse(
                    "expected `}` before end of file",
                    eof_span,
                ));
            }

            let pattern = self.parse_pattern()?;
            if !self.at(Token::FatArrow) {
                let other = *self.peek();
                return Err(Self::expected(
                    "`=>` after pattern",
                    &other.token,
                    other.span,
                ));
            }
            self.advance();
            self.skip_newlines();
            let body = self.parse_expression()?;
            arms.push(MatchArmExpr { pattern, body });

            match self.peek().token {
                Token::Newline => {
                    self.pos += 1;
                }
                Token::RBrace | Token::Eof => {}
                other => {
                    return Err(Self::expected(
                        "a newline or `}` between match arms",
                        &other,
                        self.peek().span,
                    ));
                }
            }
        }
    }

    /// `( expr )` in expression context: a non-recording variant of the
    /// statement-level condition parse.
    fn parse_paren_group_expr(&mut self) -> Result<Expr, Diagnostic> {
        if !self.at(Token::LParen) {
            let other = *self.peek();
            return Err(Self::expected("`(`", &other.token, other.span));
        }
        self.advance();
        self.skip_newlines();
        let expr = self.parse_expression()?;
        self.skip_newlines();
        if !self.at(Token::RParen) {
            let other = *self.peek();
            return Err(Self::expected("`)`", &other.token, other.span));
        }
        self.advance();
        Ok(expr)
    }

    /// After a broken arm: skip to the newline (consumed), the closing brace,
    /// or end of file (both left in place).
    fn skip_arm_to_boundary(&mut self) {
        loop {
            let token = self.peek().token;
            if matches!(token, Token::Newline | Token::RBrace | Token::Eof) {
                if token == Token::Newline {
                    self.pos += 1;
                }
                return;
            }
            self.advance();
        }
    }

    /// One match arm pattern (§2.15): a variant of the scrutinee's enum with
    /// typed payload bindings, or the `_` wildcard.
    fn parse_pattern(&mut self) -> Result<Pattern, Diagnostic> {
        let token = *self.peek();
        let Token::Ident(name) = token.token else {
            return Err(Self::expected("a pattern", &token.token, token.span));
        };
        self.advance();

        if name == "_" {
            if self.at(Token::LParen) {
                let other = *self.peek();
                return Err(Diagnostic::parse(
                    "the wildcard `_` takes no payload",
                    other.span,
                ));
            }
            return Ok(Pattern::Wildcard);
        }

        if !self.at(Token::LParen) {
            let other = *self.peek();
            return Err(Self::expected(
                "`(` after the variant name",
                &other.token,
                other.span,
            ));
        }
        self.advance();

        let mut bindings = Vec::new();
        self.skip_newlines();
        if !self.at(Token::RParen) {
            loop {
                self.skip_newlines();
                let type_token = *self.peek();
                let Some(ty) = self.parse_type() else {
                    return Err(Self::expected(
                        "a payload type",
                        &type_token.token,
                        type_token.span,
                    ));
                };
                let name_tok = *self.peek();
                let Token::Ident(binding) = name_tok.token else {
                    return Err(Self::expected(
                        "a payload name",
                        &name_tok.token,
                        name_tok.span,
                    ));
                };
                self.advance();
                bindings.push(FieldDef {
                    ty,
                    name: binding.to_string(),
                });
                self.skip_newlines();
                if self.at(Token::Comma) {
                    self.advance();
                    continue;
                }
                if self.at(Token::RParen) {
                    break;
                }
                let other = *self.peek();
                return Err(Self::expected("`,` or `)`", &other.token, other.span));
            }
        }
        self.skip_newlines();
        let closing = *self.peek();
        if closing.token != Token::RParen {
            return Err(Self::expected("`)`", &closing.token, closing.span));
        }
        self.advance();
        Ok(Pattern::Variant {
            variant: name.to_string(),
            bindings,
        })
    }

    /// A `for` statement (§2.14): `for (Type name in iterable) { body }`.
    fn parse_for_statement(&mut self, for_token: SpannedToken<'src>) -> Stmt {
        let recover = |parser: &mut Self| -> Stmt {
            let end = parser.skip_to_statement_end(parser.peek().span.end);
            let error = ErrorId(parser.errors.len().saturating_sub(1));
            Stmt {
                span: Span::new(for_token.span.start, end),
                kind: StmtKind::Invalid { error },
            }
        };

        if !self.at(Token::LParen) {
            let other = *self.peek();
            let _ = self.record(
                format!("expected `(`, but found {}", other.token.describe()),
                other.span,
            );
            return recover(self);
        }
        self.advance();
        self.skip_newlines();

        let type_token = *self.peek();
        let Some(elem_ty) = self.parse_type() else {
            let _ = self.record(
                format!(
                    "expected an element type, but found {}",
                    type_token.token.describe()
                ),
                type_token.span,
            );
            return recover(self);
        };
        let elem_name = match *self.peek() {
            SpannedToken {
                token: Token::Ident(name),
                ..
            } => {
                self.advance();
                name.to_string()
            }
            other => {
                let _ = self.record(
                    format!(
                        "expected an element name, but found {}",
                        other.token.describe()
                    ),
                    other.span,
                );
                return recover(self);
            }
        };

        if !self.at(Token::KwIn) {
            let other = *self.peek();
            let _ = self.record(
                format!("expected `in`, but found {}", other.token.describe()),
                other.span,
            );
            return recover(self);
        }
        self.advance();

        let iterable = self.parse_recovered_expression();

        if !self.at(Token::RParen) {
            let other = *self.peek();
            let _ = self.record(
                format!("expected `)`, but found {}", other.token.describe()),
                other.span,
            );
            return recover(self);
        }
        self.advance();

        if !self.at(Token::LBrace) {
            let other = *self.peek();
            let end = self.recover_to_next_statement(other.span.end);
            let error = self.record(
                format!("expected `{{`, but found {}", other.token.describe()),
                other.span,
            );
            return Stmt {
                span: Span::new(for_token.span.start, end),
                kind: StmtKind::Invalid { error },
            };
        }
        let open_brace = self.advance().span.start;
        let body = self.parse_block_body(open_brace);

        let end = self.tokens[self.pos - 1].span.end;
        Stmt::new(
            StmtKind::For {
                elem_ty,
                elem_name,
                iterable,
                body,
            },
            Span::new(for_token.span.start, end),
        )
    }

    fn parse_expression(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_logic_or()
    }

    /// The `( args )` of a call or a construction, starting at the `(`.
    /// Arguments are positional or named (§2.12); mixing the two forms in
    /// one list is a parse error. Named arguments separate by commas or by
    /// plain adjacency — the strip pass drops newlines inside parentheses
    /// (§A.8), so the multi-line named-argument style of §2.12 arrives as
    /// adjacent `name: value` pairs.
    fn parse_call_args(&mut self, start: Span) -> Result<(Vec<CallArg>, Span), Diagnostic> {
        self.advance();
        self.skip_newlines();
        let mut args = Vec::new();
        let mut has_positional = false;
        let mut has_named = false;
        if !self.at(Token::RParen) {
            loop {
                self.skip_newlines();
                let arg = if self.at_named_argument() {
                    let name_tok = self.advance();
                    self.advance(); // colon
                    let value = self.parse_expression()?;
                    has_named = true;
                    let name = match name_tok.token {
                        Token::Ident(name) => name.to_string(),
                        _ => unreachable!("checked by at_named_argument"),
                    };
                    CallArg::Named { name, expr: value }
                } else {
                    let value = self.parse_expression()?;
                    has_positional = true;
                    CallArg::Positional(value)
                };
                args.push(arg);
                self.skip_newlines();
                if self.at(Token::Comma) {
                    self.advance();
                    continue;
                }
                if self.at(Token::RParen) {
                    break;
                }
                // Adjacent named arguments: `f(x: 1 y: 2)`.
                if self.at_named_argument() {
                    continue;
                }
                let other = *self.peek();
                return Err(Self::expected(
                    "`,` or `)` in argument list",
                    &other.token,
                    other.span,
                ));
            }
        }
        let closing = *self.peek();
        if closing.token != Token::RParen {
            return Err(Self::expected("`)`", &closing.token, closing.span));
        }
        self.advance();
        if has_positional && has_named {
            // §2.12: mixing positional and named arguments is a
            // compile-time syntax error.
            return Err(Diagnostic::parse(
                "cannot mix positional and named arguments",
                closing.span,
            ));
        }
        Ok((args, Span::new(start.start, closing.span.end)))
    }

    /// True when the current token begins a named argument: an identifier
    /// directly followed by `:` (§2.12).
    fn at_named_argument(&self) -> bool {
        matches!(self.peek().token, Token::Ident(_))
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| t.token),
                Some(Token::Colon)
            )
    }

    fn parse_logic_or(&mut self) -> Result<Expr, Diagnostic> {
        self.enter()?;
        let result = self.parse_logic_or_loop();
        self.depth -= 1;
        result
    }

    fn parse_logic_or_loop(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_logic_and()?;

        while self.at(Token::Or) {
            self.advance();
            let rhs = self.parse_logic_and()?;
            let span = Span::new(expr.span.start, rhs.span.end);
            self.count_expr_op(span)?;
            expr = Expr::new(
                ExprKind::Binary {
                    op: BinaryOp::Or,
                    lhs: Box::new(expr),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }

        Ok(expr)
    }

    fn parse_logic_and(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_comparison()?;

        while self.at(Token::And) {
            self.advance();
            let rhs = self.parse_comparison()?;
            let span = Span::new(expr.span.start, rhs.span.end);
            self.count_expr_op(span)?;
            expr = Expr::new(
                ExprKind::Binary {
                    op: BinaryOp::And,
                    lhs: Box::new(expr),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }

        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<Expr, Diagnostic> {
        let lhs = self.parse_additive()?;
        let op_token = *self.peek();
        let Some(op) = comparison_operator(&op_token.token) else {
            return Ok(lhs);
        };
        self.advance();
        let rhs = self.parse_additive()?;

        if comparison_operator(&self.peek().token).is_some() {
            let next = *self.peek();
            return Err(Diagnostic::parse(
                "comparisons are non-associative; add parentheses",
                next.span,
            ));
        }

        let span = Span::new(lhs.span.start, rhs.span.end);
        self.count_expr_op(span)?;
        Ok(Expr::new(
            ExprKind::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            span,
        ))
    }

    fn parse_additive(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_multiplicative()?;

        while let Some(op) = additive_operator(&self.peek().token) {
            self.advance();
            let rhs = self.parse_multiplicative()?;
            let span = Span::new(expr.span.start, rhs.span.end);
            self.count_expr_op(span)?;
            expr = Expr::new(
                ExprKind::Binary {
                    op,
                    lhs: Box::new(expr),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }

        Ok(expr)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_unary()?;

        while let Some(op) = multiplicative_operator(&self.peek().token) {
            self.advance();
            let rhs = self.parse_unary()?;
            let span = Span::new(expr.span.start, rhs.span.end);
            self.count_expr_op(span)?;
            expr = Expr::new(
                ExprKind::Binary {
                    op,
                    lhs: Box::new(expr),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }

        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, Diagnostic> {
        self.enter()?;
        let result = self.parse_unary_guarded();
        self.depth -= 1;
        result
    }

    fn parse_unary_guarded(&mut self) -> Result<Expr, Diagnostic> {
        let op_span = self.peek().span;
        let unary_op = match self.peek().token {
            Token::Minus => UnaryOp::Neg,
            Token::Not => UnaryOp::Not,
            _ => return self.parse_postfix(),
        };

        self.advance();
        let expr = self.parse_unary()?;
        let span = Span::new(op_span.start, expr.span.end);
        Ok(Expr::new(
            ExprKind::Unary {
                op: unary_op,
                expr: Box::new(expr),
            },
            span,
        ))
    }

    fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
        let token = *self.peek();

        // A newline here means an operand is missing (for example `int i = `
        // with nothing typed yet). Fail without consuming so recovery can
        // plant a zero-width missing node and leave the statement boundary
        // intact.
        if matches!(token.token, Token::Newline) {
            return Err(Diagnostic::parse(
                "expected an expression, but found end of statement",
                token.span,
            ));
        }

        self.advance();
        match token.token {
            Token::IntLit(value) => Ok(Expr::new(ExprKind::IntLit(value), token.span)),
            Token::FloatLit(value) => Ok(Expr::new(ExprKind::FloatLit(value), token.span)),
            Token::StrLit(value) => Ok(Expr::new(
                ExprKind::StrLit(crate::lexer::unescape_str_lit(value)),
                token.span,
            )),
            Token::KwTrue => Ok(Expr::new(ExprKind::BoolLit(true), token.span)),
            Token::KwFalse => Ok(Expr::new(ExprKind::BoolLit(false), token.span)),
            Token::InterpStrLit(raw) => {
                let parts = self.parse_interp_parts(raw, token.span)?;
                Ok(Expr::new(ExprKind::Interpolated { parts }, token.span))
            }
            Token::LBracket => self.parse_array_literal(token),
            Token::LBrace => self.parse_map_literal(token),
            Token::KwMatch => self.parse_match_expression(token),
            Token::Ident(name) => {
                // A qualified call: `Enum.Variant(args)` (§2.7) or a deeper
                // impl-member path `engine.gamemode.InitGame(args)` (§2.3,
                // §10.4). The dotted chain is scanned ahead without
                // consuming; only a chain terminated by `(` becomes a call,
                // anything else falls through to postfix field access.
                let mut segments = 1usize;
                let mut scan = self.pos;
                while matches!(self.tokens.get(scan).map(|t| t.token), Some(Token::Dot))
                    && matches!(
                        self.tokens.get(scan + 1).map(|t| t.token),
                        Some(Token::Ident(_))
                    )
                {
                    segments += 1;
                    scan += 2;
                }
                if segments >= 2
                    && matches!(self.tokens.get(scan).map(|t| t.token), Some(Token::LParen))
                {
                    let mut path = vec![name.to_string()];
                    while path.len() < segments {
                        self.advance(); // dot
                        if let Token::Ident(segment) = self.advance().token {
                            path.push(segment.to_string());
                        }
                    }
                    let (args, span) = self.parse_call_args(token.span)?;
                    if path.len() == 2 {
                        // Two segments keep the `Enum.Variant` node; the
                        // checker disambiguates construction vs impl
                        // member (§2.7, §10.4).
                        let variant = path.pop().unwrap_or_default();
                        let enum_name = path.pop().unwrap_or_default();
                        return Ok(Expr::new(
                            ExprKind::VariantCall {
                                enum_name,
                                variant,
                                args,
                            },
                            span,
                        ));
                    }
                    return Ok(Expr::new(ExprKind::PathCall { path, args }, span));
                }
                // A call by name (§2.11, §2.12).
                if self.at(Token::LParen) {
                    let (args, span) = self.parse_call_args(token.span)?;
                    return Ok(Expr::new(
                        ExprKind::Call {
                            name: name.to_string(),
                            args,
                        },
                        span,
                    ));
                }
                Ok(Expr::new(ExprKind::Ident(name.to_string()), token.span))
            }
            Token::LParen => {
                let expr = self.parse_logic_or()?;
                let closing = self.require_token("expected `)`, but found end of file")?;
                if closing.token != Token::RParen {
                    return Err(Self::expected("`)`", &closing.token, closing.span));
                }
                Ok(Expr::new(
                    ExprKind::Paren {
                        expr: Box::new(expr),
                    },
                    Span::new(token.span.start, closing.span.end),
                ))
            }
            Token::Eof => Err(Diagnostic::parse(
                "expected an expression, but found end of file",
                token.span,
            )),
            other => Err(Self::expected("an expression", &other, token.span)),
        }
    }

    /// An array literal (§11): `[a, b, c]`. Elements separate by commas or
    /// newlines (newlines are significant inside brackets, §11.1 CMON
    /// style); a trailing comma is rejected, trailing newlines close.
    fn parse_array_literal(&mut self, open: SpannedToken<'src>) -> Result<Expr, Diagnostic> {
        // The `[` is already consumed.
        self.skip_newlines();
        let mut elements = Vec::new();
        if !self.at(Token::RBracket) {
            loop {
                elements.push(self.parse_expression()?);
                if self.at(Token::Comma) {
                    self.advance();
                    self.skip_newlines();
                    if self.at(Token::RBracket) {
                        let other = *self.peek();
                        return Err(Self::expected(
                            "an element after `,`",
                            &other.token,
                            other.span,
                        ));
                    }
                    continue;
                }
                if self.at(Token::Newline) {
                    self.skip_newlines();
                    if self.at(Token::RBracket) {
                        break;
                    }
                    continue;
                }
                if self.at(Token::RBracket) {
                    break;
                }
                let other = *self.peek();
                return Err(Self::expected(
                    "`,`, a newline, or `]` in array literal",
                    &other.token,
                    other.span,
                ));
            }
        }
        let closing = *self.peek();
        if closing.token != Token::RBracket {
            return Err(Self::expected("`]`", &closing.token, closing.span));
        }
        self.advance();
        Ok(Expr::new(
            ExprKind::ArrayLit { elements },
            Span::new(open.span.start, closing.span.end),
        ))
    }

    /// A map literal (§11): `{ key: value }`. Entries separate by commas or
    /// newlines (newlines are significant inside braces); a trailing comma
    /// is rejected, trailing newlines close.
    fn parse_map_literal(&mut self, open: SpannedToken<'src>) -> Result<Expr, Diagnostic> {
        // The `{` is already consumed.
        self.skip_newlines();
        let mut entries = Vec::new();
        if !self.at(Token::RBrace) {
            loop {
                let key = self.parse_expression()?;
                let colon = *self.peek();
                if colon.token != Token::Colon {
                    return Err(Self::expected(
                        "`:` after the map key",
                        &colon.token,
                        colon.span,
                    ));
                }
                self.advance();
                let value = self.parse_expression()?;
                entries.push((key, value));
                if self.at(Token::Comma) {
                    self.advance();
                    self.skip_newlines();
                    if self.at(Token::RBrace) {
                        let other = *self.peek();
                        return Err(Self::expected(
                            "an entry after `,`",
                            &other.token,
                            other.span,
                        ));
                    }
                    continue;
                }
                if self.at(Token::Newline) {
                    self.skip_newlines();
                    if self.at(Token::RBrace) {
                        break;
                    }
                    continue;
                }
                if self.at(Token::RBrace) {
                    break;
                }
                let other = *self.peek();
                return Err(Self::expected(
                    "`,`, a newline, or `}` in map literal",
                    &other.token,
                    other.span,
                ));
            }
        }
        let closing = *self.peek();
        if closing.token != Token::RBrace {
            return Err(Self::expected("`}`", &closing.token, closing.span));
        }
        self.advance();
        Ok(Expr::new(
            ExprKind::MapLit { entries },
            Span::new(open.span.start, closing.span.end),
        ))
    }

    /// The parts of an interpolated string (§2.8/§4.1): literal chunks and
    /// `{expr}` islands. Escapes decode only in the literal chunks; islands
    /// are re-lexed and parsed as expressions with spans offset to the
    /// island's absolute position.
    fn parse_interp_parts(&mut self, raw: &str, span: Span) -> Result<Vec<InterpPart>, Diagnostic> {
        // raw = `$"..."` including the sigil and both quotes.
        let content = &raw[2..raw.len() - 1];
        let base = span.start + 2;
        let mut parts = Vec::new();
        let mut literal_start = 0;
        let mut idx = 0;
        let bytes = content.as_bytes();
        while idx < bytes.len() {
            if bytes[idx] == b'{' {
                // The literal chunk before the island (never empty here).
                parts.push(InterpPart::Literal(unescape_interp_literal(
                    &content[literal_start..idx],
                )));
                // Find the matching `}` (string-aware: a `}` inside a
                // quoted string of the island does not close it, and
                // nested braces come from struct and map literals).
                let Some(close) = interp_island_close(bytes, idx) else {
                    return Err(Diagnostic::parse(
                        "unterminated interpolation island",
                        Span::new(base + idx, span.end - 1),
                    ));
                };
                let island = &content[idx + 1..close - 1];
                if island.trim().is_empty() {
                    return Err(Diagnostic::parse(
                        "empty interpolation island",
                        Span::new(base + idx, base + close),
                    ));
                }
                let expr = self.parse_island_expr(island, base + idx + 1)?;
                parts.push(InterpPart::Expr(Box::new(expr)));
                idx = close;
                literal_start = close;
            } else {
                idx += 1;
            }
        }
        parts.push(InterpPart::Literal(unescape_interp_literal(
            &content[literal_start..],
        )));
        Ok(parts)
    }

    /// Parses the text of one `{...}` island as an expression: lexes it with
    /// the standard lexer, offsets every span to the island's absolute
    /// position, and parses one expression with no trailing tokens. The
    /// island's parser inherits this parser's budgets — nesting and operator
    /// counts stay bounded across arbitrarily nested interpolation strings —
    /// and hands the monotonic counters back when it is done.
    fn parse_island_expr(&mut self, island: &str, offset: usize) -> Result<Expr, Diagnostic> {
        let (tokens, errors) = crate::lexer::lex_with_errors(island);
        if let Some(error) = errors.into_iter().next() {
            let shifted = shift_lex_error(error, offset);
            return Err(Diagnostic::lex(shifted));
        }
        let tokens: Vec<SpannedToken> = tokens
            .into_iter()
            .map(|spanned| SpannedToken {
                token: spanned.token,
                span: Span::new(spanned.span.start + offset, spanned.span.end + offset),
            })
            .collect();
        let mut parser = Parser::with_budget(&tokens, self.depth, self.expr_ops, self.type_nodes);
        let expr = match parser.parse_expression() {
            Ok(expr) => expr,
            Err(diagnostic) => {
                self.expr_ops = parser.expr_ops;
                self.type_nodes = parser.type_nodes;
                return Err(remap_island_eof(diagnostic, offset + island.len()));
            }
        };
        if !parser.at_eof() {
            let other = *parser.peek();
            return Err(Self::expected(
                "end of interpolation",
                &other.token,
                other.span,
            ));
        }
        self.expr_ops = parser.expr_ops;
        self.type_nodes = parser.type_nodes;
        Ok(expr)
    }
    /// A primary expression with its postfix chain: field access (§2.6),
    /// indexing (§11), and the `?` operator (§2.8). A `(` here can only be
    /// an attempt to call a non-name expression — functions and
    /// constructors are called by name in `parse_primary`.
    fn parse_postfix(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek().token {
                Token::Dot => {
                    self.advance();
                    let field_tok = *self.peek();
                    match field_tok.token {
                        Token::Ident(name) => {
                            self.advance();
                            let span = Span::new(expr.span.start, field_tok.span.end);
                            expr = Expr::new(
                                ExprKind::Field {
                                    obj: Box::new(expr),
                                    name: name.to_string(),
                                },
                                span,
                            );
                        }
                        _ => {
                            return Err(Self::expected(
                                "a field name after `.`",
                                &field_tok.token,
                                field_tok.span,
                            ));
                        }
                    }
                }
                Token::LBracket => {
                    self.advance();
                    self.skip_newlines();
                    let index = self.parse_expression()?;
                    self.skip_newlines();
                    let closing = *self.peek();
                    if closing.token != Token::RBracket {
                        return Err(Self::expected("`]`", &closing.token, closing.span));
                    }
                    self.advance();
                    let span = Span::new(expr.span.start, closing.span.end);
                    expr = Expr::new(
                        ExprKind::Index {
                            obj: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    );
                }
                Token::Question => {
                    let question = self.advance();
                    let span = Span::new(expr.span.start, question.span.end);
                    expr = Expr::new(
                        ExprKind::Try {
                            expr: Box::new(expr),
                        },
                        span,
                    );
                }
                Token::LParen => {
                    let other = *self.peek();
                    return Err(Diagnostic::parse(
                        "function calls must name a function or a constructor",
                        other.span,
                    ));
                }
                _ => break,
            }
        }
        Ok(expr)
    }
}

fn comparison_operator(token: &Token) -> Option<BinaryOp> {
    match token {
        Token::Eq => Some(BinaryOp::Eq),
        Token::Ne => Some(BinaryOp::Ne),
        Token::Lt => Some(BinaryOp::Lt),
        Token::Le => Some(BinaryOp::Le),
        Token::Gt => Some(BinaryOp::Gt),
        Token::Ge => Some(BinaryOp::Ge),
        _ => None,
    }
}

fn additive_operator(token: &Token) -> Option<BinaryOp> {
    match token {
        Token::Plus => Some(BinaryOp::Add),
        Token::Minus => Some(BinaryOp::Sub),
        _ => None,
    }
}

fn multiplicative_operator(token: &Token) -> Option<BinaryOp> {
    match token {
        Token::Star => Some(BinaryOp::Mul),
        Token::Slash => Some(BinaryOp::Div),
        Token::Percent => Some(BinaryOp::Rem),
        _ => None,
    }
}

/// The byte index just past the `}` matching the `{` at `open` in an
/// interpolated string's content, or `None` when the island never closes.
/// String-aware: a `"` inside the island opens a quoted run (escape pairs
/// honored) whose braces and quotes do not count, so `{ m["key"] }` and
/// `{ {"a": 1}["a"] }` find their true closing brace.
fn interp_island_close(content: &[u8], open: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut close = open + 1;
    let mut in_string = false;
    while close < content.len() {
        match content[close] {
            b'"' if in_string => in_string = false,
            b'"' if !in_string => in_string = true,
            b'\\' if in_string => {
                // Escape pair inside the island's string: skip both bytes.
                close += 1;
            }
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(close + 1);
                }
            }
            _ => {}
        }
        close += 1;
    }
    None
}

/// Decodes the four accepted escape pairs in an interpolated string's
/// literal chunk (the lexer has already validated them, §2.2).
fn unescape_interp_literal(chunk: &str) -> String {
    // The four accepted escapes (n, t, \\, ") — the lexer validated them.
    let mut out = String::with_capacity(chunk.len());
    let mut chars = chunk.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('n') => {
                chars.next();
                out.push('\n');
            }
            Some('t') => {
                chars.next();
                out.push('\t');
            }
            Some('\\') => {
                chars.next();
                out.push('\\');
            }
            Some('"') => {
                chars.next();
                out.push('"');
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// An island expression is parsed from its own token stream, so a dangling
/// operator meets that stream's end of file. The missing token is really the
/// island's closing `}` in the outer source, so report `}` instead.
fn remap_island_eof(error: Diagnostic, island_end: usize) -> Diagnostic {
    if error.message().contains("end of file") {
        let span = Span::new(island_end, island_end + 1);
        let message = error.message().replace("end of file", "`}`");
        return Diagnostic::parse(message, span);
    }
    error
}

/// Shifts a lexer error span (relative to an island's text) to the island's
/// absolute position in the source.
fn shift_lex_error(error: crate::lexer::LexError, offset: usize) -> crate::lexer::LexError {
    use crate::lexer::LexError;
    let shift = |span: Span| Span::new(span.start + offset, span.end + offset);
    match error {
        LexError::InvalidCharacter { span } => LexError::InvalidCharacter { span: shift(span) },
        LexError::UnterminatedString { span } => LexError::UnterminatedString { span: shift(span) },
        LexError::UnterminatedInterpolatedString { span } => {
            LexError::UnterminatedInterpolatedString { span: shift(span) }
        }
        LexError::UnterminatedBlockComment { span } => {
            LexError::UnterminatedBlockComment { span: shift(span) }
        }
        LexError::InvalidEscape { span } => LexError::InvalidEscape { span: shift(span) },
        LexError::IntegerOverflow { span } => LexError::IntegerOverflow { span: shift(span) },
        LexError::FloatOverflow { span } => LexError::FloatOverflow { span: shift(span) },
    }
}

/// Converts a parsed expression into an assignment target when its shape is
/// assignable: a variable, a field of a target, or an index into a target
/// (§2.10, §2.13, §A.7). Parenthesized and call results are not assignable.
fn expr_to_lvalue(expr: &Expr) -> Option<LValue> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(LValue::Var { name: name.clone() }),
        ExprKind::Field { obj, name } => Some(LValue::Field {
            base: Box::new(expr_to_lvalue(obj)?),
            name: name.clone(),
        }),
        ExprKind::Index { obj, index } => Some(LValue::Index {
            base: Box::new(expr_to_lvalue(obj)?),
            index: (**index).clone(),
        }),
        _ => None,
    }
}

/// The bracket kinds tracked by the newline strip pass.
#[derive(Clone, Copy, PartialEq)]
enum BracketKind {
    Paren,
    Bracket,
    Brace,
}

fn skip_to_next_statement<'a>(
    tokens: &[SpannedToken<'a>],
    pos: &mut usize,
) -> Option<SpannedToken<'a>> {
    while let Some(token) = tokens.get(*pos) {
        if token.token == Token::Eof {
            return None;
        }
        if matches!(token.token, Token::Newline) {
            *pos += 1;
            return Some(*token);
        }
        *pos += 1;
    }
    None
}

fn can_end_statement(t: &Token) -> bool {
    matches!(
        t,
        Token::Ident(_)
            | Token::IntLit(_)
            | Token::FloatLit(_)
            | Token::StrLit(_)
            | Token::InterpStrLit(_)
            | Token::KwTrue
            | Token::KwFalse
            | Token::RParen
            | Token::RBrace
            | Token::RBracket
            | Token::Question
    )
}

// ---------------------------------------------------------------------------
// Sub-parser helpers (§8.3.6) — parse-integrated extents for megaprograms
// ---------------------------------------------------------------------------

/// The outcome of one sub-parse: `Ok(())` when the text parses as the
/// requested fragment and nothing is left over, `Err(message)` otherwise.
type SubParseResult = Result<(), String>;

/// Shared driver for the `parse_*_text` helpers: lex, strip insignificant
/// newlines, run `parse`, then require zero diagnostics and full
/// consumption (trailing newlines allowed — they are insignificant at
/// statement level).
fn subparse_text(
    text: &str,
    what: &str,
    parse: impl FnOnce(&mut Parser<'_, '_>) -> bool,
) -> SubParseResult {
    let (tokens, lex_errors) = crate::lexer::lex_with_errors(text);
    if !lex_errors.is_empty() {
        return Err(format!("the text does not lex as Checkmate ({what})"));
    }
    let (tokens, strip_errors) = Parser::strip_insignificant_newlines_with_errors(tokens);
    if !strip_errors.is_empty() {
        return Err(format!("the text does not lex as Checkmate ({what})"));
    }
    let mut parser = Parser::new(&tokens);
    if !parse(&mut parser) {
        return Err(format!("the text does not parse as a Checkmate {what}"));
    }
    parser.skip_newlines();
    if !parser.at_eof() {
        return Err(format!(
            "unexpected trailing input after the Checkmate {what}"
        ));
    }
    if let Some(error) = parser.take_errors().first() {
        return Err(error.message().to_string());
    }
    Ok(())
}

/// Parses `text` as one Checkmate expression (§8.3.6). Used by the megaprogram
/// matcher for parse-integrated `$expr`/`$raw` boundaries and `$template`
/// islands: a candidate boundary is accepted only when the captured text
/// actually parses. Success/failure only — spans come from the original
/// region, not the fragment text.
pub fn parse_expr_text(text: &str) -> SubParseResult {
    subparse_text(text, "expression", |parser| {
        parser.parse_expression().is_ok()
    })
}

/// Parses `text` as one Checkmate type (§8.3.6) — `$type` islands.
pub fn parse_type_text(text: &str) -> SubParseResult {
    subparse_text(text, "type", |parser| parser.parse_type().is_some())
}

/// Parses `text` as one Checkmate block `{ … }` (§8.3.6) — `$block` islands.
pub fn parse_block_text(text: &str) -> SubParseResult {
    subparse_text(text, "block", |parser| {
        if !parser.at(Token::LBrace) {
            return false;
        }
        let open = parser.advance().span.start;
        parser.parse_block_body(open);
        true
    })
}

/// Parses `text` as a Checkmate statement list (§8.5's `cm.parseStmts`):
/// zero diagnostics and full consumption required. Used by the compile-time
/// evaluator to validate statements before they enter the expansion queue.
pub fn parse_stmts_text(text: &str) -> SubParseResult {
    subparse_text(text, "statement list", |parser| {
        let (_, errors) = parser.parse_program_with_errors();
        errors.is_empty()
    })
}
