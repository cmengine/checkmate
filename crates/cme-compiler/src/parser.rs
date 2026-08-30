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

    /// Parses a single statement like: `infer speed = 4.5`
    pub fn parse_statement(&mut self) -> Result<Stmt, String> {
        let token = self.advance().ok_or("Unexpected end of file")?;

        // 1. Parse the Type
        let ty = match token {
            Token::KwInt => Type::Int,
            Token::KwFloat => Type::Float,
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
            Some(Token::Ident(name)) => Expr::Ident(name.to_string()),
            Some(other) => return Err(format!("Expected an expression, but found {:?}", other)),
            None => return Err("Expected an expression, but reached end of file".to_string()),
        };

        // We successfully built a piece of the AST!
        Ok(Stmt::VarDecl { ty, name, expr })
    }
}
