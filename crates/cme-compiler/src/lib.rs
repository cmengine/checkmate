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
}
