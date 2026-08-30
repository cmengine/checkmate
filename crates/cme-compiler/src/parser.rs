use crate::lexer::Token;
use cme_core::ast::{Expr, Stmt, Type};

pub struct Parser<'a, 'src> {
    // A borrowed slice of tokens. 'src is the lifetime of the original string.
    tokens: &'a [Token<'src>],
    pos: usize,
}

impl<'a, 'src> Parser<'a, 'src> {
    pub fn new(tokens: &'a [Token<'src>]) -> Self {
        Self { tokens, pos: 0 }
    }

    /// Looks at the next token without consuming it
    #[allow(dead_code)]
    fn peek(&self) -> Option<&Token<'src>> {
        self.tokens.get(self.pos)
    }

    /// Consumes the next token and returns it, moving the parser forward
    fn advance(&mut self) -> Option<&Token<'src>> {
        let token = self.tokens.get(self.pos);
        self.pos += 1;
        token
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Some(Token::Newline)) {
            self.pos += 1;
        }
    }

    /// Parses a whole file (later: also a `{ ... }` block)
    pub fn parse_program(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();

        loop {
            self.skip_newlines();
            if self.peek().is_none() {
                break;
            }

            stmts.push(self.parse_statement()?);

            match self.peek() {
                Some(Token::Newline) => self.pos += 1, // separator
                None => {}
                Some(other) => {
                    return Err(format!(
                        "expected end of statement (newline), found {other:?}"
                    ));
                }
            }
        }
        Ok(stmts)
    }

    pub fn strip_insignificant_newlines(tokens: Vec<Token>) -> Result<Vec<Token>, String> {
        let mut out = Vec::with_capacity(tokens.len());
        let mut bracket_depth = 0usize;
        let mut prev_can_end = false;

        for tok in tokens {
            match tok {
                Token::Newline => {
                    if bracket_depth == 0 && prev_can_end {
                        out.push(Token::Newline);
                        prev_can_end = false;
                    }
                }
                Token::LParen => {
                    bracket_depth += 1;
                    prev_can_end = false;
                    out.push(tok);
                }
                Token::RParen => {
                    if bracket_depth == 0 {
                        return Err("unbalanced closing parenthesis".to_string());
                    }
                    bracket_depth -= 1;
                    prev_can_end = true;
                    out.push(tok);
                }
                _ => {
                    prev_can_end = can_end_statement(&tok);
                    out.push(tok);
                }
            }
        }

        if bracket_depth != 0 {
            return Err("unbalanced opening parenthesis".to_string());
        }

        Ok(out)
    }

    /// Parses a single statement like: `infer speed = 4.5`
    pub fn parse_statement(&mut self) -> Result<Stmt, String> {
        let token = self.advance().ok_or("Unexpected end of file")?;

        // 1. Parse the Type
        let ty = match token {
            Token::KwInt => Type::Int,
            Token::KwFloat => Type::Float,
            Token::KwStr => Type::Str,
            Token::KwInfer => Type::Infer,
            _ => {
                return Err(format!(
                    "Expected a type (int, float, infer), but found {:?}",
                    token
                ));
            }
        };

        // 2. Parse the Identifier (variable name)
        let name = match self.advance() {
            Some(Token::Ident(name)) => name.to_string(),
            Some(other) => return Err(format!("Expected a variable name, but found {:?}", other)),
            None => return Err("Expected a variable name, but reached end of file".to_string()),
        };

        // 3. Parse the '=' symbol
        match self.advance() {
            Some(Token::Assign) => {} // Great, do nothing and continue
            Some(other) => return Err(format!("Expected '=', but found {:?}", other)),
            None => return Err("Expected '=', but reached end of file".to_string()),
        }

        // 4. Parse the Expression
        let expr = match self.advance() {
            Some(Token::IntLit(val)) => Expr::IntLit(*val),
            Some(Token::FloatLit(val)) => Expr::FloatLit(*val),
            Some(Token::StrLit(value)) => Expr::StrLit(value[1..value.len() - 1].to_string()),
            Some(Token::Ident(name)) => Expr::Ident(name.to_string()),
            Some(other) => return Err(format!("Expected an expression, but found {:?}", other)),
            None => return Err("Expected an expression, but reached end of file".to_string()),
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
            | Token::KwInt
            | Token::KwFloat
            | Token::KwStr
            | Token::KwBool
            | Token::RParen
            | Token::RBrace
            | Token::KwReturn // later: bool literals, `]`, ...
    )
}
