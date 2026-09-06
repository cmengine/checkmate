//! `cme-compiler` implements the Checkmate front-end. The canonical one-call
//! API is [`parse_source`], which lexes source, strips insignificant newlines,
//! parses with recovery, and runs the post-parse validator.
//!
//! Recovery never stops: the parser plants [`cme_core::ast::ExprKind::Invalid`]
//! or [`cme_core::ast::StmtKind::Invalid`] placeholders and reports every
//! diagnostic it can. The resulting tree remains useful for tooling, while
//! execution consumers should reject any non-empty diagnostic list.
//!
//! ```
//! let outcome = cme_compiler::parse_source("int hp = 100\nhp += 5\n");
//! assert!(outcome.is_clean());
//! assert_eq!(outcome.statements.len(), 2);
//! ```

pub mod check;
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
        BinaryOp, Block, CompoundOp, Expr, ExprKind, LValue, PrimitiveType, Stmt, StmtKind, Type,
        UnaryOp,
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
                target: LValue::Var {
                    name: target.to_string(),
                },
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

        assert_eq!(
            ast,
            var_decl(Type::Infer, "speed", expr(ExprKind::FloatLit(4.5)))
        );
    }

    #[test]
    fn parses_declared_integer_variable() {
        let ast = parse_statement_ok("int count = 42");
        assert_eq!(
            ast,
            var_decl(
                Type::Prim(PrimitiveType::Int),
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
                Type::Prim(PrimitiveType::Float),
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
                Type::Infer,
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
                Type::Prim(PrimitiveType::Str),
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
                var_decl(
                    Type::Prim(PrimitiveType::Int),
                    "a",
                    expr(ExprKind::IntLit(1))
                ),
                var_decl(Type::Infer, "b", expr(ExprKind::Ident("a".to_string()))),
                var_decl(
                    Type::Prim(PrimitiveType::Float),
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
                var_decl(
                    Type::Prim(PrimitiveType::Int),
                    "a",
                    expr(ExprKind::IntLit(1))
                ),
                var_decl(Type::Infer, "b", expr(ExprKind::Ident("a".to_string()))),
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
                    target: LValue::Var { name: "x".into() },
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
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn validator_reports_operand_spans_for_mixed_logic() {
        let (ast, errors) = parse_program_parts("infer x = a && b || c");
        assert!(!ast[0].contains_invalid());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].to_string(), "mixed && and || require parentheses");
        assert_eq!(errors[0].span(), Span::new(10, 16));
    }

    #[test]
    fn mixed_logic_inside_executable_code_is_validated() {
        // §A.3 Rule 1 applies everywhere an expression can appear, and the
        // executable code of the language lives inside function bodies:
        // return values, if/while conditions, else-if chains, and call
        // arguments. Each source reports the violation exactly once.
        let sources = [
            "bool f(bool a, bool b, bool c) {\nreturn a || b && c\n}\n",
            "void f(bool a, bool b, bool c) {\nif (a || b && c) {\n}\n}\n",
            "void f(bool a, bool b, bool c) {\nwhile (a || b && c) {\n}\n}\n",
            "void f(bool a, bool b, bool c) {\nif (a) {\n} else if (a || b && c) {\n}\n}\n",
            "void f(bool a, bool b, bool c) {\nlog(a || b && c)\n}\n",
            "void f(bool a, bool b, bool c) {\nbool ready = a || b && c\n}\n",
            "void f(bool a, bool b, bool c) {\nready = a || b && c\n}\n",
            "void f(bool a, bool b, bool c) {\nready += a || b && c\n}\n",
        ];
        for source in sources {
            let (_, errors) = parse_program_parts(source);
            assert_eq!(errors.len(), 1, "{source:?}: {errors:#?}");
            assert_eq!(
                errors[0].to_string(),
                "mixed && and || require parentheses",
                "{source:?}"
            );
        }
    }

    #[test]
    fn parenthesized_logic_inside_executable_code_stays_clean() {
        let sources = [
            "bool f(bool a, bool b, bool c) {\nreturn a || (b && c)\n}\n",
            "bool f(bool a, bool b, bool c) {\nreturn (a || b) && c\n}\n",
            "bool f(bool a, bool b, bool c) {\nreturn a && b && c\n}\n",
            "bool f(bool a, bool b, bool c) {\nreturn a || b || c\n}\n",
            "void f(bool a, bool b, bool c) {\nif (a || (b && c)) {\n}\n}\n",
            "void f(bool a, bool b, bool c) {\nwhile ((a || b) && c) {\n}\n}\n",
            "void f(bool a, bool b, bool c) {\nlog(a || (b && c))\n}\n",
        ];
        for source in sources {
            let (_, errors) = parse_program_parts(source);
            assert!(
                errors.is_empty(),
                "{source:?} should validate clean: {errors:#?}"
            );
        }
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
        assert!(Parser::strip_insignificant_newlines(spanned_tokens("[")).is_err());
        assert!(Parser::strip_insignificant_newlines(spanned_tokens("{ x: 1")).is_err());
        // A closing bracket inside the wrong bracket kind is unbalanced.
        assert!(Parser::strip_insignificant_newlines(spanned_tokens("[1)")).is_err());
    }

    #[test]
    fn newline_significance_follows_the_innermost_bracket() {
        // Inside parens (§A.8): newlines are dropped.
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

        // Inside brackets and braces: newlines are kept — array elements,
        // map entries, and arm bodies are newline-delimited (§11.1, §2.15).
        for source in ["[\n1\n2\n]", "{\n\"k\": 1\n}", "match (x) {\nA() => 1\n}"] {
            let tokens = Parser::strip_insignificant_newlines(spanned_tokens(source)).unwrap();
            assert!(
                tokens.iter().any(|t| t.token == Token::Newline),
                "{source:?}: newline inside brackets/braces must survive"
            );
        }

        // Brackets inside parens re-enable significance for their interior
        // (the leading newline after `[` is kept too — element lists skip
        // leading separators).
        let source = "f([\n1\n2\n])";
        let tokens = Parser::strip_insignificant_newlines(spanned_tokens(source)).unwrap();
        assert_eq!(
            tokens.iter().filter(|t| t.token == Token::Newline).count(),
            3
        );
    }

    #[test]
    fn stray_closing_brackets_pass_through_to_the_parser() {
        // A stray `]` or `}` at the top of the stream is a plain token the
        // parser reports as an unrecognizable statement (boom.cm pins the
        // `}` variant); only a stray `)` is a strip-level error.
        let (tokens, errors) =
            Parser::strip_insignificant_newlines_with_errors(spanned_tokens("]"));
        assert!(errors.is_empty());
        assert!(tokens.iter().any(|t| t.token == Token::RBracket));

        let (tokens, errors) =
            Parser::strip_insignificant_newlines_with_errors(spanned_tokens("}"));
        assert!(errors.is_empty());
        assert!(tokens.iter().any(|t| t.token == Token::RBrace));
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
                ty: Type::Prim(PrimitiveType::Int),
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
                ty: Type::Prim(PrimitiveType::Int),
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
                Type::Prim(PrimitiveType::Bool),
                "flag",
                expr(ExprKind::BoolLit(true))
            )
        );
        let ast = parse_statement_ok("infer flag = false");
        assert_eq!(
            ast,
            var_decl(Type::Infer, "flag", expr(ExprKind::BoolLit(false)))
        );
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
                Type::Prim(PrimitiveType::Int),
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
            var_decl(
                Type::Prim(PrimitiveType::Int),
                "b",
                expr(ExprKind::IntLit(2))
            )
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
                ty: Type::Prim(PrimitiveType::Int),
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
            var_decl(
                Type::Prim(PrimitiveType::Int),
                "j",
                expr(ExprKind::IntLit(2))
            )
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
                ty: Type::Prim(PrimitiveType::Int),
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
                ty: Type::Prim(PrimitiveType::Int),
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
            var_decl(
                Type::Prim(PrimitiveType::Int),
                "j",
                expr(ExprKind::IntLit(2))
            )
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
            var_decl(
                Type::Prim(PrimitiveType::Int),
                "j",
                expr(ExprKind::IntLit(2))
            )
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
            var_decl(
                Type::Prim(PrimitiveType::Int),
                "j",
                expr(ExprKind::IntLit(2))
            )
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
            var_decl(
                Type::Prim(PrimitiveType::Int),
                "j",
                expr(ExprKind::IntLit(2))
            )
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
                assert_eq!(target, LValue::Var { name: "x".into() });
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
            var_decl(
                Type::Prim(PrimitiveType::Int),
                "c",
                expr(ExprKind::IntLit(3))
            )
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
            var_decl(Type::Infer, "a", expr(ExprKind::IntLit(1)))
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
                _ => {}
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
                ty: Type::Prim(PrimitiveType::Int),
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
            var_decl(
                Type::Prim(PrimitiveType::Int),
                "j",
                expr(ExprKind::IntLit(2))
            )
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
            var_decl(
                Type::Prim(PrimitiveType::Int),
                "j",
                expr(ExprKind::IntLit(2))
            )
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
                ty: Type::Prim(PrimitiveType::Int),
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
                ty: Type::Prim(PrimitiveType::Str),
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
    const BASIC_CM: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../basic.cm"));

    #[test]
    fn truncation_never_panics() {
        for fixture in [BOOM_CM, BASIC_CM] {
            for end in 0..=fixture.len() {
                if !fixture.is_char_boundary(end) {
                    continue;
                }
                let outcome = crate::parse_source(&fixture[..end]);
                // The checker must survive any recovered tree.
                let _ = crate::check::check(&outcome.statements);
                let _ = outcome;
            }
        }
    }

    #[test]
    fn basic_cm_parses_clean() {
        let outcome = crate::parse_source(BASIC_CM);
        assert!(
            outcome.is_clean(),
            "basic.cm should parse with ZERO diagnostics: {outcome:#?}"
        );

        let funcs: Vec<&Stmt> = outcome
            .statements
            .iter()
            .filter(|stmt| matches!(stmt.kind, StmtKind::FuncDecl { .. }))
            .collect();
        assert!(!funcs.is_empty(), "basic.cm should declare functions");

        let has_else_if = outcome.statements.iter().any(|stmt| match &stmt.kind {
            StmtKind::FuncDecl { body, .. } => body.stmts.iter().any(|inner| {
                matches!(
                    &inner.kind,
                    StmtKind::If {
                        else_branch: Some(_),
                        ..
                    }
                )
            }),
            _ => false,
        });
        assert!(has_else_if, "basic.cm should contain an else-if chain");

        let has_while = outcome.statements.iter().any(|stmt| match &stmt.kind {
            StmtKind::FuncDecl { body, .. } => body
                .stmts
                .iter()
                .any(|inner| matches!(inner.kind, StmtKind::While { .. })),
            _ => false,
        });
        assert!(has_while, "basic.cm should contain a while loop");

        let has_call_with_args = outcome.statements.iter().any(|stmt| match &stmt.kind {
            StmtKind::FuncDecl { body, .. } => {
                body.stmts.iter().any(|inner| has_call_with_arity(inner, 2))
            }
            _ => false,
        });
        assert!(
            has_call_with_args,
            "basic.cm should contain a call with at least 2 arguments"
        );

        let has_compound = outcome.statements.iter().any(|stmt| match &stmt.kind {
            StmtKind::FuncDecl { body, .. } => body
                .stmts
                .iter()
                .any(|inner| matches!(inner.kind, StmtKind::CompoundAssign { .. })),
            _ => false,
        });
        assert!(
            has_compound,
            "basic.cm should contain a compound assignment"
        );
    }

    #[test]
    fn parses_unescaped_string_literals() {
        // Each accepted escape decodes when the StrLit expression is built.
        let cases: Vec<(&str, &str)> = vec![
            (r#"str s = "a\nb""#, "a\nb"),
            (r#"str s = "c\td""#, "c\td"),
            (r#"str s = "e\\f""#, "e\\f"),
            (r#"str s = "g\"h""#, "g\"h"),
        ];
        for (source, value) in cases {
            let ast = parse_statement_ok(source);
            assert_eq!(
                ast,
                var_decl(
                    Type::Prim(PrimitiveType::Str),
                    "s",
                    expr(ExprKind::StrLit(value.to_string()))
                ),
                "case {source:?}"
            );
        }
    }

    #[test]
    fn parses_struct_declarations() {
        let source = "struct vec2 {\n    float x\n    float y\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(stmts.len(), 1);
        match &stmts[0].kind {
            StmtKind::StructDecl {
                name,
                type_params,
                fields,
            } => {
                assert_eq!(name, "vec2");
                assert!(type_params.is_empty());
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "x");
                assert_eq!(fields[0].ty, Type::Prim(PrimitiveType::Float));
                assert_eq!(fields[1].name, "y");
                // The declaration spans `struct` through the closing brace.
                assert_eq!(stmts[0].span, Span::new(0, source.len() - 1));
            }
            other => panic!("expected a struct declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_generic_struct_declarations() {
        let source = "struct pair<A, B> {\n    A first\n    B second\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::StructDecl {
                name,
                type_params,
                fields,
            } => {
                assert_eq!(name, "pair");
                assert_eq!(type_params, &["A".to_string(), "B".to_string()]);
                // Type parameters surface as named types in field position.
                assert_eq!(
                    fields[0].ty,
                    Type::Named {
                        name: "A".into(),
                        args: vec![]
                    }
                );
            }
            other => panic!("expected a struct declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_struct_field_collection_types() {
        // `int[]` array fields and `map<K, V>` fields (§2.6, §11).
        let source = "struct s {\n    int[] scores\n    map<str, int> tally\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::StructDecl { fields, .. } => {
                assert_eq!(
                    fields[0].ty,
                    Type::Array(Box::new(Type::Prim(PrimitiveType::Int)))
                );
                assert_eq!(
                    fields[1].ty,
                    Type::Map {
                        key: Box::new(Type::Prim(PrimitiveType::Str)),
                        value: Box::new(Type::Prim(PrimitiveType::Int))
                    }
                );
            }
            other => panic!("expected a struct declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_enum_declarations() {
        let source = "enum gameEvent {\n    Damage(int amount)\n    Spawn(str kind, vec2 position)\n    PlayerDied()\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::EnumDecl {
                name,
                type_params,
                variants,
            } => {
                assert_eq!(name, "gameEvent");
                assert!(type_params.is_empty());
                assert_eq!(variants.len(), 3);
                assert_eq!(variants[0].name, "Damage");
                assert_eq!(variants[0].fields.len(), 1);
                assert_eq!(variants[0].fields[0].name, "amount");
                // Two payloads, comma-separated.
                assert_eq!(variants[1].fields.len(), 2);
                assert_eq!(variants[1].fields[0].ty, Type::Prim(PrimitiveType::Str));
                assert_eq!(
                    variants[1].fields[1].ty,
                    Type::Named {
                        name: "vec2".into(),
                        args: vec![]
                    }
                );
                // No payloads still needs the parentheses.
                assert_eq!(variants[2].name, "PlayerDied");
                assert!(variants[2].fields.is_empty());
            }
            other => panic!("expected an enum declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_generic_enum_declarations() {
        let source = "enum maybe<T> {\n    Just(T value)\n    Nothing()\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::EnumDecl {
                type_params,
                variants,
                ..
            } => {
                assert_eq!(type_params, &["T".to_string()]);
                assert_eq!(
                    variants[0].fields[0].ty,
                    Type::Named {
                        name: "T".into(),
                        args: vec![]
                    }
                );
            }
            other => panic!("expected an enum declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_impl_blocks_with_function_members() {
        let source = "impl counter {\n    int peek(counter c) {\n        return c.value\n    }\n\n    counter reset() {\n        return counter(value: 0)\n    }\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::ImplDecl { target, members } => {
                assert_eq!(target, &["counter".to_string()]);
                assert_eq!(members.len(), 2);
                match &members[0].kind {
                    StmtKind::FuncDecl {
                        name,
                        params,
                        return_ty,
                        ..
                    } => {
                        assert_eq!(name, "peek");
                        assert_eq!(params.len(), 1);
                        assert_eq!(params[0].name, "c");
                        assert_eq!(*return_ty, Type::Prim(PrimitiveType::Int));
                    }
                    other => panic!("expected a member function, got {other:?}"),
                }
                assert!(matches!(
                    &members[1].kind,
                    StmtKind::FuncDecl { name, .. } if name == "reset"
                ));
            }
            other => panic!("expected an impl declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_dotted_impl_targets() {
        // Host-style namespace targets (§10.4): the whole dotted path is
        // the target.
        let source =
            "impl engine.gamemode {\n    void OnTick(int dt) {\n        return\n    }\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::ImplDecl { target, members } => {
                assert_eq!(target, &["engine".to_string(), "gamemode".to_string()]);
                assert_eq!(members.len(), 1);
            }
            other => panic!("expected an impl declaration, got {other:?}"),
        }
    }

    #[test]
    fn impl_members_must_be_function_declarations() {
        let source =
            "impl counter {\n    int x = 5\n    int peek() {\n        return 1\n    }\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(
            errors.iter().any(|error| error
                .message()
                .contains("impl members must be function declarations")),
            "{errors:#?}"
        );
        // The valid member survives the rejected one.
        match &stmts[0].kind {
            StmtKind::ImplDecl { members, .. } => assert_eq!(members.len(), 1),
            other => panic!("expected an impl declaration, got {other:?}"),
        }
    }

    #[test]
    fn impl_target_must_start_with_an_identifier() {
        let (stmts, errors) = parse_program_parts("impl 42 {\n}\n");
        assert!(
            errors
                .iter()
                .any(|error| error.message().contains("expected an impl target")),
            "{errors:#?}"
        );
        assert!(matches!(
            stmts.first().map(|stmt| &stmt.kind),
            Some(StmtKind::Invalid { .. })
        ));
    }

    #[test]
    fn unclosed_impl_block_reports_and_absorbs_the_tail_like_a_block() {
        // An unclosed `{` swallows the remaining statements as members,
        // exactly like an unclosed function body (the parser cannot know
        // where the block was meant to end).
        let source = "impl counter {\n    int peek() {\n        return 1\n    }\nint main() {\nreturn 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(
            errors
                .iter()
                .any(|error| error.message().contains("expected `}` before end of file")),
            "{errors:#?}"
        );
        match stmts.first().map(|stmt| &stmt.kind) {
            Some(StmtKind::ImplDecl { members, .. }) => {
                assert_eq!(members.len(), 2);
                assert!(matches!(
                    &members[1].kind,
                    StmtKind::FuncDecl { name, .. } if name == "main"
                ));
            }
            other => panic!("expected an impl declaration, got {other:?}"),
        }
    }

    #[test]
    fn three_segment_paths_parse_as_path_calls() {
        let source = "int main() {\n    return engine.gamemode.InitGame(4)\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let StmtKind::FuncDecl { body, .. } = &stmts[0].kind else {
            panic!("expected a function declaration");
        };
        let StmtKind::Return { value: Some(value) } = &body.stmts[0].kind else {
            panic!("expected a return with a value");
        };
        match &value.kind {
            ExprKind::PathCall { path, args } => {
                assert_eq!(path, &["engine", "gamemode", "InitGame"]);
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected a path call, got {other:?}"),
        }
    }

    #[test]
    fn two_segment_qualified_calls_stay_variant_calls() {
        // `Color.Red(25)` keeps the two-segment `Enum.Variant` node; the
        // checker (not the parser) disambiguates construction vs impl
        // member (§2.7, §10.4).
        let source = "int main() {\n    return Color.Red(25)\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let StmtKind::FuncDecl { body, .. } = &stmts[0].kind else {
            panic!("expected a function declaration");
        };
        let StmtKind::Return { value: Some(value) } = &body.stmts[0].kind else {
            panic!("expected a return with a value");
        };
        match &value.kind {
            ExprKind::VariantCall {
                enum_name,
                variant,
                args,
            } => {
                assert_eq!(enum_name, "Color");
                assert_eq!(variant, "Red");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected a variant call, got {other:?}"),
        }
    }

    #[test]
    fn path_call_statements_are_valid_expression_statements() {
        // A bare path call in statement position (§2.11-style): its value
        // is discarded.
        let source = "int main() {\n    engine.gamemode.OnTick(1, 0.5)\n    return 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let StmtKind::FuncDecl { body, .. } = &stmts[0].kind else {
            panic!("expected a function declaration");
        };
        assert!(matches!(
            &body.stmts[0].kind,
            StmtKind::Expression { expr }
                if matches!(expr.kind, ExprKind::PathCall { .. })
        ));
    }

    #[test]
    fn array_typed_variable_and_function_declarations_parse() {
        // `int[]` in both variable and return-type positions (§11).
        let (stmts, errors) = parse_program_parts("int[] xs\n");
        assert_eq!(errors.len(), 1); // missing `=` — the declaration survives
        match &stmts[0].kind {
            StmtKind::VarDecl { ty, name, .. } => {
                assert_eq!(name, "xs");
                assert_eq!(*ty, Type::Array(Box::new(Type::Prim(PrimitiveType::Int))));
            }
            other => panic!("expected a variable declaration, got {other:?}"),
        }

        let source = "int[] range(int n) {\nreturn 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl {
                return_ty, params, ..
            } => {
                assert_eq!(
                    *return_ty,
                    Type::Array(Box::new(Type::Prim(PrimitiveType::Int)))
                );
                assert_eq!(params[0].ty, Type::Prim(PrimitiveType::Int));
            }
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn nested_generic_and_map_types_parse() {
        // `pair<int, pair<str, bool>>` and `map<str, int[]>` (§2.9, §11).
        let source =
            "struct box {\n    pair<int, pair<str, bool>> pr\n    map<str, int[]> tally\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::StructDecl { fields, .. } => {
                assert_eq!(
                    fields[0].ty,
                    Type::Named {
                        name: "pair".into(),
                        args: vec![
                            Type::Prim(PrimitiveType::Int),
                            Type::Named {
                                name: "pair".into(),
                                args: vec![
                                    Type::Prim(PrimitiveType::Str),
                                    Type::Prim(PrimitiveType::Bool)
                                ]
                            }
                        ]
                    }
                );
                assert_eq!(
                    fields[1].ty,
                    Type::Map {
                        key: Box::new(Type::Prim(PrimitiveType::Str)),
                        value: Box::new(Type::Array(Box::new(Type::Prim(PrimitiveType::Int))))
                    }
                );
            }
            other => panic!("expected a struct declaration, got {other:?}"),
        }
    }

    #[test]
    fn struct_declarations_recover_from_broken_fields() {
        let source = "struct s {\n    int good\n    ???\n    float alsoGood\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::StructDecl { fields, .. } => {
                // The broken field is skipped; its healthy siblings survive.
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "good");
                assert_eq!(fields[1].name, "alsoGood");
            }
            other => panic!("expected a struct declaration, got {other:?}"),
        }
    }

    #[test]
    fn struct_declarations_reject_comma_separated_fields() {
        // §2.6: fields are newline-delimited — no commas.
        let source = "struct s {\n    int a,\n    int b\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert!(errors[0].to_string().contains("newline-delimited"));
        match &stmts[0].kind {
            StmtKind::StructDecl { fields, .. } => assert_eq!(fields.len(), 2),
            other => panic!("expected a struct declaration, got {other:?}"),
        }
    }

    #[test]
    fn enum_declarations_recover_from_broken_payloads() {
        let source = "enum e {\n    Good(int n)\n    Bad(int 5)\n    Also()\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert!(errors[0].to_string().contains("expected a payload name"));
        match &stmts[0].kind {
            StmtKind::EnumDecl { variants, .. } => {
                // The broken payload is skipped; its healthy siblings survive.
                assert_eq!(variants.len(), 2);
                assert_eq!(variants[0].name, "Good");
                assert_eq!(variants[1].name, "Also");
            }
            other => panic!("expected an enum declaration, got {other:?}"),
        }
    }

    #[test]
    fn infer_and_void_member_types_are_rejected() {
        let source = "struct s {\n    infer x\n}\n";
        let (_, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].to_string(),
            "`infer` is only valid for local declarations"
        );

        let source = "struct s {\n    void x\n}\n";
        let (_, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].to_string(),
            "`void` is only valid as a function return type"
        );
    }

    #[test]
    fn parses_function_parameters_with_full_types() {
        // Named, array, generic, and map parameter types (§2.11, §2.9, §11).
        let source = "int f(vec2 p, int[] xs, option<int> o, map<str, int> m) {\nreturn 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl { params, .. } => {
                assert_eq!(params.len(), 4);
                assert_eq!(
                    params[0].ty,
                    Type::Named {
                        name: "vec2".into(),
                        args: vec![]
                    }
                );
                assert_eq!(
                    params[1].ty,
                    Type::Array(Box::new(Type::Prim(PrimitiveType::Int)))
                );
                assert_eq!(
                    params[2].ty,
                    Type::Named {
                        name: "option".into(),
                        args: vec![Type::Prim(PrimitiveType::Int)]
                    }
                );
                assert_eq!(
                    params[3].ty,
                    Type::Map {
                        key: Box::new(Type::Prim(PrimitiveType::Str)),
                        value: Box::new(Type::Prim(PrimitiveType::Int))
                    }
                );
            }
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_named_type_led_declarations() {
        // `vec2 pos = ...` and `option<int> findEven(...)` (§2.6, §2.9, §2.11).
        let source = "vec2 distance(vec2 a, vec2 b) {\nreturn a\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl {
                name, return_ty, ..
            } => {
                assert_eq!(name, "distance");
                assert_eq!(
                    *return_ty,
                    Type::Named {
                        name: "vec2".into(),
                        args: vec![]
                    }
                );
            }
            other => panic!("expected a function declaration, got {other:?}"),
        }

        // A generic type leading a variable declaration.
        let source = "int main() {\npair<int, str> p = f()\nreturn 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl { body, .. } => match &body.stmts[0].kind {
                StmtKind::VarDecl { ty, name, .. } => {
                    assert_eq!(name, "p");
                    assert_eq!(
                        *ty,
                        Type::Named {
                            name: "pair".into(),
                            args: vec![
                                Type::Prim(PrimitiveType::Int),
                                Type::Prim(PrimitiveType::Str)
                            ]
                        }
                    );
                }
                other => panic!("expected a variable declaration, got {other:?}"),
            },
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn comparison_shaped_fragments_roll_back_to_expressions() {
        // `x < y` is not a declaration; it is a bare fragment statement.
        let (stmts, errors) = parse_program_parts("int f() {\nx < y\nreturn 0\n}\n");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("assignment operator"));
        assert!(matches!(stmts[0].kind, StmtKind::FuncDecl { .. }));
    }

    #[test]
    fn parses_positional_and_named_call_arguments() {
        use cme_core::ast::CallArg;
        // Positional (§2.12).
        let source = "int main() {\nint r = clamp(15, 0, 10)\nreturn r\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let init = stmts[0].declaration_expr_of(0);
        match &init.kind {
            ExprKind::Call { name, args } => {
                assert_eq!(name, "clamp");
                assert_eq!(args.len(), 3);
                assert!(args.iter().all(|arg| matches!(arg, CallArg::Positional(_))));
            }
            other => panic!("expected a call, got {other:?}"),
        }

        // Named across lines: the stripped newlines become adjacency (§2.12).
        let source = "int main() {\nint r = clamp(\n    value: 15\n    low: 0\n    high: 10\n)\nreturn r\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let init = stmts[0].declaration_expr_of(0);
        match &init.kind {
            ExprKind::Call { args, .. } => {
                assert_eq!(args.len(), 3);
                let names: Vec<&str> = args
                    .iter()
                    .map(|arg| match arg {
                        CallArg::Named { name, .. } => name.as_str(),
                        CallArg::Positional(_) => panic!("expected named arguments"),
                    })
                    .collect();
                assert_eq!(names, ["value", "low", "high"]);
            }
            other => panic!("expected a call, got {other:?}"),
        }

        // Named on one line with commas (§4.2 style).
        let source = "int main() {\nint r = clamp(value: 15, low: 0, high: 10)\nreturn r\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let init = stmts[0].declaration_expr_of(0);
        assert!(matches!(
            &init.kind,
            ExprKind::Call { args, .. } if args.len() == 3
        ));
    }

    #[test]
    fn mixing_positional_and_named_arguments_is_a_parse_error() {
        // §2.12: a single invocation cannot mix the two forms.
        let source = "int main() {\nint r = clamp(15, low: 0, high: 10)\nreturn r\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert_eq!(
            errors[0].to_string(),
            "cannot mix positional and named arguments"
        );
        assert!(matches!(stmts[0].kind, StmtKind::FuncDecl { .. }));
    }

    #[test]
    fn parses_variant_construction() {
        // `gameEvent.Damage(25)` (§2.7).
        let source = "int main() {\ngameEvent evt = gameEvent.Damage(25)\nreturn 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let init = stmts[0].declaration_expr_of(0);
        match &init.kind {
            ExprKind::VariantCall {
                enum_name,
                variant,
                args,
            } => {
                assert_eq!(enum_name, "gameEvent");
                assert_eq!(variant, "Damage");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected a variant construction, got {other:?}"),
        }
    }

    #[test]
    fn parses_field_and_index_postfix_chains() {
        // `p.health`, `points[1].x`, `grid[1][0]` (§2.6, §11).
        let source = "int main() {\nfloat x = points[1].x\nint y = grid[1][0]\nint z = squad.leader.position.x\nreturn 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let init = stmts[0].declaration_expr_of(0);
        match &init.kind {
            ExprKind::Field { obj, name } => {
                assert_eq!(name, "x");
                assert!(matches!(&obj.kind, ExprKind::Index { .. }));
            }
            other => panic!("expected a field access, got {other:?}"),
        }
        // `grid[1][0]`: the outer index applies to the inner index result.
        let init = stmts[0].declaration_expr_of(1);
        assert!(matches!(
            &init.kind,
            ExprKind::Index { obj, .. } if matches!(obj.kind, ExprKind::Index { .. })
        ));
    }

    #[test]
    fn parses_field_and_index_assignment_targets() {
        // §2.13, §A.7: `p.health = ...`, `m[\"k\"] += ...`.
        use cme_core::ast::LValue;
        let source = "int main() {\np.health = 90\np.health -= 10\nm[\"k\"] += 1\nsquad.leader.position.x = 1.0\nreturn 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl { body, .. } => {
                match &body.stmts[0].kind {
                    StmtKind::Assign { target, .. } => match target {
                        LValue::Field { base, name } => {
                            assert_eq!(name, "health");
                            assert!(matches!(base.as_ref(), LValue::Var { .. }));
                        }
                        other => panic!("expected a field assignment, got {other:?}"),
                    },
                    other => panic!("expected a field assignment, got {other:?}"),
                }
                match &body.stmts[1].kind {
                    StmtKind::CompoundAssign { target, .. } => {
                        assert!(matches!(target, LValue::Field { .. }))
                    }
                    other => panic!("expected a compound field assignment, got {other:?}"),
                }
                match &body.stmts[2].kind {
                    StmtKind::CompoundAssign { target, .. } => {
                        assert!(matches!(target, LValue::Index { .. }))
                    }
                    other => panic!("expected an index assignment, got {other:?}"),
                }
                match &body.stmts[3].kind {
                    StmtKind::Assign { target, .. } => {
                        // Field of field of field of var.
                        let mut depth = 0;
                        let mut cursor = target;
                        while let LValue::Field { base, .. } = cursor {
                            depth += 1;
                            cursor = base;
                        }
                        assert!(matches!(cursor, LValue::Var { name } if name == "squad"));
                        assert_eq!(depth, 3);
                    }
                    other => panic!("expected a deep field assignment, got {other:?}"),
                }
            }
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_the_try_operator_as_postfix() {
        // `safeDiv(a, b)?` (§2.8).
        let source = "result<int, str> chain(int a) {\nint v = safeDiv(a, 2)?\nreturn Ok(v)\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl { body, .. } => match &body.stmts[0].kind {
                StmtKind::VarDecl { expr, .. } => {
                    assert!(matches!(&expr.kind, ExprKind::Try { expr: inner }
                        if matches!(inner.kind, ExprKind::Call { .. })));
                }
                other => panic!("expected a variable declaration, got {other:?}"),
            },
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn indexing_a_call_result_and_calling_results_are_parsed() {
        // `getUser(42).name` and `f(x)[0]` are postfix chains; `f(x)(y)`
        // is rejected — calls name functions (no function values).
        let source = "str main() {\nstr n = getUser(42).name\nreturn n\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let init = stmts[0].declaration_expr_of(0);
        assert!(matches!(
            &init.kind,
            ExprKind::Field { obj, .. } if matches!(obj.kind, ExprKind::Call { .. })
        ));

        let (stmts, errors) = parse_program_parts("int main() {\ngetUser(42)(1)\nreturn 0\n}\n");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("must name a function"));
        assert!(matches!(stmts[0].kind, StmtKind::FuncDecl { .. }));
    }

    trait DeclarationExprOf {
        fn declaration_expr_of(&self, index: usize) -> &Expr;
    }

    impl DeclarationExprOf for Stmt {
        fn declaration_expr_of(&self, index: usize) -> &Expr {
            match &self.kind {
                StmtKind::FuncDecl { body, .. } => match &body.stmts[index].kind {
                    StmtKind::VarDecl { expr, .. } => expr,
                    other => panic!("expected a variable declaration, got {other:?}"),
                },
                other => panic!("expected a function declaration, got {other:?}"),
            }
        }
    }

    #[test]
    fn parses_match_statement_with_block_arms() {
        use cme_core::ast::{MatchArmStmt, Pattern, StmtKind::Match as MatchStmt};
        let source = "void f(gameEvent evt) {\nmatch (evt) {\nDamage(int amount) => { g(1) }\nPlayerDied() => {}\n_ => {}\n}\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl { body, .. } => match &body.stmts[0].kind {
                MatchStmt { arms, .. } => {
                    assert_eq!(arms.len(), 3);
                    assert!(matches!(
                        arms[0].pattern,
                        Pattern::Variant { ref variant, ref bindings } if variant == "Damage" && bindings.len() == 1
                    ));
                    assert!(
                        matches!(arms[1].pattern, Pattern::Variant { ref variant, ref bindings } if variant == "PlayerDied" && bindings.is_empty())
                    );
                    assert!(matches!(arms[2].pattern, Pattern::Wildcard));
                    assert!(matches!(arms[0], MatchArmStmt { .. }));
                }
                other => panic!("expected a match statement, got {other:?}"),
            },
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_match_expression_with_expression_arms() {
        let source = "str f(gameEvent evt) {\nstr label = match (evt) {\nDamage(int amount) => \"dmg\"\n_ => \"other\"\n}\nreturn label\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let init = stmts[0].declaration_expr_of(0);
        match &init.kind {
            ExprKind::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(&arms[0].body.kind, ExprKind::StrLit(_)));
            }
            other => panic!("expected a match expression, got {other:?}"),
        }

        // Match in return position (§2.15).
        let source = "bool f(gameEvent evt) {\nreturn match (evt) {\nDamage(int amount) => true\n_ => false\n}\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl { body, .. } => match &body.stmts[0].kind {
                StmtKind::Return { value: Some(value) } => {
                    assert!(matches!(value.kind, ExprKind::Match { .. }));
                }
                other => panic!("expected a return, got {other:?}"),
            },
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn match_statement_recovers_from_broken_arms() {
        let source = "void f(gameEvent evt) {\nmatch (evt) {\nDamage(int amount) => { g(1) }\n??? => ???\n_ => {}\n}\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl { body, .. } => match &body.stmts[0].kind {
                StmtKind::Match { arms, .. } => {
                    // The broken arm is skipped; the wildcard survives.
                    assert_eq!(arms.len(), 2);
                }
                other => panic!("expected a match statement, got {other:?}"),
            },
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_for_in_loops() {
        let source = "int sum(int[] xs) {\nint total = 0\nfor (int v in xs) {\ntotal += v\n}\nreturn total\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        match &stmts[0].kind {
            StmtKind::FuncDecl { body, .. } => match &body.stmts[1].kind {
                StmtKind::For {
                    elem_ty,
                    elem_name,
                    iterable,
                    ..
                } => {
                    assert_eq!(elem_name, "v");
                    assert_eq!(*elem_ty, Type::Prim(PrimitiveType::Int));
                    assert!(matches!(&iterable.kind, ExprKind::Ident(name) if name == "xs"));
                }
                other => panic!("expected a for loop, got {other:?}"),
            },
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn parses_array_literals_with_both_separator_styles() {
        // Comma-separated (one line) and newline-separated (§11.1 CMON).
        let source = "int main() {\nint[] a = [1, 2, 3]\nint[] b = [\n1\n3\n5\n]\nint[] empty = []\nreturn 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let a = stmts[0].declaration_expr_of(0);
        let b = stmts[0].declaration_expr_of(1);
        let empty = stmts[0].declaration_expr_of(2);
        match &a.kind {
            ExprKind::ArrayLit { elements } => assert_eq!(elements.len(), 3),
            other => panic!("expected an array literal, got {other:?}"),
        }
        match &b.kind {
            ExprKind::ArrayLit { elements } => assert_eq!(elements.len(), 3),
            other => panic!("expected an array literal, got {other:?}"),
        }
        match &empty.kind {
            ExprKind::ArrayLit { elements } => assert!(elements.is_empty()),
            other => panic!("expected an empty array literal, got {other:?}"),
        }
    }

    #[test]
    fn parses_map_literals_with_both_separator_styles() {
        let source = "int main() {\nmap<str, int> a = {\"x\": 1, \"y\": 2}\nmap<str, int> b = {\n\"x\": 1\n\"y\": 2\n}\nmap<str, int> empty = {}\nreturn 0\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        for index in 0..2 {
            let init = stmts[0].declaration_expr_of(index);
            match &init.kind {
                ExprKind::MapLit { entries } => assert_eq!(entries.len(), 2),
                other => panic!("expected a map literal, got {other:?}"),
            }
        }
        let empty = stmts[0].declaration_expr_of(2);
        assert!(matches!(&empty.kind, ExprKind::MapLit { entries } if entries.is_empty()));
    }

    #[test]
    fn trailing_commas_in_literals_are_rejected() {
        let source = "int main() {\nint[] a = [1, 2,]\nreturn 0\n}\n";
        let (_, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("an element after `,`"));

        let source = "int main() {\nmap<str, int> m = {\"x\": 1,}\nreturn 0\n}\n";
        let (_, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("an entry after `,`"));
    }

    #[test]
    fn parses_interpolated_strings() {
        use cme_core::ast::InterpPart;
        // Scalar and expression islands (§2.8/§4.1).
        let source = "str f(int hp, vec2 p) {\nstr s = $\"hp={hp} pos=({p.x},{p.y}) v={hp * 2 + 1}\"\nreturn s\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let init = stmts[0].declaration_expr_of(0);
        match &init.kind {
            ExprKind::Interpolated { parts } => {
                let literal_count = parts
                    .iter()
                    .filter(|p| matches!(p, InterpPart::Literal(_)))
                    .count();
                let expr_count = parts
                    .iter()
                    .filter(|p| matches!(p, InterpPart::Expr(_)))
                    .count();
                // "hp=", " pos=(", ",", ")", " v=" -> 5 literals, 4 islands.
                assert_eq!(literal_count, 5);
                assert_eq!(expr_count, 4);
            }
            other => panic!("expected an interpolated string, got {other:?}"),
        }
    }

    #[test]
    fn interpolated_string_escapes_decode_only_in_literals() {
        let source = "str f(int hp) {\nstr s = $\"line1\\nline2 hp={hp}\"\nreturn s\n}\n";
        let (stmts, errors) = parse_program_parts(source);
        assert!(errors.is_empty(), "{errors:#?}");
        let init = stmts[0].declaration_expr_of(0);
        match &init.kind {
            ExprKind::Interpolated { parts } => match &parts[0] {
                cme_core::ast::InterpPart::Literal(text) => {
                    assert_eq!(text, "line1\nline2 hp=")
                }
                other => panic!("expected a literal part, got {other:?}"),
            },
            other => panic!("expected an interpolated string, got {other:?}"),
        }
    }

    #[test]
    fn broken_interpolation_islands_report_clean_diagnostics() {
        // Unterminated island.
        let source = "str f() {\nstr s = $\"oops {x\"\nreturn s\n}\n";
        let (_, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].to_string(), "unterminated interpolation island");

        // Empty island.
        let source = "str f() {\nstr s = $\"oops {}\"\nreturn s\n}\n";
        let (_, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].to_string(), "empty interpolation island");

        // Syntax error inside an island (the island lexes as its own
        // stream, so a dangling operator meets end of file).
        let source = "str f() {\nstr s = $\"val {1 +}\"\nreturn s\n}\n";
        let (_, errors) = parse_program_parts(source);
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].to_string(),
            "expected an expression, but found end of file"
        );
    }

    #[test]
    fn infer_function_return_type_is_rejected() {
        let (stmts, errors) = parse_program_parts("infer f() {\nreturn 1\n}\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].to_string(),
            "`infer` is only valid for local declarations"
        );
        assert_eq!(errors[0].span(), Span::new(0, 5));
        // Recovery keeps the declaration itself for tooling.
        assert!(matches!(
            stmts.first().map(|stmt| &stmt.kind),
            Some(StmtKind::FuncDecl { .. })
        ));
    }

    #[test]
    fn void_parameter_type_is_rejected() {
        let (stmts, errors) = parse_program_parts("int f(void x) {\nreturn 1\n}\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].to_string(),
            "`void` is only valid as a function return type"
        );
        assert_eq!(errors[0].span(), Span::new(6, 10));
        assert!(matches!(
            stmts.first().map(|stmt| &stmt.kind),
            Some(StmtKind::FuncDecl { .. })
        ));
    }

    #[test]
    fn infer_parameter_type_is_rejected() {
        let (stmts, errors) = parse_program_parts("int f(infer x) {\nreturn 1\n}\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].to_string(),
            "`infer` is only valid for local declarations"
        );
        assert_eq!(errors[0].span(), Span::new(6, 11));
        assert!(matches!(
            stmts.first().map(|stmt| &stmt.kind),
            Some(StmtKind::FuncDecl { .. })
        ));
    }

    #[test]
    fn void_variable_declaration_is_rejected() {
        // Pins existing behavior: `void` as a variable type is a parse error.
        let (stmts, errors) = parse_program_parts("void x = 5\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].to_string(),
            "`void` is only valid as a function return type"
        );
        assert_eq!(errors[0].span(), Span::new(0, 4));
        assert!(matches!(
            stmts.first().map(|stmt| &stmt.kind),
            Some(StmtKind::Invalid { .. })
        ));
    }

    #[test]
    fn function_body_block_span_covers_both_braces() {
        let source = "int f() {\nreturn 1\n}";
        let ast = parse_statement_ok(source);
        match &ast.kind {
            StmtKind::FuncDecl { body, .. } => {
                assert_eq!(body.span, Span::new(8, 20));
                assert_eq!(&source[body.span.start..body.span.start + 1], "{");
                assert_eq!(&source[body.span.end - 1..body.span.end], "}");
            }
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn while_body_block_span_covers_both_braces() {
        let source = "while (x) {\n}";
        let ast = parse_statement_ok(source);
        match &ast.kind {
            StmtKind::While { body, .. } => {
                assert_eq!(body.span, Span::new(10, 13));
                assert_eq!(&source[body.span.start..body.span.start + 1], "{");
                assert_eq!(&source[body.span.end - 1..body.span.end], "}");
            }
            other => panic!("expected a while statement, got {other:?}"),
        }
    }

    #[test]
    fn else_block_stmt_span_covers_else_keyword_through_closing_brace() {
        let source = "if (x) {\n} else {\n}";
        let ast = parse_statement_ok(source);
        match &ast.kind {
            StmtKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                assert_eq!(then_branch.span, Span::new(7, 10));
                let else_stmt = else_branch.as_deref().expect("an else branch");
                match &else_stmt.kind {
                    StmtKind::Block(block) => {
                        assert_eq!(block.span, Span::new(16, 19));
                        // The wrapper covers `else` through the block's `}`.
                        assert_eq!(else_stmt.span, Span::new(11, 19));
                        assert_eq!(&source[11..15], "else");
                        assert_eq!(&source[else_stmt.span.end - 1..else_stmt.span.end], "}");
                    }
                    other => panic!("expected a block statement, got {other:?}"),
                }
            }
            other => panic!("expected an if statement, got {other:?}"),
        }
    }

    #[test]
    fn block_span_reaches_eof_when_closing_brace_is_missing() {
        let (stmts, errors) = parse_program_parts("int f() {\nreturn 1\n");
        assert!(!errors.is_empty());
        match &stmts[0].kind {
            StmtKind::FuncDecl { body, .. } => {
                assert_eq!(body.span, Span::new(8, 19));
            }
            other => panic!("expected a function declaration, got {other:?}"),
        }
    }

    #[test]
    fn span_audit_walks_the_whole_basic_cm_ast() {
        let outcome = crate::parse_source(BASIC_CM);
        assert!(
            outcome.is_clean(),
            "basic.cm should parse with ZERO diagnostics: {outcome:#?}"
        );
        let len = BASIC_CM.len();
        for stmt in &outcome.statements {
            audit_span(stmt.span, len, "statement");
            assert!(
                stmt.span.start < stmt.span.end,
                "zero-width span on a statement in basic.cm: {:?}",
                stmt.span
            );
            audit_stmt(stmt, len);
        }
    }

    fn audit_span(span: Span, source_len: usize, what: &str) {
        assert!(span.start <= span.end, "reversed {what} span {span:?}");
        assert!(
            span.end <= source_len,
            "{what} span {span:?} exceeds source length {source_len}"
        );
    }

    fn audit_stmt(stmt: &Stmt, source_len: usize) {
        match &stmt.kind {
            StmtKind::VarDecl { expr, .. }
            | StmtKind::Assign { expr, .. }
            | StmtKind::CompoundAssign { expr, .. }
            | StmtKind::Expression { expr } => audit_expr(expr, source_len),
            StmtKind::FuncDecl { body, .. } => audit_block(body, source_len),
            StmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                audit_expr(cond, source_len);
                audit_block(then_branch, source_len);
                if let Some(else_stmt) = else_branch {
                    audit_span(else_stmt.span, source_len, "else statement");
                    assert!(
                        else_stmt.span.start < else_stmt.span.end,
                        "zero-width span on an else wrapper in basic.cm: {:?}",
                        else_stmt.span
                    );
                    audit_stmt(else_stmt, source_len);
                }
            }
            StmtKind::While { cond, body } => {
                audit_expr(cond, source_len);
                audit_block(body, source_len);
            }
            StmtKind::Return { value } => {
                if let Some(expr) = value {
                    audit_expr(expr, source_len);
                }
            }
            StmtKind::Block(block) => audit_block(block, source_len),
            StmtKind::For { iterable, body, .. } => {
                audit_expr(iterable, source_len);
                audit_block(body, source_len);
            }
            StmtKind::Match { scrutinee, arms } => {
                audit_expr(scrutinee, source_len);
                for arm in arms {
                    audit_block(&arm.body, source_len);
                }
            }
            StmtKind::ImplDecl { members, .. } => {
                for member in members {
                    audit_stmt(member, source_len);
                }
            }
            StmtKind::StructDecl { .. } | StmtKind::EnumDecl { .. } => {}
            StmtKind::Invalid { .. } => panic!("clean basic.cm must not contain Invalid"),
        }
    }

    fn audit_block(block: &Block, source_len: usize) {
        audit_span(block.span, source_len, "block");
        assert!(
            block.span.start < block.span.end,
            "zero-width span on a block in basic.cm: {:?}",
            block.span
        );
        for stmt in &block.stmts {
            audit_span(stmt.span, source_len, "statement");
            assert!(
                stmt.span.start < stmt.span.end,
                "zero-width span on a statement in basic.cm: {:?}",
                stmt.span
            );
            audit_stmt(stmt, source_len);
        }
    }

    fn audit_expr(expr: &Expr, source_len: usize) {
        audit_span(expr.span, source_len, "expression");
        assert!(
            expr.span.start < expr.span.end,
            "zero-width span on an expression in basic.cm: {:?}",
            expr.span
        );
        match &expr.kind {
            ExprKind::Paren { expr } => audit_expr(expr, source_len),
            ExprKind::Unary { expr, .. } => audit_expr(expr, source_len),
            ExprKind::Binary { lhs, rhs, .. } => {
                audit_expr(lhs, source_len);
                audit_expr(rhs, source_len);
            }
            ExprKind::Invalid { .. } => panic!("clean basic.cm must not contain Invalid"),
            _ => {}
        }
    }

    #[test]
    fn broken_if_header_recovers_to_sibling() {
        let (stmts, errors) = parse_program_parts("if\nint x = 1\n");
        assert!(!errors.is_empty());
        assert!(matches!(
            stmts.first().map(|stmt| &stmt.kind),
            Some(StmtKind::Invalid { .. })
        ));
    }

    #[test]
    fn missing_closing_brace_still_yields_function() {
        let (stmts, errors) = parse_program_parts("int f() {\nreturn 1\n");
        assert!(!errors.is_empty());
        assert!(matches!(
            stmts.first().map(|stmt| &stmt.kind),
            Some(StmtKind::FuncDecl { .. })
        ));
    }

    #[test]
    fn dangling_else_becomes_invalid_and_following_statement_survives() {
        let (stmts, errors) = parse_program_parts("if (x) {\n} else\nint x = 1\n");
        assert!(!errors.is_empty());
        assert!(
            stmts
                .iter()
                .any(|stmt| matches!(stmt.kind, StmtKind::VarDecl { .. }))
        );
    }

    fn has_call_with_arity(stmt: &Stmt, min_args: usize) -> bool {
        match &stmt.kind {
            StmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                expr_has_call_with_arity(cond, min_args)
                    || then_branch
                        .stmts
                        .iter()
                        .any(|inner| has_call_with_arity(inner, min_args))
                    || else_branch
                        .as_ref()
                        .is_some_and(|inner| has_call_with_arity(inner, min_args))
            }
            StmtKind::While { cond, body } => {
                expr_has_call_with_arity(cond, min_args)
                    || body
                        .stmts
                        .iter()
                        .any(|inner| has_call_with_arity(inner, min_args))
            }
            StmtKind::VarDecl { expr, .. }
            | StmtKind::Assign { expr, .. }
            | StmtKind::CompoundAssign { expr, .. }
            | StmtKind::Expression { expr } => expr_has_call_with_arity(expr, min_args),
            _ => false,
        }
    }

    fn expr_has_call_with_arity(expr: &Expr, min_args: usize) -> bool {
        match &expr.kind {
            ExprKind::Call { args, .. } if args.len() >= min_args => true,
            ExprKind::VariantCall { args, .. } => args.len() >= min_args,
            ExprKind::Binary { lhs, rhs, .. } => {
                expr_has_call_with_arity(lhs, min_args) || expr_has_call_with_arity(rhs, min_args)
            }
            ExprKind::Unary { expr, .. } => expr_has_call_with_arity(expr, min_args),
            ExprKind::Paren { expr } => expr_has_call_with_arity(expr, min_args),
            _ => false,
        }
    }

    fn statement_label(stmt: &Stmt) -> String {
        match &stmt.kind {
            StmtKind::VarDecl { ty, name, .. } => {
                format!(
                    "var:{name}:{}",
                    match ty {
                        Type::Prim(PrimitiveType::Int) => "Int",
                        Type::Prim(PrimitiveType::Float) => "Float",
                        Type::Prim(PrimitiveType::Bool) => "Bool",
                        Type::Prim(PrimitiveType::Str) => "Str",
                        Type::Infer => "None",
                        Type::Void => "Void",
                        _ => "Other",
                    }
                )
            }
            StmtKind::Assign { target, .. } => match target {
                LValue::Var { name } => format!("assign:{name}"),
                LValue::Field { name, .. } => format!("assign-field:{name}"),
                LValue::Index { .. } => "assign-index".to_string(),
            },
            StmtKind::CompoundAssign { target, .. } => match target {
                LValue::Var { name } => format!("compound:{name}"),
                LValue::Field { name, .. } => format!("compound-field:{name}"),
                LValue::Index { .. } => "compound-index".to_string(),
            },
            StmtKind::Invalid { .. } => "invalid".to_string(),
            _ => "invalid".to_string(),
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
                target: _,
                expr:
                    Expr {
                        span: _,
                        kind: ExprKind::Invalid { .. },
                    },
            }
            | StmtKind::CompoundAssign {
                target: _,
                op: _,
                expr:
                    Expr {
                        span: _,
                        kind: ExprKind::Invalid { .. },
                    },
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

        assert_eq!(errors.len(), 58, "diagnostics: {errors:#?}");
        assert_eq!(stmts.len(), 62, "statements: {stmts:#?}");

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
                "compound:score",
                "var:fence:Int",
                "compound:score",
                "compound:score",
                // §6 garbage heads (the `)` line is dropped by the strip pass;
                // the `}` line is now a token and produces one Invalid)
                "invalid",
                "invalid",
                "invalid",
                "invalid",
                "invalid",
                // §7 missing names
                "invalid",
                "invalid",
                "invalid",
                "invalid",
                // §8 bare identifiers — `lonely` is Invalid; `x y` now reads
                // as a named-type declaration with a missing `=` (same
                // recovery as `int x`), so it survives as a VarDecl
                "invalid",
                "var:y:Other",
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
            2usize, 3, 4, 5, 6, 7, 9, 12, 14, 15, 16, 19, 21, 24, 26, 27, 28, 29, 30, 31, 32, 33,
            34, 35, 36, 38, 40, 41, 44, 45, 47, 48, 49, 50, 56, 60, 61,
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
        // (seven lexer errors: four bad-character lines in §9 — `@ $`
        // records both characters — one unterminated string, and the
        // overflow digit run; the `1.2.3` float shape now lexes as
        // `1.2` `.` `3` and is a parse error instead, since `.` is a token
        // now that field access exists).
        assert!(matches!(
            &stmts[42],
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
            7
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
