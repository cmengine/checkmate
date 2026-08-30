use crate::lexer::{LexError, SpannedToken, Token};
use std::fmt;

use cme_core::Span;
use cme_core::ast::{Expr, Stmt, Type};

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
    // A borrowed slice of tokens. 'src is the lifetime of the original string.
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

    /// Looks at the next token without consuming it
    #[allow(dead_code)]
    fn peek(&self) -> Option<&SpannedToken<'src>> {
        self.tokens.get(self.pos)
    }

    /// Consumes the next token and returns it, moving the parser forward
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

    /// Parses a whole file (later: also a `{ ... }` block)
    pub fn parse_program(&mut self) -> Result<Vec<Stmt>, Diagnostic> {
        let mut stmts = Vec::new();

        loop {
            self.skip_newlines();
            if self.peek().is_none() {
                break;
            }

            stmts.push(self.parse_statement()?);

            match self.peek() {
                Some(SpannedToken {
                    token: Token::Newline,
                    ..
                }) => self.pos += 1, // separator
                None => {}
                Some(other) => {
                    return Err(Diagnostic::Parse {
                        message: format!(
                            "expected end of statement (newline), found {:?}",
                            other.token
                        ),
                        span: other.span,
                    });
                }
            }
        }
        Ok(stmts)
    }

    pub fn strip_insignificant_newlines(
        tokens: Vec<SpannedToken>,
        source_len: usize,
    ) -> Result<Vec<SpannedToken>, Diagnostic> {
        let mut out = Vec::with_capacity(tokens.len());
        let mut bracket_depth = 0usize;
        let mut prev_can_end = false;

        for SpannedToken { token: tok, span } in tokens {
            match tok {
                Token::Newline => {
                    if bracket_depth == 0 && prev_can_end {
                        out.push(SpannedToken {
                            token: Token::Newline,
                            span,
                        });
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
                        return Err(Diagnostic::Parse {
                            message: "unbalanced closing parenthesis".to_string(),
                            span,
                        });
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
        }

        if bracket_depth != 0 {
            return Err(Diagnostic::Parse {
                message: "unbalanced opening parenthesis".to_string(),
                span: Span::new(source_len, source_len + 1),
            });
        }

        Ok(out)
    }

    /// Parses a single statement like: `infer speed = 4.5`
    pub fn parse_statement(&mut self) -> Result<Stmt, Diagnostic> {
        let eof_span = self.eof_span;
        let token = self.advance().ok_or(Diagnostic::Parse {
            message: "unexpected end of file".to_string(),
            span: eof_span,
        })?;

        // 1. Parse the Type
        let ty = match &token.token {
            Token::KwInt => Type::Int,
            Token::KwFloat => Type::Float,
            Token::KwStr => Type::Str,
            Token::KwInfer => Type::Infer,
            Token::KwBool => Type::Bool,
            _ => {
                return Err(Diagnostic::Parse {
                    message: format!(
                        "Expected a type (int, float, infer), but found {:?}",
                        token.token
                    ),
                    span: token.span,
                });
            }
        };

        // 2. Parse the Identifier (variable name)
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

        // 3. Parse the '=' symbol
        match self.advance() {
            Some(SpannedToken {
                token: Token::Assign,
                ..
            }) => {}
            Some(other) => {
                return Err(Diagnostic::Parse {
                    message: format!("Expected '=', but found {:?}", other.token),
                    span: other.span,
                });
            }
            None => {
                return Err(Diagnostic::Parse {
                    message: "Expected '=', but reached end of file".to_string(),
                    span: self.eof_span,
                });
            }
        }

        // 4. Parse the Expression
        let expr = match self.advance() {
            Some(SpannedToken {
                token: Token::IntLit(val),
                ..
            }) => Expr::IntLit(*val),
            Some(SpannedToken {
                token: Token::FloatLit(val),
                ..
            }) => Expr::FloatLit(*val),
            Some(SpannedToken {
                token: Token::StrLit(value),
                ..
            }) => Expr::StrLit(value[1..value.len() - 1].to_string()),
            Some(SpannedToken {
                token: Token::KwTrue,
                ..
            }) => Expr::BoolLit(true),
            Some(SpannedToken {
                token: Token::KwFalse,
                ..
            }) => Expr::BoolLit(false),
            Some(SpannedToken {
                token: Token::Ident(name),
                ..
            }) => Expr::Ident(name.to_string()),
            Some(other) => {
                return Err(Diagnostic::Parse {
                    message: format!("Expected an expression, but found {:?}", other.token),
                    span: other.span,
                });
            }
            None => {
                return Err(Diagnostic::Parse {
                    message: "Expected an expression, but reached end of file".to_string(),
                    span: self.eof_span,
                });
            }
        };

        // We successfully built a piece of the AST!
        Ok(Stmt::VarDecl { ty, name, expr })
    }
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
            | Token::KwInt
            | Token::KwFloat
            | Token::KwStr
            | Token::KwBool
            | Token::RParen
            | Token::RBrace
            | Token::KwReturn // later: `]`, ...
    )
}
