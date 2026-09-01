use std::fmt;

use cme_core::Span;
use logos::Logos;

#[derive(Logos, Debug, PartialEq, Clone, Copy)]
#[logos(skip r"[ \t\f]+")]
#[logos(skip r"//[^\r\n]*")]
pub enum Token<'a> {
    #[regex(r"[\r\n]+")] // one or more line breaks -> one token
    Newline,

    // String Literals
    #[regex(r#""[^"\r\n]*""#)]
    StrLit(&'a str),

    // Symbols
    #[token("||")]
    Or,
    #[token("&&")]
    And,
    #[token("==")]
    Eq,
    #[token("!=")]
    Ne,
    #[token("<=")]
    Le,
    #[token(">=")]
    Ge,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("+=")]
    AddAssign,
    #[token("-=")]
    SubAssign,
    #[token("*=")]
    MulAssign,
    #[token("/=")]
    DivAssign,
    #[token("%=")]
    RemAssign,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("!")]
    Not,
    #[token("=")]
    Assign,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,

    // Identifiers (e.g., variable names, function names)
    // This regex matches a letter or underscore, followed by any number of letters, numbers, or underscores.
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]
    Ident(&'a str),

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

    // Integer Literals
    // This regex matches digits, and the closure parses it into an i64. A
    // digit run too large for i64 fails the callback, which turns the token
    // into a lexing error instead of panicking — recovery then skips the
    // literal like any other invalid region.
    #[regex(r"[0-9]+", |lex| lex.slice().parse::<i64>().map_err(|_| ()))]
    IntLit(i64),

    // Float Literals
    #[regex(
        r"[0-9]+\.[0-9]+",
        |lex| lex.slice().parse::<f64>().ok().filter(|v| v.is_finite()).ok_or(())
    )]
    FloatLit(f64),

    /// Synthetic end-of-input marker appended by the lexer. Never produced by a
    /// regex; the parser relies on it to make `advance` infallible.
    Eof,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct SpannedToken<'a> {
    pub token: Token<'a>,
    pub span: Span,
}

/// A lexer failure. Each variant points at the offending source region.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum LexError {
    /// A character that cannot begin any token (e.g. `@`, `$`, a stray `.`).
    InvalidCharacter { span: Span },
    /// A `"` with no closing `"` before the end of the line/file.
    UnterminatedString { span: Span },
    /// An integer literal whose digit run does not fit in `i64`.
    IntegerOverflow { span: Span },
    /// A float literal that would parse to infinity.
    FloatOverflow { span: Span },
}

impl LexError {
    pub fn span(&self) -> Span {
        match self {
            LexError::InvalidCharacter { span }
            | LexError::UnterminatedString { span }
            | LexError::IntegerOverflow { span }
            | LexError::FloatOverflow { span } => *span,
        }
    }
}

impl<'a> Token<'a> {
    /// A human-readable name for use in diagnostics.
    pub fn describe(&self) -> String {
        match self {
            Token::Ident(name) => format!("identifier `{name}`"),
            Token::StrLit(_) => "string literal".into(),
            Token::IntLit(value) => format!("integer literal `{value}`"),
            Token::FloatLit(value) => format!("float literal `{value}`"),
            Token::Newline => "end of statement".into(),
            Token::KwInt => "`int`".into(),
            Token::KwFloat => "`float`".into(),
            Token::KwStr => "`str`".into(),
            Token::KwBool => "`bool`".into(),
            Token::KwInfer => "`infer`".into(),
            Token::KwReturn => "`return`".into(),
            Token::KwTrue => "`true`".into(),
            Token::KwFalse => "`false`".into(),
            Token::Assign => "`=`".into(),
            Token::AddAssign => "`+=`".into(),
            Token::SubAssign => "`-=`".into(),
            Token::MulAssign => "`*=`".into(),
            Token::DivAssign => "`/=`".into(),
            Token::RemAssign => "`%=`".into(),
            Token::Plus => "`+`".into(),
            Token::Minus => "`-`".into(),
            Token::Star => "`*`".into(),
            Token::Slash => "`/`".into(),
            Token::Percent => "`%`".into(),
            Token::And => "`&&`".into(),
            Token::Or => "`||`".into(),
            Token::Not => "`!`".into(),
            Token::Eq => "`==`".into(),
            Token::Ne => "`!=`".into(),
            Token::Le => "`<=`".into(),
            Token::Ge => "`>=`".into(),
            Token::Lt => "`<`".into(),
            Token::Gt => "`>`".into(),
            Token::LParen => "`(`".into(),
            Token::RParen => "`)`".into(),
            Token::Eof => "end of file".into(),
        }
    }

    /// The keywords that can start a variable declaration.
    pub(crate) fn is_type_keyword(&self) -> bool {
        matches!(
            self,
            Token::KwInt | Token::KwFloat | Token::KwStr | Token::KwBool | Token::KwInfer
        )
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            LexError::InvalidCharacter { .. } => "invalid character",
            LexError::UnterminatedString { .. } => "unterminated string literal",
            LexError::IntegerOverflow { .. } => "integer literal is too large",
            LexError::FloatOverflow { .. } => "float literal is too large",
        };
        f.write_str(msg)
    }
}

/// Chooses the `LexError` variant for a failed region by inspecting the source
/// text: a leading `"` means an unterminated string; an all-digit run is integer
/// overflow; a `digits.digits` shape is float overflow; anything else is a bad
/// character.
fn classify_error(source: &str, span: Span) -> LexError {
    let text = &source[span.start..span.end];
    if text.starts_with('"') {
        LexError::UnterminatedString { span }
    } else if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        LexError::IntegerOverflow { span }
    } else if is_float_shape(text) {
        LexError::FloatOverflow { span }
    } else {
        LexError::InvalidCharacter { span }
    }
}

fn is_float_shape(text: &str) -> bool {
    match text.split_once('.') {
        Some((int_part, frac_part)) => {
            !int_part.is_empty()
                && !frac_part.is_empty()
                && int_part.bytes().all(|b| b.is_ascii_digit())
                && frac_part.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

pub fn lex(source: &str) -> Result<Vec<SpannedToken<'_>>, LexError> {
    let (tokens, errors) = lex_with_errors(source);
    match errors.into_iter().next() {
        Some(error) => Err(error),
        None => Ok(tokens),
    }
}

pub fn lex_with_errors(source: &str) -> (Vec<SpannedToken<'_>>, Vec<LexError>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    let mut lexer = Token::lexer(source);
    while let Some(result) = lexer.next() {
        let span = Span::new(lexer.span().start, lexer.span().end);
        match result {
            Ok(token) => tokens.push(SpannedToken { token, span }),
            Err(()) => {
                errors.push(classify_error(source, span));
                if let Some(newline_span) = skip_to_line_end(&mut lexer, source, &mut errors) {
                    tokens.push(SpannedToken {
                        token: Token::Newline,
                        span: newline_span,
                    });
                }
            }
        }
    }
    let eof_span = Span::new(source.len(), source.len());
    tokens.push(SpannedToken {
        token: Token::Eof,
        span: eof_span,
    });

    (tokens, errors)
}

/// Resynchronizes after a lexing error: consumes tokens up to and including the
/// next newline so the damaged line stays line-granular, and records every lexer
/// error encountered on the way — errors are never swallowed. Valid tokens inside
/// the damaged region are dropped (recovery keeps statement boundaries only).
/// Returns the newline's span if the region ended at a line break.
fn skip_to_line_end<'src>(
    lexer: &mut logos::Lexer<'src, Token<'src>>,
    source: &'src str,
    errors: &mut Vec<LexError>,
) -> Option<Span> {
    while let Some(result) = lexer.next() {
        let span = Span::new(lexer.span().start, lexer.span().end);
        match result {
            Ok(Token::Newline) => return Some(span),
            Ok(_) => {}
            Err(()) => errors.push(classify_error(source, span)),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::Token;
    use crate::lexer::LexError;
    use crate::lexer::lex;
    use crate::lexer::lex_with_errors;
    use cme_core::Span;

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
    fn recovers_from_invalid_token_at_next_newline() {
        let source = "infer a = @\nint b = 1\n";
        let (tokens, errors) = crate::lexer::lex_with_errors(source);

        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0],
            LexError::InvalidCharacter {
                span: Span::new(10, 11),
            }
        );
        assert_eq!(
            tokens
                .into_iter()
                .map(|token| token.token)
                .collect::<Vec<_>>(),
            vec![
                Token::KwInfer,
                Token::Ident("a"),
                Token::Assign,
                Token::Newline,
                Token::KwInt,
                Token::Ident("b"),
                Token::Assign,
                Token::IntLit(1),
                Token::Newline,
                Token::Eof,
            ]
        );
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
                Token::Eof,
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
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_symbols() {
        let source = "= ( )";
        assert_eq!(
            lex_ok(source),
            vec![Token::Assign, Token::LParen, Token::RParen, Token::Eof]
        );
    }

    #[test]
    fn describes_every_token_for_diagnostics() {
        let cases: Vec<(Token<'_>, &str)> = vec![
            (Token::Ident("x"), "identifier `x`"),
            (Token::StrLit("\"x\""), "string literal"),
            (Token::IntLit(42), "integer literal `42`"),
            (Token::FloatLit(4.2), "float literal `4.2`"),
            (Token::Newline, "end of statement"),
            (Token::KwInt, "`int`"),
            (Token::KwFloat, "`float`"),
            (Token::KwStr, "`str`"),
            (Token::KwBool, "`bool`"),
            (Token::KwInfer, "`infer`"),
            (Token::KwReturn, "`return`"),
            (Token::KwTrue, "`true`"),
            (Token::KwFalse, "`false`"),
            (Token::Assign, "`=`"),
            (Token::AddAssign, "`+=`"),
            (Token::SubAssign, "`-=`"),
            (Token::MulAssign, "`*=`"),
            (Token::DivAssign, "`/=`"),
            (Token::RemAssign, "`%=`"),
            (Token::Plus, "`+`"),
            (Token::Minus, "`-`"),
            (Token::Star, "`*`"),
            (Token::Slash, "`/`"),
            (Token::Percent, "`%`"),
            (Token::And, "`&&`"),
            (Token::Or, "`||`"),
            (Token::Not, "`!`"),
            (Token::Eq, "`==`"),
            (Token::Ne, "`!=`"),
            (Token::Le, "`<=`"),
            (Token::Ge, "`>=`"),
            (Token::Lt, "`<`"),
            (Token::Gt, "`>`"),
            (Token::LParen, "`(`"),
            (Token::RParen, "`)`"),
            (Token::Eof, "end of file"),
        ];

        for (token, description) in cases {
            assert_eq!(token.describe(), description);
        }
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
                Token::Eof,
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
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_integer_literals_without_digit_separators() {
        let source = "0 42";
        assert_eq!(
            lex_ok(source),
            vec![Token::IntLit(0), Token::IntLit(42), Token::Eof]
        );
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
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_adjacent_number_and_symbol() {
        let source = "12.5)";
        assert_eq!(
            lex_ok(source),
            vec![Token::FloatLit(12.5), Token::RParen, Token::Eof]
        );
    }

    #[test]
    fn consecutive_newlines_become_one_token() {
        let source = "a\n\n\r\nb";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::Ident("a"),
                Token::Newline,
                Token::Ident("b"),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn each_commented_line_produces_a_newline() {
        let source = "int // ignored\n\t// another comment\nfloat";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::KwInt,
                Token::Newline,
                Token::Newline,
                Token::KwFloat,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn comment_does_not_swallow_following_newline() {
        let source = "a // comment\nb";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::Ident("a"),
                Token::Newline,
                Token::Ident("b"),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn empty_source_produces_only_eof() {
        assert_eq!(lex_ok(""), vec![Token::Eof]);
        assert_eq!(
            lex_ok(" \t \n // only whitespace and comments\n"),
            vec![Token::Newline, Token::Newline, Token::Eof]
        );
    }

    #[test]
    fn rejects_unrecognized_characters() {
        assert!(lex("$").is_err());
        assert!(lex("1.2.3").is_err());
    }

    #[test]
    fn digit_run_overflowing_i64_is_an_error_not_a_panic() {
        // 23 digits cannot fit an i64; the callback must fail the token
        // (recovery skips it) instead of panicking on unwrap.
        let source = "int huge = 99999999999999999999999\nint ok = 1\n";
        let (tokens, errors) = crate::lexer::lex_with_errors(source);

        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0],
            LexError::IntegerOverflow {
                span: Span::new(11, 34), // the 23-digit run
            }
        );
        let kinds: Vec<_> = tokens.iter().map(|spanned| spanned.token).collect();
        assert!(matches!(
            kinds.as_slice(),
            [
                Token::KwInt,
                Token::Ident("huge"),
                Token::Assign,
                Token::Newline,
                Token::KwInt,
                Token::Ident("ok"),
                Token::Assign,
                Token::IntLit(1),
                Token::Newline,
                Token::Eof,
            ]
        ));

        // The i64 boundary itself must keep lexing.
        let source = "9223372036854775807";
        let (tokens, errors) = crate::lexer::lex_with_errors(source);
        assert!(errors.is_empty());
        assert_eq!(tokens[0].token, Token::IntLit(i64::MAX));
        assert_eq!(tokens[1].token, Token::Eof);
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
                Token::Eof,
            ]
        );
    }

    #[test]
    fn rejects_unterminated_string_literals() {
        assert!(lex("\"text").is_err());
        let (_, errors) = lex_with_errors("\"text");
        assert_eq!(
            errors,
            vec![LexError::UnterminatedString {
                span: Span::new(0, 5)
            }]
        );
    }

    #[test]
    fn float_digit_run_overflowing_f64_is_an_error_not_infinity() {
        let source = format!("float huge = {}.0\nint ok = 1\n", "9".repeat(400));
        let (tokens, errors) = lex_with_errors(&source);

        assert_eq!(
            errors,
            vec![LexError::FloatOverflow {
                span: Span::new(13, 415)
            }]
        );
        assert!(matches!(
            tokens.last().map(|spanned| &spanned.token),
            Some(Token::Eof)
        ));
    }

    #[test]
    fn multiple_bad_chars_on_one_line_are_all_reported() {
        let (tokens, errors) = lex_with_errors("@ $\nint b = 1\n");

        assert_eq!(errors.len(), 2);
        assert_eq!(
            errors,
            vec![
                LexError::InvalidCharacter {
                    span: Span::new(0, 1)
                },
                LexError::InvalidCharacter {
                    span: Span::new(2, 3)
                },
            ]
        );
        assert!(matches!(
            tokens.first().map(|spanned| &spanned.token),
            Some(Token::Newline)
        ));
        assert!(matches!(
            tokens.last().map(|spanned| &spanned.token),
            Some(Token::Eof)
        ));
    }
}
