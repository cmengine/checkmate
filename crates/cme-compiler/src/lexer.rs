use cme_core::Span;
use logos::Logos;

#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\f]+")]
#[logos(skip r"//[^\r\n]*")]
pub enum Token<'a> {
    #[regex(r"[\r\n]+")] // one or more line breaks -> one token
    Newline,

    // Keywords
    #[token("int")]
    KwInt,
    #[token("float")]
    KwFloat,
    #[token("infer")]
    KwInfer,
    #[token("return")]
    KwReturn,
    #[token("str")]
    KwStr,
    #[token("bool")]
    KwBool,
    #[token("true")]
    KwTrue,
    #[token("false")]
    KwFalse,

    // String Literals
    #[regex(r#""[^"\r\n]*""#)]
    StrLit(&'a str),

    // Symbols
    #[token("=")]
    Assign,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,

    // Identifiers (e.g., variable names, function names)
    // This regex matches a letter or underscore, followed by any number of letters, numbers, or underscores.
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]
    Ident(&'a str),

    // Integer Literals
    // This regex matches digits, and the closure parses it into an i64.
    #[regex(r"[0-9]+", |lex| lex.slice().parse::<i64>().unwrap())]
    IntLit(i64),

    // Float Literals
    #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().unwrap())]
    FloatLit(f64),
}

#[derive(Debug, PartialEq, Clone)]
pub struct SpannedToken<'a> {
    pub token: Token<'a>,
    pub span: Span,
}

#[derive(Debug, PartialEq, Clone)]
pub enum LexError {
    Invalid { span: Span },
}

pub fn lex(source: &str) -> Result<Vec<SpannedToken<'_>>, LexError> {
    let mut tokens = Vec::new();

    let mut lexer = Token::lexer(source);

    while let Some(result) = lexer.next() {
        let span = lexer.span();
        let token = result.map_err(|_| LexError::Invalid {
            span: Span::new(span.start, span.end),
        })?;
        tokens.push(SpannedToken {
            token,
            span: Span::new(span.start, span.end),
        });
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::Token;
    use crate::lexer::lex;

    fn lex_tokens(source: &str) -> Vec<Token<'_>> {
        lex(source)
            .unwrap_or_else(|error| panic!("source should lex: {error:?}"))
            .into_iter()
            .map(|spanned| spanned.token)
            .collect()
    }

    fn lex_ok(source: &str) -> Vec<Token<'_>> {
        lex_tokens(source)
    }

    #[test]
    fn lexes_keywords() {
        let source = "int float infer return str bool true false";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::KwInt,
                Token::KwFloat,
                Token::KwInfer,
                Token::KwReturn,
                Token::KwStr,
                Token::KwBool,
                Token::KwTrue,
                Token::KwFalse,
            ]
        );
    }

    #[test]
    fn keywords_take_precedence_over_boolean_identifiers() {
        let source = "true_x false_x trueish falseish";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::Ident("true_x"),
                Token::Ident("false_x"),
                Token::Ident("trueish"),
                Token::Ident("falseish"),
            ]
        );
    }

    #[test]
    fn lexes_symbols() {
        let source = "= ( ) { }";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::Assign,
                Token::LParen,
                Token::RParen,
                Token::LBrace,
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn lexes_identifiers() {
        let source = "x _value value_1 snake_case CamelCase";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::Ident("x"),
                Token::Ident("_value"),
                Token::Ident("value_1"),
                Token::Ident("snake_case"),
                Token::Ident("CamelCase"),
            ]
        );
    }

    #[test]
    fn keywords_take_precedence_over_identifiers() {
        let source = "intx infer_ return_x";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::Ident("intx"),
                Token::Ident("infer_"),
                Token::Ident("return_x"),
            ]
        );
    }

    #[test]
    fn lexes_integer_literals_without_digit_separators() {
        let source = "0 42";
        assert_eq!(lex_ok(source), vec![Token::IntLit(0), Token::IntLit(42)]);
    }

    #[test]
    fn lexes_float_literals() {
        let source = "0.0 42.5 123.0001";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::FloatLit(0.0),
                Token::FloatLit(42.5),
                Token::FloatLit(123.0001),
            ]
        );
    }

    #[test]
    fn lexes_adjacent_number_and_symbol() {
        let source = "12.5)";
        assert_eq!(lex_ok(source), vec![Token::FloatLit(12.5), Token::RParen]);
    }

    #[test]
    fn consecutive_newlines_become_one_token() {
        let source = "a\n\n\r\nb";
        assert_eq!(
            lex_ok(source),
            vec![Token::Ident("a"), Token::Newline, Token::Ident("b"),]
        );
    }

    #[test]
    fn each_commented_line_produces_a_newline() {
        let source = "int // ignored\n\t// another comment\nfloat";
        assert_eq!(
            lex_ok(source),
            vec![Token::KwInt, Token::Newline, Token::Newline, Token::KwFloat]
        );
    }

    #[test]
    fn comment_does_not_swallow_following_newline() {
        let source = "a // comment\nb";
        assert_eq!(
            lex_ok(source),
            vec![Token::Ident("a"), Token::Newline, Token::Ident("b")]
        );
    }

    #[test]
    fn empty_source_produces_no_tokens() {
        assert!(lex_ok("").is_empty());
        assert_eq!(
            lex_ok(" \t \n // only whitespace and comments\n"),
            vec![Token::Newline, Token::Newline]
        );
    }

    #[test]
    fn rejects_unrecognized_characters() {
        assert!(lex("$").is_err());
        assert!(lex("a + b").is_err());
        assert!(lex("1.2.3").is_err());
    }

    #[test]
    fn lexes_string_literals() {
        let source = r#""text" "" "spaces and symbols!""#;
        assert_eq!(
            lex_ok(source),
            vec![
                Token::StrLit("\"text\""),
                Token::StrLit("\"\""),
                Token::StrLit("\"spaces and symbols!\""),
            ]
        );
    }

    #[test]
    fn rejects_unterminated_string_literals() {
        assert!(lex("\"text").is_err());
    }
}
