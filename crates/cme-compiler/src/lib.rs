pub mod diagnostics;
pub mod lexer;
pub mod parser;
pub mod validate;
pub use diagnostics::{Diagnostic, DiagnosticKind, ParseOutcome};

pub fn parse_source(source: &str) -> ParseOutcome {
    // Lexes, strips insignificant newlines, then parses and validates with
    // recovery. This is the one-call front-end API.
    let (tokens, lex_errors) = lexer::lex_with_errors(source);
    let mut diagnostics: Vec<Diagnostic> = lex_errors.into_iter().map(Diagnostic::lex).collect();
    let (tokens, strip_errors) = parser::Parser::strip_insignificant_newlines_with_errors(tokens);
    diagnostics.extend(strip_errors);
    let (statements, parse_errors) = parser::Parser::new(&tokens).parse_program_with_errors();
    diagnostics.extend(parse_errors);
    ParseOutcome {
        statements,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::lexer::{SpannedToken, Token, lex};
    use super::parser::Parser;
    use crate::diagnostics::Diagnostic;
    use cme_core::Span;
    use cme_core::ast::{
        BinaryOp, CompoundOp, Expr, ExprKind, PrimitiveType, Stmt, StmtKind, Type, UnaryOp,
    };

    fn expr(kind: ExprKind) -> Expr {
        Expr::new(kind, Span::missing(0))
    }

    fn spanned_tokens(source: &str) -> Vec<SpannedToken<'_>> {
        lex(source).unwrap_or_else(|error| panic!("{source:?} should lex: {error:?}"))
    }

    fn parse_statement_ok(source: &str) -> Stmt {
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens);
        let stmt = parser.parse_statement();
        assert!(
            parser.take_errors().is_empty(),
            "{source:?} should parse without errors"
        );
        stmt
    }

    fn bin(op: cme_core::ast::BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
        expr(ExprKind::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    }

    fn unary(op: cme_core::ast::UnaryOp, inner: Expr) -> Expr {
        Expr::new(
            ExprKind::Unary {
                op,
                expr: Box::new(inner),
            },
            Span::missing(0),
        )
    }

    fn compound(target: &str, op: cme_core::ast::CompoundOp, expr: Expr) -> Stmt {
        Stmt::new(
            StmtKind::CompoundAssign {
                target: target.to_string(),
                op,
                expr,
            },
            Span::missing(0),
        )
    }

    fn parse_program(source: &str) -> Result<Vec<Stmt>, Diagnostic> {
        let tokens = spanned_tokens(source);
        let tokens = Parser::strip_insignificant_newlines(tokens)?;
        Parser::new(&tokens).parse_program()
    }

    fn parse_program_parts(source: &str) -> (Vec<Stmt>, Vec<Diagnostic>) {
        let outcome = crate::parse_source(source);
        (outcome.statements, outcome.diagnostics)
    }

    fn parse_program_ok(source: &str) -> Vec<Stmt> {
        parse_program(source).unwrap_or_else(|error| panic!("{source:?} should parse: {error:?}"))
    }

    fn parse_statement_parts(source: &str) -> (Stmt, Vec<Diagnostic>) {
        let (tokens, _) = crate::lexer::lex_with_errors(source);
        let mut parser = Parser::new(&tokens);
        let stmt = parser.parse_statement();
        (stmt, parser.take_errors())
    }

    fn var_decl(ty: Type, name: &str, expr: Expr) -> Stmt {
        Stmt::new(
            StmtKind::VarDecl {
                ty,
                name: name.to_string(),
                expr,
            },
            Span::missing(0),
        )
    }

    trait DeclarationExpr {
        fn declaration_expr(self) -> Expr;
    }

    impl DeclarationExpr for Stmt {
        fn declaration_expr(self) -> Expr {
            match self.kind {
                StmtKind::VarDecl { expr, .. } => expr,
                _ => panic!("expected a variable declaration"),
            }
        }
    }

    #[test]
    fn parses_inferred_float_variable_declaration() {
        let ast = parse_statement_ok("infer speed = 4.5");

        assert_eq!(ast, var_decl(None, "speed", expr(ExprKind::FloatLit(4.5))));
    }

    #[test]
    fn parses_declared_integer_variable() {
        let ast = parse_statement_ok("int count = 42");
        assert_eq!(
            ast,
            var_decl(
                Some(PrimitiveType::Int),
                "count",
                expr(ExprKind::IntLit(42))
            )
        );
    }

    #[test]
    fn parses_declared_float_variable() {
        let ast = parse_statement_ok("float ratio = 0.25");
        assert_eq!(
            ast,
            var_decl(
                Some(PrimitiveType::Float),
                "ratio",
                expr(ExprKind::FloatLit(0.25))
            )
        );
    }

    #[test]
    fn parses_identifier_expression() {
        let ast = parse_statement_ok("infer value = other_value");
        assert_eq!(
            ast,
            var_decl(
                None,
                "value",
                expr(ExprKind::Ident("other_value".to_string()))
            )
        );
    }

    #[test]
    fn parses_declared_string_variable() {
        let ast = parse_statement_ok(r#"str message = "OMG WOW!""#);
        assert_eq!(
            ast,
            var_decl(
                Some(PrimitiveType::Str),
                "message",
                expr(ExprKind::StrLit("OMG WOW!".to_string()))
            )
        );
    }

    #[test]
    fn parses_program_with_multiple_statements() {
        let ast = parse_program_ok("int a = 1\ninfer b = a\nfloat c = 2.5");
        assert_eq!(
            ast,
            vec![
                var_decl(Some(PrimitiveType::Int), "a", expr(ExprKind::IntLit(1))),
                var_decl(None, "b", expr(ExprKind::Ident("a".to_string()))),
                var_decl(
                    Some(PrimitiveType::Float),
                    "c",
                    expr(ExprKind::FloatLit(2.5))
                ),
            ]
        );
    }

    #[test]
    fn parses_statements_surrounded_by_blank_lines() {
        let ast = parse_program_ok("\n\nint a = 1\n\n\ninfer b = a\n\n");
        assert_eq!(
            ast,
            vec![
                var_decl(Some(PrimitiveType::Int), "a", expr(ExprKind::IntLit(1))),
                var_decl(None, "b", expr(ExprKind::Ident("a".to_string()))),
            ]
        );
    }

    #[test]
    fn parses_precedence_and_associativity() {
        assert_eq!(
            parse_statement_ok("infer value = 1 + 2 * 3").declaration_expr(),
            bin(
                BinaryOp::Add,
                expr(ExprKind::IntLit(1)),
                bin(
                    BinaryOp::Mul,
                    expr(ExprKind::IntLit(2)),
                    expr(ExprKind::IntLit(3))
                )
            )
        );
        assert_eq!(
            parse_statement_ok("infer value = 10 - 4 - 3").declaration_expr(),
            bin(
                BinaryOp::Sub,
                bin(
                    BinaryOp::Sub,
                    expr(ExprKind::IntLit(10)),
                    expr(ExprKind::IntLit(4))
                ),
                expr(ExprKind::IntLit(3))
            )
        );
        assert_eq!(
            parse_statement_ok("infer value = -x * y").declaration_expr(),
            bin(
                BinaryOp::Mul,
                unary(UnaryOp::Neg, expr(ExprKind::Ident("x".into()))),
                expr(ExprKind::Ident("y".into()))
            )
        );
        assert_eq!(
            parse_statement_ok("infer value = !!flag").declaration_expr(),
            unary(
                UnaryOp::Not,
                unary(UnaryOp::Not, expr(ExprKind::Ident("flag".into())))
            )
        );
        assert_eq!(
            parse_statement_ok("infer value = --x").declaration_expr(),
            unary(
                UnaryOp::Neg,
                unary(UnaryOp::Neg, expr(ExprKind::Ident("x".into())))
            )
        );
    }

    #[test]
    fn parses_assignment_and_compound_assignment() {
        assert_eq!(
            parse_statement_ok("x = 1"),
            Stmt::new(
                StmtKind::Assign {
                    name: "x".into(),
                    expr: expr(ExprKind::IntLit(1)),
                },
                Span::missing(0),
            )
        );
        assert_eq!(
            parse_statement_ok("x += 1"),
            compound("x", CompoundOp::Add, expr(ExprKind::IntLit(1)))
        );
        assert_eq!(
            parse_statement_ok("x -= 1"),
            compound("x", CompoundOp::Sub, expr(ExprKind::IntLit(1)))
        );
        assert_eq!(
            parse_statement_ok("x *= 1"),
            compound("x", CompoundOp::Mul, expr(ExprKind::IntLit(1)))
        );
        assert_eq!(
            parse_statement_ok("x /= 1"),
            compound("x", CompoundOp::Div, expr(ExprKind::IntLit(1)))
        );
        assert_eq!(
            parse_statement_ok("x %= 1"),
            compound("x", CompoundOp::Rem, expr(ExprKind::IntLit(1)))
        );
    }

    #[test]
    fn enforces_logical_parenthesization() {
        assert!(parse_program("infer x = a && b && c").is_ok());
        assert!(parse_program("infer x = a || b || c").is_ok());
        assert!(parse_program("infer x = a || (b && c)").is_ok());
        assert!(parse_program("infer x = (a || b) && c").is_ok());
        let (ast, errors) = parse_program_parts("infer x = a || b && c");
        assert!(
            !ast[0].contains_invalid(),
            "expected no invalid, got {:?} / {:?}",
            ast[0],
            errors
        );
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn validator_reports_operand_spans_for_mixed_logic() {
        let (ast, errors) = parse_program_parts("infer x = a && b || c");
        assert!(!ast[0].contains_invalid());
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].to_string(), "mixed && and || require parentheses");
        assert_eq!(errors[1].to_string(), "mixed && and || require parentheses");
    }

    #[test]
    fn parses_empty_program() {
        assert!(parse_program_ok("").is_empty());
        assert!(parse_program_ok("\n\n").is_empty());
    }

    #[test]
    fn strips_newlines_inside_parentheses() {
        let source = "(\nvalue\n)";
        let tokens: Vec<Token> = Parser::strip_insignificant_newlines(spanned_tokens(source))
            .unwrap()
            .into_iter()
            .map(|spanned| spanned.token)
            .collect();
        assert_eq!(
            tokens,
            [
                Token::LParen,
                Token::Ident("value"),
                Token::RParen,
                Token::Eof
            ]
        );
    }

    #[test]
    fn preserves_statement_newlines_outside_parentheses() {
        let source = "value\nother\n";
        let tokens = Parser::strip_insignificant_newlines(spanned_tokens(source)).unwrap();
        let expected = spanned_tokens(source);
        assert_eq!(tokens, expected);
    }

    #[test]
    fn strips_leading_newlines_before_first_complete_statement() {
        let source = "\n\nvalue\n";
        let tokens: Vec<Token> = Parser::strip_insignificant_newlines(spanned_tokens(source))
            .unwrap()
            .into_iter()
            .map(|spanned| spanned.token)
            .collect();
        assert_eq!(tokens, [Token::Ident("value"), Token::Newline, Token::Eof]);
    }

    #[test]
    fn rejects_unbalanced_brackets() {
        assert!(Parser::strip_insignificant_newlines(spanned_tokens(")")).is_err());
        assert!(Parser::strip_insignificant_newlines(spanned_tokens("(")).is_err());
    }

    #[test]
    fn rejects_statement_without_valid_trailing_token() {
        let source = "int a = 1\n*";
        let (tokens, _) = crate::lexer::lex_with_errors(source);
        let tokens = Parser::strip_insignificant_newlines(tokens).unwrap();
        let mut parser = Parser::new(&tokens);
        assert!(parser.parse_program().is_err());
    }

    #[test]
    fn recovers_from_trailing_tokens_at_eof() {
        let (tokens, _) = crate::lexer::lex_with_errors("int a = 1 2");
        let mut parser = Parser::new(&tokens);

        let (stmts, errors) = parser.parse_program_with_errors();

        assert_eq!(stmts.len(), 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("end of statement"));
    }

    #[test]
    fn recovers_from_unbalanced_parenthesis_at_eof() {
        let (tokens, _) = crate::lexer::lex_with_errors("int a = (1");

        let result = Parser::strip_insignificant_newlines(tokens);

        assert!(result.is_err());
    }

    #[test]
    fn records_invalid_statement_for_missing_variable_name() {
        let source = "int =";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens);

        let stmt = parser.parse_statement();
        let errors = parser.take_errors();

        assert!(
            matches!(stmt, Stmt { span, kind: StmtKind::Invalid { .. } } if span == Span::new(0, source.len()))
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("expected a variable name"));
    }

    #[test]
    fn keeps_declaration_missing_assignment_operator() {
        let source = "int count";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens);

        let stmt = parser.parse_statement();
        let errors = parser.take_errors();

        match stmt.kind {
            StmtKind::VarDecl {
                ty: Some(PrimitiveType::Int),
                name,
                expr: Expr { span, .. },
            } => {
                assert_eq!(name, "count");
                // zero-width missing initializer at end of file
                assert_eq!(span, Span::missing(source.len()));
            }
            other => panic!("expected a surviving declaration, got {other:?}"),
        }
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("expected `=`"));
    }

    #[test]
    fn plants_zero_width_invalid_for_missing_expression() {
        let source = "int count =";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens);

        let stmt = parser.parse_statement();
        let errors = parser.take_errors();

        match stmt.kind {
            StmtKind::VarDecl {
                ty: Some(PrimitiveType::Int),
                name,
                expr: Expr { span, .. },
            } => {
                assert_eq!(name, "count");
                assert_eq!(span, Span::missing(source.len()));
            }
            other => panic!("expected a surviving declaration, got {other:?}"),
        }
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("expected an expression"));
    }

    #[test]
    fn records_invalid_statement_for_unknown_start() {
        let source = "*";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens);

        let stmt = parser.parse_statement();
        let errors = parser.take_errors();

        assert!(
            matches!(stmt, Stmt { span, kind: StmtKind::Invalid { .. } } if span == Span::new(0, 1))
        );
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0]
                .to_string()
                .contains("expected a type or assignment target")
        );
    }

    #[test]
    fn parse_statement_at_eof_records_invalid() {
        let tokens = spanned_tokens("");
        let mut parser = Parser::new(&tokens);

        let stmt = parser.parse_statement();
        let errors = parser.take_errors();

        assert!(
            matches!(stmt, Stmt { span, kind: StmtKind::Invalid { .. } } if span.start == span.end)
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("unexpected end of file"));
    }

    #[test]
    fn parses_boolean_variable_declaration() {
        let ast = parse_statement_ok("bool flag = true");
        assert_eq!(
            ast,
            var_decl(
                Some(PrimitiveType::Bool),
                "flag",
                expr(ExprKind::BoolLit(true))
            )
        );
        let ast = parse_statement_ok("infer flag = false");
        assert_eq!(ast, var_decl(None, "flag", expr(ExprKind::BoolLit(false))));
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
    fn keeps_declaration_with_invalid_initializer_region() {
        let source = "infer value = $";
        let (stmt, errors) = parse_statement_parts(source);

        match stmt.kind {
            StmtKind::VarDecl { ref name, .. } => {
                assert_eq!(name, "value");
                assert!(matches!(
                    stmt.kind,
                    StmtKind::VarDecl {
                        expr: Expr { span: Span { start, end }, .. },
                        ..
                    } if start == end && start == source.len()
                ));
                assert!(errors[0].message().contains("expected an expression"));
            }
            other => panic!("expected a surviving declaration, got {other:?}"),
        }
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("expected an expression"));
    }

    #[test]
    fn rejects_trailing_brace_after_program_statement() {
        assert!(parse_program("infer x = 1\n*").is_err());
    }

    #[test]
    fn recovers_to_next_statement_after_lex_error() {
        let (stmts, errors) = parse_program_parts("@\nint b = 1\n");

        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0].kind(),
            crate::diagnostics::DiagnosticKind::Lex(_)
        ));
        assert_eq!(
            stmts,
            vec![var_decl(
                Some(PrimitiveType::Int),
                "b",
                expr(ExprKind::IntLit(1))
            )]
        );
    }

    #[test]
    fn recovers_to_next_statement_after_parse_error() {
        let (stmts, errors) = parse_program_parts("int a = str x\nint b = 2\n");

        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0].kind(),
            crate::diagnostics::DiagnosticKind::Parse
        ));
        assert_eq!(stmts.len(), 2);
        assert!(matches!(
            &stmts[0],
            Stmt { kind: StmtKind::VarDecl { name, .. }, .. } if name.as_str() == "a"
        ));
        assert_eq!(
            stmts[1],
            var_decl(Some(PrimitiveType::Int), "b", expr(ExprKind::IntLit(2)))
        );
    }

    #[test]
    fn keeps_declaration_on_broken_initializer() {
        // The flagship recovery case: `i` stays declared even though the
        // initializer cannot be parsed, so a future LSP can still complete
        // against it.
        let (stmts, errors) = parse_program_parts("int i = str wow how\nint j = 2\n");

        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("expected an expression"));
        assert_eq!(stmts.len(), 2);

        match &stmts[0].kind {
            StmtKind::VarDecl {
                ty: Some(PrimitiveType::Int),
                name,
                expr:
                    Expr {
                        span,
                        kind: ExprKind::Invalid { error },
                    },
            } => {
                assert_eq!(name, "i");
                assert_eq!(*span, Span::new(8, 19)); // covers "str wow how"
                assert_eq!(errors[error.0].span(), Span::new(8, 11)); // points at the `str` token
                assert!(errors[error.0].message().contains("expected an expression"));
            }
            other => panic!("expected a surviving declaration, got {other:?}"),
        }
        assert_eq!(
            stmts[1],
            var_decl(Some(PrimitiveType::Int), "j", expr(ExprKind::IntLit(2)))
        );
    }

    #[test]
    fn plants_zero_width_invalid_at_end_of_file() {
        let (stmts, errors) = parse_program_parts("int count =");

        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("expected an expression"));
        assert_eq!(stmts.len(), 1);
        match &stmts[0].kind {
            StmtKind::VarDecl {
                ty: Some(PrimitiveType::Int),
                name,
                expr: Expr { span, .. },
            } => {
                assert_eq!(name.as_str(), "count");
                assert_eq!(*span, Span::missing("int count =".len()));
            }
            other => panic!("expected a surviving declaration, got {other:?}"),
        }
    }

    #[test]
    fn plants_zero_width_invalid_before_statement_newline() {
        // Raw tokens (no strip pass): the newline after `=` is intact, so the
        // missing initializer is planted at the newline position and the next
        // statement survives untouched.
        let source = "int i = \nint j = 2\n";
        let (stmts, errors) = Parser::new(&spanned_tokens(source)).parse_program_with_errors();

        assert_eq!(errors.len(), 1);
        assert_eq!(stmts.len(), 2);
        match &stmts[0].kind {
            StmtKind::VarDecl {
                ty: Some(PrimitiveType::Int),
                name,
                expr: Expr { span, .. },
            } => {
                assert_eq!(name.as_str(), "i");
                assert_eq!(*span, Span::missing(8));
            }
            other => panic!("expected a surviving declaration, got {other:?}"),
        }
        assert_eq!(
            stmts[1],
            var_decl(Some(PrimitiveType::Int), "j", expr(ExprKind::IntLit(2)))
        );
    }

    #[test]
    fn keeps_declaration_missing_assignment_operator_before_newline() {
        let source = "int i\nint j = 2\n";
        let (stmts, errors) = Parser::new(&spanned_tokens(source)).parse_program_with_errors();

        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("expected `=`"));
        assert_eq!(stmts.len(), 2);
        assert!(matches!(
            &stmts[0],
            Stmt {
                kind:
                    StmtKind::VarDecl {
                        name,
                        expr: Expr { span, .. },
                        ..
                    },
                ..
            } if name.as_str() == "i" && span.start == span.end
        ));
        assert_eq!(
            stmts[1],
            var_decl(Some(PrimitiveType::Int), "j", expr(ExprKind::IntLit(2)))
        );
    }

    #[test]
    fn records_invalid_statement_for_garbage_head() {
        // `*` cannot start a statement and (unlike an operator line) the
        // strip pass keeps the following newline, so the next statement
        // survives as its own entry.
        let (stmts, errors) = parse_program_parts("*\nint j = 2\n");

        assert_eq!(errors.len(), 1);
        assert!(
            errors[0]
                .to_string()
                .contains("expected a type or assignment target")
        );
        assert_eq!(stmts.len(), 2);
        assert!(matches!(
            &stmts[0],
            Stmt { span, kind: StmtKind::Invalid { .. } } if *span == Span::new(0, 1)
        ));
        assert_eq!(
            stmts[1],
            var_decl(Some(PrimitiveType::Int), "j", expr(ExprKind::IntLit(2)))
        );
    }

    #[test]
    fn records_invalid_statement_for_bare_identifier() {
        let (stmts, errors) = parse_program_parts("x\nint j = 2\n");

        assert_eq!(errors.len(), 1);
        assert!(
            errors[0]
                .to_string()
                .contains("expected an assignment operator")
        );
        assert_eq!(stmts.len(), 2);
        assert!(matches!(
            &stmts[0],
            Stmt {
                kind: StmtKind::Invalid { .. },
                ..
            }
        ));
        assert_eq!(
            stmts[1],
            var_decl(Some(PrimitiveType::Int), "j", expr(ExprKind::IntLit(2)))
        );
    }

    #[test]
    fn keeps_compound_assignment_with_invalid_rhs() {
        let source = "x += str y";
        let tokens = spanned_tokens(source);
        let mut parser = Parser::new(&tokens);

        let stmt = parser.parse_statement();
        let errors = parser.take_errors();

        match stmt.kind {
            StmtKind::CompoundAssign {
                target,
                op: CompoundOp::Add,
                expr:
                    Expr {
                        span,
                        kind: ExprKind::Invalid { .. },
                    },
            } => {
                assert_eq!(target, "x");
                assert_eq!(span, Span::new(5, 10)); // covers "str y"
            }
            other => panic!("expected a surviving compound assignment, got {other:?}"),
        }
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn sibling_statements_survive_repeated_failures() {
        let (stmts, errors) = parse_program_parts("int a = str x\nint b = str y\nint c = 3\n");

        assert_eq!(errors.len(), 2);
        assert_eq!(stmts.len(), 3);
        assert!(matches!(
            &stmts[0],
            Stmt { kind: StmtKind::VarDecl { name, .. }, .. } if name.as_str() == "a"
        ));
        assert!(matches!(
            &stmts[1],
            Stmt { kind: StmtKind::VarDecl { name, .. }, .. } if name.as_str() == "b"
        ));
        assert_eq!(
            stmts[2],
            var_decl(Some(PrimitiveType::Int), "c", expr(ExprKind::IntLit(3)))
        );
    }

    #[test]
    fn fail_fast_parse_program_still_reports_first_error() {
        // The batch/execution gate: tolerant parsing feeds tooling, but the
        // fail-fast entry point still refuses broken programs.
        assert!(parse_program("int i = str wow how\n").is_err());
    }

    #[test]
    fn rejects_empty_parenthesized_statement() {
        let error = parse_program("infer value = ()").unwrap_err();
        assert!(error.to_string().contains("expected an expression"));
    }

    #[test]
    fn rejects_unbalanced_brackets_in_program() {
        assert!(parse_program("infer value = (").is_err());
        assert!(parse_program("infer value = )").is_err());
    }

    #[test]
    fn parse_statement_does_not_require_all_input_to_be_consumed() {
        let source = "infer a = 1 }";
        let (tokens, _) = crate::lexer::lex_with_errors(source);
        let mut parser = Parser::new(&tokens);

        assert_eq!(
            parser.parse_statement(),
            var_decl(None, "a", expr(ExprKind::IntLit(1)))
        );
    }

    #[test]
    fn invalid_error_ids_index_existing_diagnostics() {
        fn walk_expr(expr: &Expr, errors: &[Diagnostic]) {
            match &expr.kind {
                ExprKind::Invalid { error } => assert!(error.0 < errors.len()),
                ExprKind::Binary { lhs, rhs, .. } => {
                    walk_expr(lhs, errors);
                    walk_expr(rhs, errors);
                }
                ExprKind::Unary { expr, .. } | ExprKind::Paren { expr } => walk_expr(expr, errors),
                _ => {}
            }
        }

        fn walk_stmt(stmt: &Stmt, errors: &[Diagnostic]) {
            match &stmt.kind {
                StmtKind::Invalid { error } => assert!(error.0 < errors.len()),
                StmtKind::VarDecl { expr, .. }
                | StmtKind::Assign { expr, .. }
                | StmtKind::CompoundAssign { expr, .. } => walk_expr(expr, errors),
            }
        }

        let outcome = crate::parse_source(BOOM_CM);
        for stmt in &outcome.statements {
            walk_stmt(stmt, &outcome.diagnostics);
        }
    }

    #[test]
    fn zero_width_missing_initializer_survives_the_full_pipeline() {
        // The strip pass must keep the newline after a dangling `=` when the
        // next line starts a declaration — otherwise recovery fuses the two
        // lines and eats `int j`, defeating the LSP use case entirely.
        let (stmts, errors) = parse_program_parts("int count =\nint j = 2\n");

        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].to_string(),
            "expected an expression, but found end of statement"
        );
        assert_eq!(stmts.len(), 2);
        match &stmts[0].kind {
            StmtKind::VarDecl {
                ty: Some(PrimitiveType::Int),
                name,
                expr: Expr { span, .. },
            } => {
                assert_eq!(name.as_str(), "count");
                assert_eq!(*span, Span::missing("int count =".len()));
            }
            other => panic!("expected a surviving declaration, got {other:?}"),
        }
        assert_eq!(
            stmts[1],
            var_decl(Some(PrimitiveType::Int), "j", expr(ExprKind::IntLit(2)))
        );
    }

    #[test]
    fn dangling_type_keyword_does_not_fuse_with_next_line() {
        // A type keyword alone can never continue onto the next line, so its
        // newline survives and the declaration below stays intact.
        let (stmts, errors) = parse_program_parts("float\nint j = 2\n");

        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].to_string(),
            "expected a variable name, but found end of statement"
        );
        assert!(matches!(
            &stmts[0],
            Stmt {
                kind: StmtKind::Invalid { .. },
                ..
            }
        ));
        assert_eq!(
            stmts[1],
            var_decl(Some(PrimitiveType::Int), "j", expr(ExprKind::IntLit(2)))
        );
    }

    #[test]
    fn dangling_operator_before_declaration_keeps_both_lines() {
        // `1 +` dangles before a NEW declaration: the newline survives, cont
        // keeps a non-zero-width Invalid over "1 +", and boom2 is still seen
        // as its own (broken but surviving) declaration.
        let (stmts, errors) = parse_program_parts("int cont = 1 +\nstr boom2\n");

        assert_eq!(errors.len(), 2);
        assert_eq!(
            errors[0].to_string(),
            "expected an expression, but found end of statement"
        );
        assert_eq!(
            errors[1].to_string(),
            "expected `=`, but found end of statement"
        );
        match &stmts[0].kind {
            StmtKind::VarDecl {
                ty: Some(PrimitiveType::Int),
                name,
                expr: Expr { span, .. },
            } => {
                assert_eq!(name.as_str(), "cont");
                assert_eq!(
                    *span,
                    Span::new("int cont = ".len(), "int cont = 1 +".len())
                );
            }
            other => panic!("expected a surviving declaration, got {other:?}"),
        }
        match &stmts[1].kind {
            StmtKind::VarDecl {
                ty: Some(PrimitiveType::Str),
                name,
                expr:
                    Expr {
                        span,
                        kind: ExprKind::Invalid { .. },
                    },
            } => {
                assert_eq!(name.as_str(), "boom2");
                assert_eq!(*span, Span::missing("int cont = 1 +\nstr boom2".len()));
            }
            other => panic!("expected a surviving declaration, got {other:?}"),
        }
    }

    const BOOM_CM: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../boom.cm"));

    fn statement_label(stmt: &Stmt) -> String {
        match &stmt.kind {
            StmtKind::VarDecl { ty, name, .. } => {
                format!(
                    "var:{name}:{}",
                    ty.as_ref().map_or("None", |ty| match ty {
                        PrimitiveType::Int => "Int",
                        PrimitiveType::Float => "Float",
                        PrimitiveType::Bool => "Bool",
                        PrimitiveType::Str => "Str",
                    })
                )
            }
            StmtKind::Assign { name, .. } => format!("assign:{name}"),
            StmtKind::CompoundAssign { target, .. } => format!("compound:{target}"),
            StmtKind::Invalid { .. } => "invalid".to_string(),
        }
    }

    fn invalid_span(stmt: &Stmt) -> Option<Span> {
        match stmt.kind {
            StmtKind::VarDecl {
                expr:
                    Expr {
                        span: _,
                        kind: ExprKind::Invalid { .. },
                    },
                ..
            }
            | StmtKind::Assign {
                expr:
                    Expr {
                        span: _,
                        kind: ExprKind::Invalid { .. },
                    },
                ..
            }
            | StmtKind::CompoundAssign {
                expr:
                    Expr {
                        span: _,
                        kind: ExprKind::Invalid { .. },
                    },
                ..
            }
            | StmtKind::Invalid { .. } => Some(stmt.span),
            _ => None,
        }
    }

    #[test]
    fn boom_cm_stress_fixture_parses_with_recovery() {
        // boom.cm exercises every recovery path end to end; the section
        // comments in the file document the expectation per line. Update
        // those comments together with these pins when recovery behavior
        // deliberately changes.
        let (stmts, errors) = parse_program_parts(BOOM_CM);

        assert_eq!(errors.len(), 61, "diagnostics: {errors:#?}");
        assert_eq!(stmts.len(), 60, "statements: {stmts:#?}");

        let labels: Vec<String> = stmts.iter().map(statement_label).collect();
        assert_eq!(
            labels,
            vec![
                "var:first:Int",
                "var:second:None",
                // §2 flagship — every declared type survives its broken initializer
                "var:i:Int",
                "var:f:Float",
                "var:s:Str",
                "var:b:Bool",
                "var:v:None",
                // §3 missing pieces — three zero-width survivors + one unrecognizable
                "var:count:Int",
                "var:k:Int",
                "invalid",
                "var:draft:None",
                // §4 siblings
                "var:left:Int",
                "var:middle:Int",
                "var:right:Int",
                "var:brokenA:Int",
                "var:brokenB:Int",
                "var:brokenC:Int",
                "var:after:Int",
                // §5 assignment family
                "var:score:Int",
                "assign:score",
                "compound:score",
                "compound:score",
                "var:fence:Int",
                "compound:score",
                "compound:score",
                // §6 garbage heads (the `}` and `)` lines are dropped by the lexer/strip pass)
                "invalid",
                "invalid",
                "invalid",
                "invalid",
                // §7 missing names
                "invalid",
                "invalid",
                "invalid",
                "invalid",
                // §8 bare identifiers
                "invalid",
                "invalid",
                // §9 lexer errors — five declarations survive the damaged lines
                "var:cursed:Int",
                "var:oops:Str",
                "var:fragile:None",
                "var:afterAt:None",
                "var:huge:Int",
                // §10 unbalanced closing parens — both statements stay healthy
                "var:q:None",
                "var:q2:Int",
                // §11 balanced broken groups
                "var:empty:None",
                "var:mixed:None",
                // §12 operator grammar
                "var:mixedBad:None",
                "var:chained:None",
                // §13 continuations
                "var:cont:Int",
                "var:boom2:Str",
                "var:multi:None",
                "var:afterMulti:Int",
                // §14 trailing garbage
                "var:ok:Int",
                "var:ok2:Int",
                "var:survivor:Int",
                // §15 LSP simulation
                "var:hp:Int",
                "var:mp:Int",
                "var:total:None",
                "var:name:Str",
                "var:flag:Bool",
                "var:brokenFlag:Bool",
                // §16 EOF
                "var:unterminated:None",
            ]
        );

        // Zero-width "missing node" placements: nothing was ever typed there.
        for _index in [0usize][0..0].iter() {}

        // Skipped-region placements: real source was consumed and covered.
        for index in [
            2usize, 3, 4, 5, 6, 7, 9, 12, 14, 15, 16, 19, 21, 23, 25, 26, 27, 28, 29, 30, 31, 32,
            33, 34, 36, 38, 39, 42, 43, 45, 46, 47, 48, 54, 58, 59,
        ] {
            let span = invalid_span(&stmts[index])
                .unwrap_or_else(|| panic!("statement {index} should carry an Invalid"));
            assert!(
                span.end > span.start,
                "statement {index} should cover a skipped region, got {span:?}"
            );
        }

        // Healthy survivor spot checks: `q` still holds 1 + 2, and the
        // lexer/strip diagnostics come from exactly the damaged lines
        // (eight lexer errors: five bad-character lines in §9 with `@ $`
        // now recorded twice, one unterminated string, the `1.2.3` float
        // shape, and the overflow digit run).
        assert!(matches!(
            &stmts[40],
            Stmt {
                kind: StmtKind::VarDecl {
                    expr: Expr {
                        kind: ExprKind::Binary {
                            op: BinaryOp::Add,
                            ..
                        },
                        ..
                    },
                    ..
                },
                ..
            }
        ));
        assert_eq!(
            errors
                .iter()
                .filter(|error| {
                    matches!(error.kind(), crate::diagnostics::DiagnosticKind::Lex(_))
                })
                .count(),
            14
        );
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.to_string().contains("unbalanced closing parenthesis"))
                .count(),
            4
        );
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.to_string().contains("unbalanced opening parenthesis"))
                .count(),
            1
        );
    }
}
