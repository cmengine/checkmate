use crate::diagnostics::Diagnostic;
use crate::lexer::{SpannedToken, Token};

use cme_core::Span;
use cme_core::ast::{BinaryOp, CompoundOp, Expr, Stmt, SyntaxError, Type, UnaryOp};

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

    /// Records a recoverable diagnostic and returns the matching `SyntaxError`
    /// for embedding into an `Invalid` AST node.
    fn record(&mut self, message: impl Into<String>, span: Span) -> SyntaxError {
        let message = message.into();
        self.errors.push(Diagnostic::parse(message.clone(), span));
        SyntaxError { message, span }
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
        let mut out = Vec::with_capacity(tokens.len());
        let mut errors = Vec::new();
        let mut bracket_depth = 0usize;
        let mut prev_can_end = false;
        let mut prev_was_type_kw = false;

        let mut index = 0usize;
        while index < tokens.len() {
            let SpannedToken { token: tok, span } = tokens[index];
            match tok {
                Token::Newline => {
                    // A newline survives at depth zero when it genuinely ends
                    // a statement — or when it follows a region the parser
                    // must still see as broken on its own line:
                    //   - a dangling type keyword (`float` alone) can never
                    //     continue onto the next line, and
                    //   - a type keyword can only START a statement, so a
                    //     line ending in a dangling operator (`int count =`)
                    //     must not swallow the declaration typed below it.
                    // Without this, recovery would fuse the broken line with
                    // the next one and eat the very declaration an LSP needs.
                    let next_starts_statement = tokens
                        .get(index + 1)
                        .is_some_and(|next| next.token.is_type_keyword());
                    if bracket_depth == 0
                        && (prev_can_end || prev_was_type_kw || next_starts_statement)
                    {
                        out.push(SpannedToken { token: tok, span });
                        prev_can_end = false;
                        prev_was_type_kw = false;
                    }
                }
                Token::LParen => {
                    bracket_depth += 1;
                    prev_can_end = false;
                    prev_was_type_kw = false;
                    out.push(SpannedToken { token: tok, span });
                }
                Token::RParen => {
                    if bracket_depth == 0 {
                        errors.push(Diagnostic::parse("unbalanced closing parenthesis", span));
                        if let Some(token) = skip_to_next_statement(&tokens, &mut index) {
                            out.push(token);
                        }
                        bracket_depth = 0;
                        prev_can_end = false;
                        prev_was_type_kw = false;
                        continue;
                    }
                    bracket_depth -= 1;
                    prev_can_end = true;
                    prev_was_type_kw = false;
                    out.push(SpannedToken { token: tok, span });
                }
                _ => {
                    prev_can_end = can_end_statement(&tok);
                    prev_was_type_kw = tok.is_type_keyword();
                    out.push(SpannedToken { token: tok, span });
                }
            }
            index += 1;
        }

        if bracket_depth != 0 {
            let eof_span = tokens
                .last()
                .filter(|token| token.token == Token::Eof)
                .map_or(Span::new(0, 0), |token| token.span);
            errors.push(Diagnostic::parse(
                "unbalanced opening parenthesis",
                eof_span,
            ));
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
                let error = SyntaxError::new(diagnostic.to_string(), diagnostic.span());
                self.errors.push(diagnostic);
                Expr::Invalid {
                    error,
                    span: Span::new(start, end),
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
        Expr::Invalid {
            error,
            span: Span::missing(offset),
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
    /// [`Self::parse_program_with_errors`].
    pub fn parse_statement(&mut self) -> Stmt {
        if self.at_eof() {
            let eof_span = self.eof_span();
            let error = self.record("unexpected end of file", eof_span);
            return Stmt::Invalid {
                error,
                span: Span::missing(eof_span.start),
            };
        }
        let first = self.advance();

        if first.token.is_type_keyword() {
            return self.parse_variable_declaration(first);
        }

        if let Token::Ident(name) = &first.token {
            return self.parse_assignment_statement(first, name.to_string());
        }

        let error = SyntaxError::new(
            format!(
                "expected a type or assignment target, but found {}",
                first.token.describe()
            ),
            first.span,
        );
        self.errors
            .push(Diagnostic::parse(error.message.clone(), first.span));
        let end = self.skip_to_statement_end(first.span.end);
        Stmt::Invalid {
            error,
            span: Span::new(first.span.start, end),
        }
    }

    fn parse_variable_declaration(&mut self, type_token: SpannedToken<'src>) -> Stmt {
        let ty = match type_token.token {
            Token::KwInt => Type::Int,
            Token::KwFloat => Type::Float,
            Token::KwStr => Type::Str,
            Token::KwBool => Type::Bool,
            Token::KwInfer => Type::Infer,
            _ => unreachable!("the caller only dispatches type keywords"),
        };

        let name = match *self.peek() {
            SpannedToken {
                token: Token::Ident(name),
                ..
            } => {
                self.advance();
                name.to_string()
            }
            other => {
                let error = SyntaxError::new(
                    format!(
                        "expected a variable name, but found {}",
                        other.token.describe()
                    ),
                    other.span,
                );
                self.errors
                    .push(Diagnostic::parse(error.message.clone(), other.span));
                let end = self.skip_to_statement_end(other.span.end);
                return Stmt::Invalid {
                    error,
                    span: Span::new(type_token.span.start, end),
                };
            }
        };

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
                return Stmt::VarDecl { ty, name, expr };
            }
            Token::Eof => {
                let span = self.eof_span();
                let expr = self.missing_expression(
                    "expected `=`, but found end of file",
                    span,
                    span.start,
                );
                return Stmt::VarDecl { ty, name, expr };
            }
            Token::Assign => {
                self.advance();
            }
            _ => {
                let other = *self.peek();
                let error = SyntaxError::new(
                    format!("expected `=`, but found {}", other.token.describe()),
                    other.span,
                );
                self.errors
                    .push(Diagnostic::parse(error.message.clone(), other.span));
                let end = self.skip_to_statement_end(other.span.end);
                return Stmt::Invalid {
                    error,
                    span: Span::new(type_token.span.start, end),
                };
            }
        }

        let expr = self.parse_recovered_expression();
        Stmt::VarDecl { ty, name, expr }
    }

    fn parse_assignment_statement(&mut self, target: SpannedToken<'src>, name: String) -> Stmt {
        let operator = *self.peek();

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
                let error = SyntaxError::new(
                    format!(
                        "expected an assignment operator, but found {}",
                        other.describe()
                    ),
                    operator.span,
                );
                self.errors
                    .push(Diagnostic::parse(error.message.clone(), operator.span));
                let end = self.skip_to_statement_end(operator.span.end);
                return Stmt::Invalid {
                    error,
                    span: Span::new(target.span.start, end),
                };
            }
        };
        self.advance();

        let expr = self.parse_recovered_expression();
        match compound {
            None => Stmt::Assign { name, expr },
            Some(op) => Stmt::CompoundAssign {
                target: name,
                op,
                expr,
            },
        }
    }

    fn parse_expression(&mut self) -> Result<Expr, Diagnostic> {
        Ok(self.parse_logic_or()?.0)
    }

    fn parse_logic_or(&mut self) -> Result<(Expr, LogicalKind), Diagnostic> {
        let mut expr = self.parse_logic_and()?;

        while self.at(Token::Or) {
            self.advance();
            let rhs = self.parse_logic_and()?;
            if matches!(expr.1, LogicalKind::And) || matches!(rhs.1, LogicalKind::And) {
                return Err(Diagnostic::parse(
                    "mixed && and || require parentheses",
                    expr_span(&expr.0),
                ));
            }
            expr = (
                Expr::Binary {
                    op: BinaryOp::Or,
                    lhs: Box::new(expr.0),
                    rhs: Box::new(rhs.0),
                },
                LogicalKind::Or,
            );
        }

        Ok(expr)
    }

    fn parse_logic_and(&mut self) -> Result<(Expr, LogicalKind), Diagnostic> {
        let mut expr = self.parse_comparison()?;

        while self.at(Token::And) {
            self.advance();
            let rhs = self.parse_comparison()?;
            if matches!(expr.1, LogicalKind::Or) || matches!(rhs.1, LogicalKind::Or) {
                return Err(Diagnostic::parse(
                    "mixed && and || require parentheses",
                    expr_span(&expr.0),
                ));
            }
            expr = (
                Expr::Binary {
                    op: BinaryOp::And,
                    lhs: Box::new(expr.0),
                    rhs: Box::new(rhs.0),
                },
                LogicalKind::And,
            );
        }

        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<(Expr, LogicalKind), Diagnostic> {
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

        Ok((
            Expr::Binary {
                op,
                lhs: Box::new(lhs.0),
                rhs: Box::new(rhs.0),
            },
            LogicalKind::None,
        ))
    }

    fn parse_additive(&mut self) -> Result<(Expr, LogicalKind), Diagnostic> {
        let mut expr = self.parse_multiplicative()?;

        while let Some(op) = additive_operator(&self.peek().token) {
            self.advance();
            let rhs = self.parse_multiplicative()?;
            expr = (
                Expr::Binary {
                    op,
                    lhs: Box::new(expr.0),
                    rhs: Box::new(rhs.0),
                },
                combine_operand_kind(expr.1, rhs.1),
            );
        }

        Ok(expr)
    }

    fn parse_multiplicative(&mut self) -> Result<(Expr, LogicalKind), Diagnostic> {
        let mut expr = self.parse_unary()?;

        while let Some(op) = multiplicative_operator(&self.peek().token) {
            self.advance();
            let rhs = self.parse_unary()?;
            expr = (
                Expr::Binary {
                    op,
                    lhs: Box::new(expr.0),
                    rhs: Box::new(rhs.0),
                },
                combine_operand_kind(expr.1, rhs.1),
            );
        }

        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<(Expr, LogicalKind), Diagnostic> {
        let unary_op = match self.peek().token {
            Token::Minus => UnaryOp::Neg,
            Token::Not => UnaryOp::Not,
            _ => return self.parse_primary(),
        };

        self.advance();
        let expr = self.parse_unary()?;
        let kind = expr.1;

        Ok((
            Expr::Unary {
                op: unary_op,
                expr: Box::new(expr.0),
            },
            kind,
        ))
    }

    fn parse_primary(&mut self) -> Result<(Expr, LogicalKind), Diagnostic> {
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
            Token::IntLit(value) => Ok((Expr::IntLit(value), LogicalKind::None)),
            Token::FloatLit(value) => Ok((Expr::FloatLit(value), LogicalKind::None)),
            Token::StrLit(value) => Ok((
                Expr::StrLit(value[1..value.len() - 1].to_string()),
                LogicalKind::None,
            )),
            Token::KwTrue => Ok((Expr::BoolLit(true), LogicalKind::None)),
            Token::KwFalse => Ok((Expr::BoolLit(false), LogicalKind::None)),
            Token::Ident(name) => Ok((Expr::Ident(name.to_string()), LogicalKind::None)),
            Token::LParen => {
                let expr = self.parse_logic_or()?;
                let closing = self.require_token("expected `)`, but found end of file")?;
                if closing.token != Token::RParen {
                    return Err(Self::expected("`)`", &closing.token, closing.span));
                }
                Ok((expr.0, LogicalKind::Parenthesized))
            }
            Token::Eof => Err(Diagnostic::parse(
                "expected an expression, but found end of file",
                token.span,
            )),
            other => Err(Self::expected("an expression", &other, token.span)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogicalKind {
    None,
    Or,
    And,
    Parenthesized,
}

fn combine_operand_kind(lhs: LogicalKind, rhs: LogicalKind) -> LogicalKind {
    match (lhs, rhs) {
        (LogicalKind::Parenthesized, _) | (_, LogicalKind::Parenthesized) => {
            LogicalKind::Parenthesized
        }
        _ => LogicalKind::None,
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

fn expr_span(expr: &Expr) -> Span {
    match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::StrLit(_) | Expr::BoolLit(_) => Span::new(0, 0),
        Expr::Ident(_) => Span::new(0, 0),
        Expr::Binary { .. } | Expr::Unary { .. } => Span::new(0, 0),
        Expr::Invalid { span, .. } => *span,
    }
}

fn skip_to_next_statement<'a>(
    tokens: &[SpannedToken<'a>],
    pos: &mut usize,
) -> Option<SpannedToken<'a>> {
    while let Some(token) = tokens.get(*pos) {
        *pos += 1;
        if matches!(token.token, Token::Newline) {
            return Some(*token);
        }
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
            | Token::KwTrue
            | Token::KwFalse
            | Token::RParen
    )
}
