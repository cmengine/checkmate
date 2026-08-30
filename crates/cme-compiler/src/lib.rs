pub mod lexer;
pub mod parser;
pub use logos;

#[cfg(test)]
mod tests {
    use super::lexer::{SpannedToken, Token, lex};
    use super::parser::Parser;
    use crate::parser::Diagnostic;
    use cme_core::ast::{BinaryOp, CompoundOp, Expr, Stmt, Type, UnaryOp};

    fn spanned_tokens(source: &str) -> Vec<SpannedToken<'_>> {
        lex(source).unwrap_or_else(|error| panic!("{source:?} should lex: {error:?}"))
    }

    fn parse_statement_ok(source: &str) -> Stmt {
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens, source.len());
        parser
            .parse_statement()
            .unwrap_or_else(|error| panic!("{source:?} should parse: {error:?}"))
    }

    fn bin(op: cme_core::ast::BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
        Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    fn unary(op: cme_core::ast::UnaryOp, expr: Expr) -> Expr {
        Expr::Unary {
            op,
            expr: Box::new(expr),
        }
    }

    fn compound(target: &str, op: cme_core::ast::CompoundOp, expr: Expr) -> Stmt {
        Stmt::CompoundAssign {
            target: target.to_string(),
            op,
            expr,
        }
    }

    fn parse_program(source: &str) -> Result<Vec<Stmt>, Diagnostic> {
        let tokens = spanned_tokens(source);
        let tokens = Parser::strip_insignificant_newlines(tokens, source.len())?;
        Parser::new(&tokens, source.len()).parse_program()
    }

    fn parse_program_parts(source: &str) -> (Vec<Stmt>, Vec<Diagnostic>) {
        let (tokens, lex_errors) = crate::lexer::lex_with_errors(source);
        let mut errors = lex_errors
            .into_iter()
            .map(Diagnostic::Lex)
            .collect::<Vec<_>>();
        let (tokens, strip_errors) =
            Parser::strip_insignificant_newlines_with_errors(tokens, source.len());
        errors.extend(strip_errors);
        let (stmts, parse_errors) = Parser::new(&tokens, source.len()).parse_program_with_errors();
        errors.extend(parse_errors);
        (stmts, errors)
    }

    fn parse_program_ok(source: &str) -> Vec<Stmt> {
        parse_program(source).unwrap_or_else(|error| panic!("{source:?} should parse: {error:?}"))
    }

    fn var_decl(ty: Type, name: &str, expr: Expr) -> Stmt {
        Stmt::VarDecl {
            ty,
            name: name.to_string(),
            expr,
        }
    }

    trait DeclarationExpr {
        fn declaration_expr(self) -> Expr;
    }

    impl DeclarationExpr for Stmt {
        fn declaration_expr(self) -> Expr {
            match self {
                Stmt::VarDecl { expr, .. } => expr,
                _ => panic!("expected a variable declaration"),
            }
        }
    }

    #[test]
    fn parses_inferred_float_variable_declaration() {
        let ast = parse_statement_ok("infer speed = 4.5");

        assert_eq!(ast, var_decl(Type::Infer, "speed", Expr::FloatLit(4.5)));
    }

    #[test]
    fn parses_declared_integer_variable() {
        let ast = parse_statement_ok("int count = 42");
        assert_eq!(ast, var_decl(Type::Int, "count", Expr::IntLit(42)));
    }

    #[test]
    fn parses_declared_float_variable() {
        let ast = parse_statement_ok("float ratio = 0.25");
        assert_eq!(ast, var_decl(Type::Float, "ratio", Expr::FloatLit(0.25)));
    }

    #[test]
    fn parses_identifier_expression() {
        let ast = parse_statement_ok("infer value = other_value");
        assert_eq!(
            ast,
            var_decl(Type::Infer, "value", Expr::Ident("other_value".to_string()))
        );
    }

    #[test]
    fn parses_declared_string_variable() {
        let ast = parse_statement_ok(r#"str message = "OMG WOW!""#);
        assert_eq!(
            ast,
            var_decl(Type::Str, "message", Expr::StrLit("OMG WOW!".to_string()))
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
    fn parses_precedence_and_associativity() {
        assert_eq!(
            parse_statement_ok("infer value = 1 + 2 * 3").declaration_expr(),
            bin(
                BinaryOp::Add,
                Expr::IntLit(1),
                bin(BinaryOp::Mul, Expr::IntLit(2), Expr::IntLit(3))
            )
        );
        assert_eq!(
            parse_statement_ok("infer value = 10 - 4 - 3").declaration_expr(),
            bin(
                BinaryOp::Sub,
                bin(BinaryOp::Sub, Expr::IntLit(10), Expr::IntLit(4)),
                Expr::IntLit(3)
            )
        );
        assert_eq!(
            parse_statement_ok("infer value = -x * y").declaration_expr(),
            bin(
                BinaryOp::Mul,
                unary(UnaryOp::Neg, Expr::Ident("x".into())),
                Expr::Ident("y".into())
            )
        );
        assert_eq!(
            parse_statement_ok("infer value = !!flag").declaration_expr(),
            unary(
                UnaryOp::Not,
                unary(UnaryOp::Not, Expr::Ident("flag".into()))
            )
        );
        assert_eq!(
            parse_statement_ok("infer value = --x").declaration_expr(),
            unary(UnaryOp::Neg, unary(UnaryOp::Neg, Expr::Ident("x".into())))
        );
    }

    #[test]
    fn parses_assignment_and_compound_assignment() {
        assert_eq!(
            parse_statement_ok("x = 1"),
            Stmt::Assign {
                name: "x".into(),
                expr: Expr::IntLit(1)
            }
        );
        assert_eq!(
            parse_statement_ok("x += 1"),
            compound("x", CompoundOp::Add, Expr::IntLit(1))
        );
        assert_eq!(
            parse_statement_ok("x -= 1"),
            compound("x", CompoundOp::Sub, Expr::IntLit(1))
        );
        assert_eq!(
            parse_statement_ok("x *= 1"),
            compound("x", CompoundOp::Mul, Expr::IntLit(1))
        );
        assert_eq!(
            parse_statement_ok("x /= 1"),
            compound("x", CompoundOp::Div, Expr::IntLit(1))
        );
        assert_eq!(
            parse_statement_ok("x %= 1"),
            compound("x", CompoundOp::Rem, Expr::IntLit(1))
        );
    }

    #[test]
    fn enforces_logical_parenthesization() {
        assert!(parse_program("infer x = a && b && c").is_ok());
        assert!(parse_program("infer x = a || b || c").is_ok());
        assert!(parse_program("infer x = a || (b && c)").is_ok());
        assert!(parse_program("infer x = (a || b) && c").is_ok());
        assert!(parse_program("infer x = a || b && c").is_err());
    }

    #[test]
    fn parses_empty_program() {
        assert!(parse_program_ok("").is_empty());
        assert!(parse_program_ok("\n\n").is_empty());
    }

    #[test]
    fn strips_newlines_inside_parentheses() {
        let source = "(\nvalue\n)";
        let tokens: Vec<Token> =
            Parser::strip_insignificant_newlines(spanned_tokens(source), source.len())
                .unwrap()
                .into_iter()
                .map(|spanned| spanned.token)
                .collect();
        assert_eq!(
            tokens,
            [Token::LParen, Token::Ident("value"), Token::RParen]
        );
    }

    #[test]
    fn preserves_statement_newlines_outside_parentheses() {
        let source = "value\nother\n";
        let tokens =
            Parser::strip_insignificant_newlines(spanned_tokens(source), source.len()).unwrap();
        let expected = spanned_tokens(source);
        assert_eq!(tokens, expected);
    }

    #[test]
    fn strips_leading_newlines_before_first_complete_statement() {
        let source = "\n\nvalue\n";
        let tokens: Vec<Token> =
            Parser::strip_insignificant_newlines(spanned_tokens(source), source.len())
                .unwrap()
                .into_iter()
                .map(|spanned| spanned.token)
                .collect();
        assert_eq!(tokens, [Token::Ident("value"), Token::Newline]);
    }

    #[test]
    fn rejects_unbalanced_brackets() {
        assert!(Parser::strip_insignificant_newlines(spanned_tokens(")"), 1).is_err());
        assert!(Parser::strip_insignificant_newlines(spanned_tokens("("), 1).is_err());
    }

    #[test]
    fn rejects_statement_without_valid_trailing_token() {
        let error = parse_program("int a = 1 }").unwrap_err();
        assert!(error.to_string().contains("expected end of statement"));
    }

    #[test]
    fn rejects_missing_variable_name() {
        let source = "int =";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens, source.len());

        let error = parser.parse_statement().unwrap_err();
        assert!(error.to_string().contains("Expected a variable name"));
    }

    #[test]
    fn rejects_missing_assignment_operator() {
        let source = "int count";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens, source.len());

        let error = parser.parse_statement().unwrap_err();
        assert!(error.to_string().contains("Expected '='"));
    }

    #[test]
    fn rejects_missing_expression() {
        let source = "int count =";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens, source.len());

        let error = parser.parse_statement().unwrap_err();
        assert!(error.to_string().contains("Expected an expression"));
    }

    #[test]
    fn rejects_unknown_statement_start() {
        let source = "}";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens, source.len());

        let error = parser.parse_statement().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Expected a type or assignment target")
        );
    }

    #[test]
    fn parses_boolean_variable_declaration() {
        let ast = parse_statement_ok("bool flag = true");
        assert_eq!(ast, var_decl(Type::Bool, "flag", Expr::BoolLit(true)));
        let ast = parse_statement_ok("infer flag = false");
        assert_eq!(ast, var_decl(Type::Infer, "flag", Expr::BoolLit(false)));
    }

    #[test]
    fn enforces_non_associative_comparisons() {
        assert!(parse_program("infer x = a < b").is_ok());
        assert!(parse_program("infer x = a == b").is_ok());
        assert!(parse_program("infer x = (a < b) == c").is_ok());
        assert!(parse_program("infer x = a < b < c").is_err());
        assert!(parse_program("infer x = a == b < c").is_err());
    }

    #[test]
    fn continues_expressions_after_trailing_operators() {
        assert!(parse_program("int total = base +\n    bonus").is_ok());
        assert!(parse_program("int d = a +\n    -b").is_ok());
        assert!(parse_program("infer x = (a +\n    b)").is_ok());
        assert!(parse_program("int total = base\n    + bonus").is_err());
    }

    #[test]
    fn rejects_unknown_expression_symbol() {
        let source = "infer value = }";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens, source.len());

        let error = parser.parse_statement().unwrap_err();
        assert!(error.to_string().contains("Expected an expression"));
    }

    #[test]
    fn rejects_trailing_rbrace_after_program_statement() {
        assert!(parse_program("infer x = 1\n}").is_err());
    }

    #[test]
    fn recovers_to_next_statement_after_lex_error() {
        let (stmts, errors) = parse_program_parts("@\nint b = 1\n");

        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], Diagnostic::Lex(_)));
        assert_eq!(stmts, vec![var_decl(Type::Int, "b", Expr::IntLit(1))]);
    }

    #[test]
    fn recovers_to_next_statement_after_parse_error() {
        let (stmts, errors) = parse_program_parts("int a = }\nint b = 2\n");

        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], Diagnostic::Parse { .. }));
        assert_eq!(stmts, vec![var_decl(Type::Int, "b", Expr::IntLit(2))]);
    }

    #[test]
    fn rejects_empty_parenthesized_statement() {
        let error = parse_program("infer value = ()").unwrap_err();
        assert!(error.to_string().contains("Expected an expression"));
    }

    #[test]
    fn rejects_unbalanced_brackets_in_program() {
        assert!(parse_program("infer value = (").is_err());
        assert!(parse_program("infer value = )").is_err());
    }

    #[test]
    fn parse_statement_does_not_require_all_input_to_be_consumed() {
        let source = "infer a = 1 }";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens, source.len());

        assert_eq!(
            parser.parse_statement().unwrap(),
            var_decl(Type::Infer, "a", Expr::IntLit(1))
        );
    }
}
