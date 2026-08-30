use crate::lexer::{LexError, SpannedToken, Token};
use std::fmt;

use cme_core::Span;
use cme_core::ast::{BinaryOp, CompoundOp, Expr, Stmt, Type, UnaryOp};

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
}

impl<'a, 'src> Parser<'a, 'src> {
    pub fn new(tokens: &'a [SpannedToken<'src>], source_len: usize) -> Self {
        Self {
            tokens,
            pos: 0,
            eof_span: Span::new(source_len, source_len + 1),
        }
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

    pub fn parse_program_with_errors(&mut self) -> (Vec<Stmt>, Vec<Diagnostic>) {
        let mut stmts = Vec::new();
        let mut errors = Vec::new();

        loop {
            self.skip_newlines();
            if self.peek().is_none() {
                break;
            }

            match self.parse_statement() {
                Ok(stmt) => stmts.push(stmt),
                Err(error) => {
                    errors.push(error);
                    self.skip_to_next_statement();
                    continue;
                }
            }

            match self.peek() {
                Some(SpannedToken {
                    token: Token::Newline,
                    ..
                }) => self.pos += 1,
                None => {}
                Some(other) => {
                    errors.push(Diagnostic::Parse {
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

        let mut index = 0usize;
        while index < tokens.len() {
            let SpannedToken { token: tok, span } = tokens[index].clone();
            match tok {
                Token::Newline => {
                    if bracket_depth == 0 && prev_can_end {
                        out.push(SpannedToken { token: tok, span });
                        prev_can_end = false;
                    }
                }
                Token::LParen => {
                    bracket_depth += 1;
                    prev_can_end = false;
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
                        continue;
                    }
                    bracket_depth -= 1;
                    prev_can_end = true;
                    out.push(SpannedToken { token: tok, span });
                }
                _ => {
                    prev_can_end = can_end_statement(&tok);
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

    fn require_token(&mut self, message: &str) -> Result<SpannedToken<'src>, Diagnostic> {
        self.advance().cloned().ok_or(Diagnostic::Parse {
            message: message.to_string(),
            span: self.eof_span,
        })
    }

    pub fn parse_statement(&mut self) -> Result<Stmt, Diagnostic> {
        let first = self.require_token("unexpected end of file")?;

        if matches!(
            first.token,
            Token::KwInt | Token::KwFloat | Token::KwStr | Token::KwBool | Token::KwInfer
        ) {
            return self.parse_variable_declaration(first);
        }

        if let Token::Ident(name) = &first.token {
            return self.parse_assignment_statement(first.clone(), name.to_string());
        }

        Err(Diagnostic::Parse {
            message: format!(
                "Expected a type or assignment target, but found {:?}",
                first.token
            ),
            span: first.span,
        })
    }

    fn parse_variable_declaration(
        &mut self,
        type_token: SpannedToken<'src>,
    ) -> Result<Stmt, Diagnostic> {
        let ty = match type_token.token {
            Token::KwInt => Type::Int,
            Token::KwFloat => Type::Float,
            Token::KwStr => Type::Str,
            Token::KwBool => Type::Bool,
            Token::KwInfer => Type::Infer,
            _ => unreachable!("the caller only dispatches type keywords"),
        };

        let name = match self.advance() {
            Some(SpannedToken {
                token: Token::Ident(name),
                ..
            }) => name.to_string(),
            Some(other) => {
                return Err(Diagnostic::Parse {
                    message: format!("Expected a variable name, but found {:?}", other.token),
                    span: other.span,
                });
            }
            None => {
                return Err(Diagnostic::Parse {
                    message: "Expected a variable name, but reached end of file".to_string(),
                    span: self.eof_span,
                });
            }
        };

        let operator = self
            .require_token("Expected '=', but reached end of file")?
            .clone();
        if operator.token != Token::Assign {
            return Err(Diagnostic::Parse {
                message: format!("Expected '=', but found {:?}", operator.token),
                span: operator.span,
            });
        }

        let expr = self.parse_expression()?;
        Ok(Stmt::VarDecl { ty, name, expr })
    }

    fn parse_assignment_statement(
        &mut self,
        target: SpannedToken<'src>,
        name: String,
    ) -> Result<Stmt, Diagnostic> {
        let operator = self
            .require_token("Expected assignment operator, but reached end of file")?
            .clone();

        let op = match operator.token {
            Token::Assign => {
                let expr = self.parse_expression()?;
                return Ok(Stmt::Assign { name, expr });
            }
            Token::AddAssign => CompoundOp::Add,
            Token::SubAssign => CompoundOp::Sub,
            Token::MulAssign => CompoundOp::Mul,
            Token::DivAssign => CompoundOp::Div,
            Token::RemAssign => CompoundOp::Rem,
            other => {
                return Err(Diagnostic::Parse {
                    message: format!("Expected assignment operator, but found {:?}", other),
                    span: target.span,
                });
            }
        };

        let expr = self.parse_expression()?;
        Ok(Stmt::CompoundAssign {
            target: name,
            op,
            expr,
        })
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
        let token = self.require_token("Expected an expression, but reached end of file")?;
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
