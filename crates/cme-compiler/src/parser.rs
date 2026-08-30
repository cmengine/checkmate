use crate::lexer::{LexError, SpannedToken, Token};
use std::fmt;

use cme_core::Span;
use cme_core::ast::{BinaryOp, CompoundOp, Expr, Stmt, SyntaxError, Type, UnaryOp};

#[derive(Debug, PartialEq, Clone)]
pub enum Diagnostic {
    Lex(LexError),
    Parse { message: String, span: Span },
}

impl From<Diagnostic> for String {
    fn from(value: Diagnostic) -> Self {
        match value {
            Diagnostic::Lex(_) => "invalid token".to_string(),
            Diagnostic::Parse { message, .. } => message,
        }
    }
}

impl Diagnostic {
    /// The precise source location of this diagnostic (offending token, or the
    /// position where something was expected).
    fn span(&self) -> Span {
        match self {
            Diagnostic::Lex(LexError::Invalid { span }) => *span,
            Diagnostic::Parse { span, .. } => *span,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Diagnostic::Lex(_) => write!(formatter, "invalid token"),
            Diagnostic::Parse { message, .. } => write!(formatter, "{message}"),
        }
    }
}

pub struct Parser<'a, 'src> {
    tokens: &'a [SpannedToken<'src>],
    pos: usize,
    eof_span: Span,
    /// Diagnostics recorded so far. Recoverable errors are pushed here while
    /// an `Invalid` node is planted into the AST at the failure site, so the
    /// parser never stops and always produces the fullest possible tree.
    errors: Vec<Diagnostic>,
}

impl<'a, 'src> Parser<'a, 'src> {
    pub fn new(tokens: &'a [SpannedToken<'src>], source_len: usize) -> Self {
        Self {
            tokens,
            pos: 0,
            eof_span: Span::new(source_len, source_len + 1),
            errors: Vec::new(),
        }
    }

    /// Records a recoverable diagnostic and returns the matching `SyntaxError`
    /// for embedding into an `Invalid` AST node.
    fn record(&mut self, message: impl Into<String>, span: Span) -> SyntaxError {
        let message = message.into();
        self.errors.push(Diagnostic::Parse {
            message: message.clone(),
            span,
        });
        SyntaxError { message, span }
    }

    /// Drains the diagnostics recorded so far. Useful alongside
    /// [`Parser::parse_statement`] when parsing a single statement directly.
    pub fn take_errors(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.errors)
    }

    fn peek(&self) -> Option<&SpannedToken<'src>> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&SpannedToken<'src>> {
        let token = self.tokens.get(self.pos);
        self.pos += 1;
        token
    }

    fn skip_newlines(&mut self) {
        while matches!(
            self.peek(),
            Some(SpannedToken {
                token: Token::Newline,
                ..
            })
        ) {
            self.pos += 1;
        }
    }

    fn parse_error(message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::Parse {
            message: message.into(),
            span,
        }
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
            if self.peek().is_none() {
                break;
            }

            stmts.push(self.parse_statement());

            match self.peek().cloned() {
                Some(SpannedToken {
                    token: Token::Newline,
                    ..
                }) => self.pos += 1,
                None => {}
                Some(other) => {
                    self.errors.push(Diagnostic::Parse {
                        message: format!(
                            "expected end of statement (newline), found {:?}",
                            other.token
                        ),
                        span: other.span,
                    });
                    self.skip_to_next_statement();
                }
            }
        }

        let errors = std::mem::take(&mut self.errors);
        (stmts, errors)
    }

    pub fn strip_insignificant_newlines(
        tokens: Vec<SpannedToken>,
        source_len: usize,
    ) -> Result<Vec<SpannedToken>, Diagnostic> {
        let (tokens, errors) = Self::strip_insignificant_newlines_with_errors(tokens, source_len);
        match errors.into_iter().next() {
            Some(error) => Err(error),
            None => Ok(tokens),
        }
    }

    pub fn strip_insignificant_newlines_with_errors(
        tokens: Vec<SpannedToken>,
        source_len: usize,
    ) -> (Vec<SpannedToken>, Vec<Diagnostic>) {
        let mut out = Vec::with_capacity(tokens.len());
        let mut errors = Vec::new();
        let mut bracket_depth = 0usize;
        let mut prev_can_end = false;
        let mut prev_was_type_kw = false;

        let mut index = 0usize;
        while index < tokens.len() {
            let SpannedToken { token: tok, span } = tokens[index].clone();
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
                    let next_starts_statement = tokens.get(index + 1).is_some_and(|next| {
                        matches!(
                            next.token,
                            Token::KwInt
                                | Token::KwFloat
                                | Token::KwStr
                                | Token::KwBool
                                | Token::KwInfer
                        )
                    });
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
                        errors.push(Self::parse_error("unbalanced closing parenthesis", span));
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
                    prev_was_type_kw = matches!(
                        tok,
                        Token::KwInt | Token::KwFloat | Token::KwStr | Token::KwBool | Token::KwInfer
                    );
                    out.push(SpannedToken { token: tok, span });
                }
            }
            index += 1;
        }

        if bracket_depth != 0 {
            errors.push(Self::parse_error(
                "unbalanced opening parenthesis",
                Span::new(source_len, source_len + 1),
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
        while let Some(token) = self.peek().cloned() {
            if matches!(token.token, Token::Newline) {
                break;
            }
            end = end.max(token.span.end);
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
                let start = self
                    .tokens
                    .get(start_pos)
                    .map_or(self.eof_span.start, |token| token.span.start);
                let mut end = start;
                if self.pos > start_pos {
                    // `advance` bumps `pos` even when it returns None, so a
                    // failure at end of file leaves `pos - 1` one past the
                    // array; fall back to the final token, which was
                    // necessarily consumed.
                    if let Some(token) =
                        self.tokens.get(self.pos - 1).or_else(|| self.tokens.last())
                    {
                        end = token.span.end;
                    }
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
        self.advance().cloned().ok_or(Diagnostic::Parse {
            message: message.to_string(),
            span: self.eof_span,
        })
    }

    /// Parses one statement. Infallible: a statement whose structure cannot
    /// be recognized becomes [`Stmt::Invalid`] covering the skipped region,
    /// and its diagnostic is recorded for [`Self::take_errors`] or
    /// [`Self::parse_program_with_errors`].
    pub fn parse_statement(&mut self) -> Stmt {
        let Some(first) = self.peek().cloned() else {
            let error = self.record("unexpected end of file", self.eof_span);
            return Stmt::Invalid {
                error,
                span: Span::missing(self.eof_span.start),
            };
        };
        self.pos += 1;

        if matches!(
            first.token,
            Token::KwInt | Token::KwFloat | Token::KwStr | Token::KwBool | Token::KwInfer
        ) {
            return self.parse_variable_declaration(first);
        }

        if let Token::Ident(name) = &first.token {
            return self.parse_assignment_statement(first.clone(), name.to_string());
        }

        let error = self.record(
            format!(
                "Expected a type or assignment target, but found {:?}",
                first.token
            ),
            first.span,
        );
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

        let name = match self.peek().cloned() {
            Some(SpannedToken {
                token: Token::Ident(name),
                ..
            }) => {
                self.pos += 1;
                name.to_string()
            }
            Some(other) => {
                let error = self.record(
                    format!("Expected a variable name, but found {:?}", other.token),
                    other.span,
                );
                let end = self.skip_to_statement_end(other.span.end);
                return Stmt::Invalid {
                    error,
                    span: Span::new(type_token.span.start, end),
                };
            }
            None => {
                let error = self.record(
                    "Expected a variable name, but reached end of file",
                    self.eof_span,
                );
                return Stmt::Invalid {
                    error,
                    span: Span::new(type_token.span.start, type_token.span.end),
                };
            }
        };

        // A declaration whose header simply ends (`int i` at a newline or end
        // of file) keeps the declared variable for tooling, with a zero-width
        // invalid initializer marking the missing `= value`.
        match self.peek().cloned() {
            None => {
                let expr = self.missing_expression(
                    "Expected '=', but reached end of file",
                    self.eof_span,
                    self.eof_span.start,
                );
                return Stmt::VarDecl { ty, name, expr };
            }
            Some(SpannedToken {
                token: Token::Newline,
                span,
            }) => {
                let expr = self.missing_expression(
                    "Expected '=', but found end of statement",
                    span,
                    span.start,
                );
                return Stmt::VarDecl { ty, name, expr };
            }
            Some(SpannedToken {
                token: Token::Assign,
                ..
            }) => self.pos += 1,
            Some(other) => {
                let error = self.record(
                    format!("Expected '=', but found {:?}", other.token),
                    other.span,
                );
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
        let Some(operator) = self.peek().cloned() else {
            let error = self.record(
                "Expected assignment operator, but reached end of file",
                self.eof_span,
            );
            return Stmt::Invalid {
                error,
                span: Span::new(target.span.start, target.span.end),
            };
        };

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
                let error = self.record(
                    format!("Expected assignment operator, but found {:?}", other),
                    operator.span,
                );
                let end = self.skip_to_statement_end(operator.span.end);
                return Stmt::Invalid {
                    error,
                    span: Span::new(target.span.start, end),
                };
            }
        };
        self.pos += 1;

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

        while matches!(
            self.peek(),
            Some(SpannedToken {
                token: Token::Or,
                ..
            })
        ) {
            self.advance();
            let rhs = self.parse_logic_and()?;
            if matches!(expr.1, LogicalKind::And) || matches!(rhs.1, LogicalKind::And) {
                return Err(Self::parse_error(
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

        while matches!(
            self.peek(),
            Some(SpannedToken {
                token: Token::And,
                ..
            })
        ) {
            self.advance();
            let rhs = self.parse_comparison()?;
            if matches!(expr.1, LogicalKind::Or) || matches!(rhs.1, LogicalKind::Or) {
                return Err(Self::parse_error(
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
        let Some(op_token) = self.peek().cloned() else {
            return Ok(lhs);
        };
        let Some(op) = comparison_operator(&op_token.token) else {
            return Ok(lhs);
        };
        self.advance();
        let rhs = self.parse_additive()?;

        if let Some(next) = self.peek()
            && comparison_operator(&next.token).is_some()
        {
            return Err(Diagnostic::Parse {
                message: "comparisons are non-associative; add parentheses".to_string(),
                span: next.span,
            });
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

        while let Some(op) = self
            .peek()
            .and_then(|token| additive_operator(&token.token))
        {
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

        while let Some(op) = self
            .peek()
            .and_then(|token| multiplicative_operator(&token.token))
        {
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
        let unary_op = match self.peek().map(|token| token.token.clone()) {
            Some(Token::Minus) => UnaryOp::Neg,
            Some(Token::Not) => UnaryOp::Not,
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
        let Some(token) = self.peek().cloned() else {
            return Err(Diagnostic::Parse {
                message: "Expected an expression, but reached end of file".to_string(),
                span: self.eof_span,
            });
        };

        // A newline here means an operand is missing (for example `int i = `
        // with nothing typed yet). Fail without consuming so recovery can
        // plant a zero-width missing node and leave the statement boundary
        // intact.
        if matches!(token.token, Token::Newline) {
            return Err(Diagnostic::Parse {
                message: "Expected an expression, but found end of statement".to_string(),
                span: token.span,
            });
        }

        self.pos += 1;
        match &token.token {
            Token::IntLit(value) => Ok((Expr::IntLit(*value), LogicalKind::None)),
            Token::FloatLit(value) => Ok((Expr::FloatLit(*value), LogicalKind::None)),
            Token::StrLit(value) => Ok((
                Expr::StrLit(value[1..value.len() - 1].to_string()),
                LogicalKind::None,
            )),
            Token::KwTrue => Ok((Expr::BoolLit(true), LogicalKind::None)),
            Token::KwFalse => Ok((Expr::BoolLit(false), LogicalKind::None)),
            Token::Ident(name) => Ok((Expr::Ident(name.to_string()), LogicalKind::None)),
            Token::LParen => {
                let expr = self.parse_logic_or()?;
                let closing = self.require_token("Expected ')', but reached end of file")?;
                if closing.token != Token::RParen {
                    return Err(Diagnostic::Parse {
                        message: format!("Expected ')', but found {:?}", closing.token),
                        span: closing.span,
                    });
                }
                Ok((expr.0, LogicalKind::Parenthesized))
            }
            other => Err(Diagnostic::Parse {
                message: format!("Expected an expression, but found {:?}", other),
                span: token.span,
            }),
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
            return Some(token.clone());
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
            | Token::RBrace
            | Token::KwReturn
    )
}
