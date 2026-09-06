use crate::diagnostics::Diagnostic;
use crate::lexer::{SpannedToken, Token};
use crate::validate;

use cme_core::Span;
use cme_core::ast::{
    BinaryOp, Block, CallArg, CompoundOp, ErrorId, Expr, ExprKind, FieldDef, LValue, Param,
    PrimitiveType, Stmt, StmtKind, Type, UnaryOp, VariantDecl,
};

pub struct Parser<'a, 'src> {
    tokens: &'a [SpannedToken<'src>],
    pos: usize,
    /// Diagnostics recorded so far. Recoverable errors are pushed here while
    /// an `Invalid` node is planted into the AST at the failure site, so the
    /// parser never stops and always produces the fullest possible tree.
    errors: Vec<Diagnostic>,
}

impl<'a, 'src> Parser<'a, 'src> {
    pub fn new(tokens: &'a [SpannedToken<'src>]) -> Self {
        debug_assert!(
            matches!(tokens.last(), Some(token) if token.token == Token::Eof),
            "token stream must end with a synthetic Eof"
        );
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
        }
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
                    let significant = match stack.last() {
                        Some(BracketKind::Paren) => false,
                        Some(BracketKind::Bracket) | Some(BracketKind::Brace) => true,
                        None => prev_can_end || prev_was_type_kw || next_starts_statement,
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
                    out.push(SpannedToken { token: tok, span });
                }
                Token::LBracket => {
                    stack.push(BracketKind::Bracket);
                    prev_can_end = false;
                    prev_was_type_kw = false;
                    out.push(SpannedToken { token: tok, span });
                }
                Token::LBrace => {
                    stack.push(BracketKind::Brace);
                    prev_can_end = false;
                    prev_was_type_kw = false;
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
                        out.push(SpannedToken { token: tok, span });
                    } else if stack.is_empty() && matches!(tok, Token::RBracket | Token::RBrace) {
                        // A stray `]` or `}` at the top of the stream is kept
                        // as a plain token: the parser reports it as an
                        // unrecognizable statement (boom.cm pins this).
                        prev_can_end = can_end_statement(&tok);
                        prev_was_type_kw = false;
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
    pub fn parse_statement(&mut self) -> Stmt {
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
        if first.token == Token::KwEnum {
            return self.parse_enum_declaration(first);
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

        if let Token::Ident(name) = &first.token {
            return self.parse_assignment_statement(first, name.to_string());
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
                Type::Named {
                    name: name.to_string(),
                    args: Vec::new(),
                }
            }
            _ => return None,
        };
        Some(self.parse_type_array_suffixes(base))
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
            return Some(self.parse_type_array_suffixes(Type::Map {
                key: Box::new(first),
                value: Box::new(value),
            }));
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
        Some(self.parse_type_array_suffixes(Type::Named { name, args }))
    }

    /// Zero or more `[]` suffixes on an already-parsed base type (§11).
    fn parse_type_array_suffixes(&mut self, ty: Type) -> Type {
        let mut ty = ty;
        while self.at(Token::LBracket)
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| t.token),
                Some(Token::RBracket)
            )
        {
            self.advance();
            self.advance();
            ty = Type::Array(Box::new(ty));
        }
        ty
    }

    /// A type declaration led by a scalar keyword (or `void`): the full
    /// declared type (with `[]` suffixes), the declared name, then either a
    /// function (followed by `(`) or a variable (followed by `=`, a newline,
    /// or end of file). `name_kind` selects the diagnostic for a missing
    /// name, preserving the historical split between the `void`-led
    /// function path and the variable path.
    fn parse_type_declaration(&mut self, type_token: SpannedToken<'src>, name_kind: &str) -> Stmt {
        let mut ty = Self::parse_type_from_token(&type_token.token).unwrap_or(Type::Infer);
        ty = self.parse_type_array_suffixes(ty);

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
        let _open_brace = self.advance().span.start;

        let mut fields = Vec::new();
        let end = self.parse_decl_member_list(
            &mut |parser| {
                // One `Type name` field (§2.6); no commas.
                parser.parse_decl_member()
            },
            &mut fields,
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
        let _open_brace = self.advance().span.start;

        let mut variants = Vec::new();
        let end =
            self.parse_decl_member_list(&mut |parser| parser.parse_enum_variant(), &mut variants);

        Stmt::new(
            StmtKind::EnumDecl {
                name,
                type_params,
                variants,
            },
            Span::new(enum_token.span.start, end),
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
    /// are newline-delimited, no commas). Returns the body end offset.
    fn parse_decl_member_list<T>(
        &mut self,
        member: &mut dyn FnMut(&mut Self) -> Option<T>,
        out: &mut Vec<T>,
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

    fn parse_assignment_statement(&mut self, target: SpannedToken<'src>, name: String) -> Stmt {
        let operator = *self.peek();

        if operator.token == Token::LParen {
            match self.parse_call_expression(name, target.span) {
                Ok(expr) => {
                    return Stmt::new(
                        StmtKind::Expression { expr },
                        Span::new(target.span.start, self.tokens[self.pos - 1].span.end),
                    );
                }
                Err(diagnostic) => {
                    self.errors.push(diagnostic);
                    let end = self.skip_to_statement_end(operator.span.end);
                    return Stmt {
                        span: Span::new(target.span.start, end),
                        kind: StmtKind::Invalid {
                            error: ErrorId(self.errors.len() - 1),
                        },
                    };
                }
            }
        }

        let compound = match operator.token {
            Token::Assign => None,
            Token::AddAssign => Some(CompoundOp::Add),
            Token::SubAssign => Some(CompoundOp::Sub),
            Token::MulAssign => Some(CompoundOp::Mul),
            Token::DivAssign => Some(CompoundOp::Div),
            Token::RemAssign => Some(CompoundOp::Rem),
            other => {
                // A bare identifier with no operator has no recoverable
                // statement structure; record it as an invalid statement.
                let end = self.skip_to_statement_end(operator.span.end);
                let error = self.record(
                    format!(
                        "expected an assignment operator, but found {}",
                        other.describe()
                    ),
                    operator.span,
                );
                return Stmt {
                    span: Span::new(target.span.start, end),
                    kind: StmtKind::Invalid { error },
                };
            }
        };
        self.advance();

        let expr = self.parse_recovered_expression();
        let kind = match compound {
            None => StmtKind::Assign {
                target: LValue::Var { name },
                expr: expr.clone(),
            },
            Some(op) => StmtKind::CompoundAssign {
                target: LValue::Var { name },
                op,
                expr: expr.clone(),
            },
        };
        Stmt::new(kind, Span::new(target.span.start, expr.span.end))
    }

    fn parse_expression(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_logic_or()
    }

    fn parse_call_expression(&mut self, name: String, start: Span) -> Result<Expr, Diagnostic> {
        self.advance();
        self.skip_newlines();
        let mut args = Vec::new();
        if !self.at(Token::RParen) {
            loop {
                self.skip_newlines();
                args.push(CallArg::Positional(self.parse_expression()?));
                self.skip_newlines();
                if self.at(Token::Comma) {
                    self.advance();
                    continue;
                }
                if self.at(Token::RParen) {
                    break;
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
        let span = Span::new(start.start, closing.span.end);
        Ok(Expr::new(ExprKind::Call { name, args }, span))
    }

    fn parse_logic_or(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_logic_and()?;

        while self.at(Token::Or) {
            self.advance();
            let rhs = self.parse_logic_and()?;
            let span = Span::new(expr.span.start, rhs.span.end);
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
        let op_span = self.peek().span;
        let unary_op = match self.peek().token {
            Token::Minus => UnaryOp::Neg,
            Token::Not => UnaryOp::Not,
            _ => return self.parse_primary(),
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
            Token::Ident(name) => {
                if self.at(Token::LParen) {
                    self.parse_call_expression(name.to_string(), token.span)
                } else {
                    Ok(Expr::new(ExprKind::Ident(name.to_string()), token.span))
                }
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
