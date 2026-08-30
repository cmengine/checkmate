pub mod lexer;
pub mod parser;

#[cfg(test)]
mod tests {
    use super::lexer::Token;
    use super::parser::Parser;
    use cme_core::ast::{Expr, Stmt, Type};
    use logos::Logos;

    fn parse_statement_ok(source: &str) -> (Stmt, Vec<Token<'_>>) {
        let tokens: Vec<Token> = Token::lexer(source).map(|result| result.unwrap()).collect();
        let mut parser = Parser::new(&tokens);
        let statement = parser
            .parse_statement()
            .unwrap_or_else(|error| panic!("{source:?} should parse: {error}"));
        (statement, tokens)
    }

    fn parse_program(source: &str) -> Result<Vec<Stmt>, String> {
        let tokens = Token::lexer(source)
            .map(|result| result.map_err(|_| "lexing failed".to_string()))
            .collect::<Result<Vec<_>, String>>()?;
        let tokens = Parser::strip_insignificant_newlines(tokens)?;
        Parser::new(&tokens).parse_program()
    }

    fn parse_program_ok(source: &str) -> Vec<Stmt> {
        parse_program(source).unwrap_or_else(|error| panic!("{source:?} should parse: {error}"))
    }

    fn var_decl(ty: Type, name: &str, expr: Expr) -> Stmt {
        Stmt::VarDecl {
            ty,
            name: name.to_string(),
            expr,
        }
    }

    #[test]
    fn parses_inferred_float_variable_declaration() {
        let (ast, _) = parse_statement_ok("infer speed = 4.5");

        assert_eq!(ast, var_decl(Type::Infer, "speed", Expr::FloatLit(4.5)));
    }

    #[test]
    fn parses_declared_integer_variable() {
        let (ast, _) = parse_statement_ok("int count = 42");
        assert_eq!(ast, var_decl(Type::Int, "count", Expr::IntLit(42)));
    }

    #[test]
    fn parses_declared_float_variable() {
        let (ast, _) = parse_statement_ok("float ratio = 0.25");
        assert_eq!(ast, var_decl(Type::Float, "ratio", Expr::FloatLit(0.25)));
    }

    #[test]
    fn parses_identifier_expression() {
        let (ast, _) = parse_statement_ok("infer value = other_value");
        assert_eq!(
            ast,
            var_decl(Type::Infer, "value", Expr::Ident("other_value".to_string()))
        );
    }

    #[test]
    fn parses_program_with_multiple_statements() {
        let ast = parse_program_ok("int a = 1\ninfer b = a\nfloat c = 2.5");
        assert_eq!(
            ast,
            vec![
                var_decl(Type::Int, "a", Expr::IntLit(1)),
                var_decl(Type::Infer, "b", Expr::Ident("a".to_string())),
                var_decl(Type::Float, "c", Expr::FloatLit(2.5)),
            ]
        );
    }

    #[test]
    fn parses_statements_surrounded_by_blank_lines() {
        let ast = parse_program_ok("\n\nint a = 1\n\n\ninfer b = a\n\n");
        assert_eq!(
            ast,
            vec![
                var_decl(Type::Int, "a", Expr::IntLit(1)),
                var_decl(Type::Infer, "b", Expr::Ident("a".to_string())),
            ]
        );
    }

    #[test]
    fn parses_empty_program() {
        assert!(parse_program_ok("").is_empty());
        assert!(parse_program_ok("\n\n").is_empty());
    }

    #[test]
    fn strips_newlines_inside_parentheses() {
        let tokens = vec![
            Token::LParen,
            Token::Newline,
            Token::Ident("value"),
            Token::Newline,
            Token::RParen,
        ];
        assert_eq!(
            Parser::strip_insignificant_newlines(tokens).unwrap(),
            vec![Token::LParen, Token::Ident("value"), Token::RParen]
        );
    }

    #[test]
    fn preserves_statement_newlines_outside_parentheses() {
        let tokens = vec![
            Token::Ident("value"),
            Token::Newline,
            Token::Ident("other"),
            Token::Newline,
        ];
        assert_eq!(
            Parser::strip_insignificant_newlines(tokens).unwrap(),
            vec![
                Token::Ident("value"),
                Token::Newline,
                Token::Ident("other"),
                Token::Newline,
            ]
        );
    }

    #[test]
    fn strips_leading_newlines_before_first_complete_statement() {
        let tokens = vec![
            Token::Newline,
            Token::Newline,
            Token::Ident("value"),
            Token::Newline,
        ];
        assert_eq!(
            Parser::strip_insignificant_newlines(tokens).unwrap(),
            vec![Token::Ident("value"), Token::Newline]
        );
    }

    #[test]
    fn rejects_unbalanced_brackets() {
        assert!(Parser::strip_insignificant_newlines(vec![Token::RParen]).is_err());
        assert!(Parser::strip_insignificant_newlines(vec![Token::LParen]).is_err());
    }

    #[test]
    fn rejects_statement_without_valid_trailing_token() {
        let error = parse_program("int a = 1 }").unwrap_err();
        assert!(error.contains("expected end of statement"));
    }

    #[test]
    fn rejects_missing_variable_name() {
        let tokens: Vec<Token> = Token::lexer("int =").map(Result::unwrap).collect();
        let mut parser = Parser::new(&tokens);

        let error = parser.parse_statement().unwrap_err();
        assert!(error.contains("Expected a variable name"));
    }

    #[test]
    fn rejects_missing_assignment_operator() {
        let tokens: Vec<Token> = Token::lexer("int count").map(Result::unwrap).collect();
        let mut parser = Parser::new(&tokens);

        let error = parser.parse_statement().unwrap_err();
        assert!(error.contains("Expected '='"));
    }

    #[test]
    fn rejects_missing_expression() {
        let tokens: Vec<Token> = Token::lexer("int count =").map(Result::unwrap).collect();
        let mut parser = Parser::new(&tokens);

        let error = parser.parse_statement().unwrap_err();
        assert!(error.contains("Expected an expression"));
    }

    #[test]
    fn rejects_unknown_statement_start() {
        let tokens: Vec<Token> = Token::lexer("value").map(Result::unwrap).collect();
        let mut parser = Parser::new(&tokens);

        let error = parser.parse_statement().unwrap_err();
        assert!(error.contains("Expected a type"));
    }

    #[test]
    fn rejects_unsupported_type_keyword() {
        let tokens: Vec<Token> = Token::lexer("str message").map(Result::unwrap).collect();
        let mut parser = Parser::new(&tokens);

        let error = parser.parse_statement().unwrap_err();
        assert!(error.contains("Expected a type"));
    }

    #[test]
    fn rejects_unknown_expression_symbol() {
        let tokens: Vec<Token> = Token::lexer("infer value = }")
            .map(Result::unwrap)
            .collect();
        let mut parser = Parser::new(&tokens);

        let error = parser.parse_statement().unwrap_err();
        assert!(error.contains("Expected an expression"));
    }

    #[test]
    fn rejects_trailing_rbrace_after_program_statement() {
        assert!(parse_program("infer x = 1\n}").is_err());
    }

    #[test]
    fn rejects_empty_parenthesized_statement() {
        let error = parse_program("infer value = ()").unwrap_err();
        assert!(error.contains("Expected an expression"));
    }

    #[test]
    fn rejects_unbalanced_brackets_in_program() {
        assert!(parse_program("infer value = (").is_err());
        assert!(parse_program("infer value = )").is_err());
    }

    #[test]
    fn parse_statement_does_not_require_all_input_to_be_consumed() {
        let tokens: Vec<Token> = Token::lexer("infer a = 1 }").map(Result::unwrap).collect();
        let mut parser = Parser::new(&tokens);

        assert_eq!(
            parser.parse_statement().unwrap(),
            var_decl(Type::Infer, "a", Expr::IntLit(1))
        );
    }
}
