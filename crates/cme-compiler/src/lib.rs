pub mod lexer;
pub mod parser;

#[cfg(test)]
mod tests {
    use super::lexer::Token;
use super::parser::Parser;
use cme_core::ast::{Expr, Stmt, Type};
use logos::Logos;

    #[test]
    fn test_parse_var_decl() {
        let source = "infer speed = 4.5";

        // 1. Lexing
        let lexer = Token::lexer(source);
        let tokens: Vec<Token> = lexer.map(|res| res.unwrap()).collect();

        // 2. Parsing
        let mut parser = Parser::new(&tokens);
        let ast = parser.parse_statement().expect("Failed to parse statement");

        // 3. Validation
        assert_eq!(
            ast,
            Stmt::VarDecl {
                ty: Type::Infer,
                name: "speed".to_string(),
                expr: Expr::FloatLit(4.5),
            }
        );
    }

    #[test]
    fn test_parse_program_rejects_trailing_rbrace() {
        let source = "infer x = 1\n}";
        let tokens: Vec<Token> = Token::lexer(source)
            .map(|result| result.unwrap())
            .collect();
        let mut parser = Parser::new(&tokens);

        assert!(parser.parse_program().is_err());
    }

    #[test]
    fn test_strip_insignificant_newlines_rejects_unbalanced_brackets() {
        assert!(Parser::strip_insignificant_newlines(vec![Token::RParen]).is_err());
        assert!(Parser::strip_insignificant_newlines(vec![Token::LParen]).is_err());
    }
}
