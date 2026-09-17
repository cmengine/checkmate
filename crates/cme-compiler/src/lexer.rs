use std::fmt;

use cme_core::Span;
use logos::Logos;

/// The callback for [`Token::StrLit`]: validates that every backslash in
/// the matched slice is followed by one of the four accepted escape
/// characters, returning the raw slice when the literal is well formed and
/// `None` (failing the token) when it is not. The lexer's error recovery
/// then classifies and reports the failure.
fn str_lit<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Option<&'src str> {
    let slice = lex.slice();
    str_escapes_valid(slice).then_some(slice)
}

/// True when every backslash inside a matched string slice pairs with one
/// of the four accepted escape characters. The final byte is the
/// delimiting quote, so a backslash in the last content position is a
/// dangling escape and fails validation.
fn str_escapes_valid(slice: &str) -> bool {
    let content = &slice[1..slice.len() - 1];
    let bytes = content.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            let Some(&next) = bytes.get(index + 1) else {
                return false;
            };
            if !is_accepted_escape_byte(next) {
                return false;
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    true
}

fn is_accepted_escape_byte(byte: u8) -> bool {
    matches!(byte, b'n' | b't' | b'\\' | b'"')
}

/// The callback for [`Token::InterpStrLit`]: the regex matches only the
/// `$"` opener; this callback scans the rest of the literal by hand
/// because a `{...}` island may itself contain string literals —
/// `$"age: { ages["ana"] }"` — whose quotes must not close the outer
/// literal. The scan tracks island depth: a `"` at depth 0 closes the
/// literal, a `"` at depth > 0 opens a nested string that is skipped
/// whole (honoring the four accepted escape pairs). On success the
/// lexer is bumped past the full literal; on failure recovery
/// classifies the region via [`classify_interp_string_error`].
fn interp_str_lit<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Option<&'src str> {
    let rest = lex.remainder();
    match scan_interp_rest(rest) {
        InterpScan::Closed(rest_len) => {
            lex.bump(rest_len);
            Some(lex.slice())
        }
        // Consume the damaged extent before failing so recovery resyncs
        // past the whole literal (one diagnostic, like the old regex's
        // whole-literal failure span). A rejected escape pair swallows the
        // rest of the line; an unterminated literal stops at its failure
        // point (the recovery for that shape is already line-granular).
        InterpScan::Unterminated(consumed) => {
            lex.bump(consumed);
            None
        }
        InterpScan::BadEscape(consumed) => {
            let line_end = rest[consumed..]
                .find(['\n', '\r'])
                .map_or(rest.len(), |offset| consumed + offset);
            lex.bump(line_end);
            None
        }
    }
}

/// The outcome of scanning the text after a `$"` opener.
enum InterpScan {
    /// The literal is well formed: byte length consumed, including the
    /// closing quote.
    Closed(usize),
    /// No closing quote before end of line/input: how far the literal
    /// extends (the failure point).
    Unterminated(usize),
    /// An invalid escape pair outside an island string: how far the
    /// literal extends, ending at the backslash.
    BadEscape(usize),
}

/// Scans the text after a `$"` opener for the end of the interpolated
/// string literal:
///
/// - a `"` outside any `{...}` island closes the literal;
/// - a `"` inside an island opens a nested string that is skipped whole
///   (escape pairs honored), so `{ m["key"] }` scans correctly;
/// - an island string that never closes before the line end backtracks:
///   its opening quote then terminates the literal, mirroring the old
///   flat-regex reading and leaving the broken island to the parser's
///   island diagnostics;
/// - `\` pairs with one of the four accepted escapes; outside an island's
///   nested strings a rejected pair fails the scan, inside one it is
///   consumed (the parser's island sub-lex reports it later);
/// - a line break or end of input outside any island string is a failure
///   (literals are single-line).
fn scan_interp_rest(rest: &str) -> InterpScan {
    let bytes = rest.as_bytes();
    let mut index = 0usize;
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' if depth == 0 => return InterpScan::Closed(index + 1),
            b'"' => {
                // A nested string inside an island: skip it whole. If it
                // never closes on this line, backtrack — this quote ends
                // the literal instead (the old flat-regex reading).
                let quote = index;
                index += 1;
                loop {
                    match bytes.get(index) {
                        Some(b'"') => {
                            index += 1;
                            break;
                        }
                        Some(b'\\') => {
                            match bytes.get(index + 1) {
                                Some(&escape) if is_accepted_escape_byte(escape) => index += 2,
                                // An invalid escape pair inside the island's
                                // string is still inside the outer literal's
                                // extent: keep scanning so the closing quote
                                // (if any) is found and the parser's island
                                // sub-lex reports the real error later.
                                Some(_) => index += 2,
                                None => return InterpScan::Closed(quote + 1),
                            }
                        }
                        Some(b'\n') | Some(b'\r') | None => return InterpScan::Closed(quote + 1),
                        Some(_) => index += 1,
                    }
                }
            }
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'\\' => {
                match bytes.get(index + 1) {
                    Some(&escape) if is_accepted_escape_byte(escape) => index += 2,
                    // A rejected pair — including a dangling backslash at
                    // end of line or input — fails the token like the old
                    // regex + validation did; recovery classifies it as
                    // `InvalidEscape`.
                    Some(_) | None => return InterpScan::BadEscape(index),
                }
            }
            b'\n' | b'\r' => return InterpScan::Unterminated(index),
            _ => index += 1,
        }
    }
    InterpScan::Unterminated(bytes.len())
}

/// Rescans an interpolated string literal from `start` (the `$`) to
/// classify why it failed, mirroring [`scan_interp_rest`] with absolute
/// spans.
fn classify_interp_string_error(source: &str, start: usize) -> LexError {
    let content_start = start + 2;
    match scan_interp_rest(&source[content_start.min(source.len())..]) {
        InterpScan::Closed(_) => LexError::InvalidCharacter {
            span: Span::new(start, start + 2),
        },
        InterpScan::Unterminated(consumed) => LexError::UnterminatedInterpolatedString {
            span: Span::new(start, content_start + consumed),
        },
        InterpScan::BadEscape(at) => {
            let backslash = content_start + at;
            LexError::InvalidEscape {
                span: Span::new(backslash, (backslash + 2).min(source.len())),
            }
        }
    }
}

/// The callback for every [`Token::IntLit`] regex: parses the matched
/// slice into its `i64` value, honoring the radix prefix (`0x`/`0X` hex,
/// `0o`/`0O` octal, `0b`/`0B` binary) and stripping `_` digit separators.
/// A digit run too large for `i64` fails the callback, which turns the
/// token into a lexing error — recovery classifies it as
/// [`LexError::IntegerOverflow`] via [`classify_error`].
fn int_lit<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Result<i64, ()> {
    parse_int_slice(lex.slice()).ok_or(())
}

/// Parses a full integer-literal slice (radix prefix and `_` separators
/// allowed) into its value. Returns `None` when the digits overflow `i64`.
pub(crate) fn parse_int_slice(slice: &str) -> Option<i64> {
    let (radix, digits) = if let Some(rest) = strip_radix_prefix(slice, "0x") {
        (16, rest)
    } else if let Some(rest) = strip_radix_prefix(slice, "0o") {
        (8, rest)
    } else if let Some(rest) = strip_radix_prefix(slice, "0b") {
        (2, rest)
    } else {
        (10, slice)
    };
    // The regexes only deliver separators *between* digits, so stripping
    // them cannot empty the digit field; a decimal slice of `_`-free digits
    // either parses or overflows.
    let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
    i64::from_str_radix(&cleaned, radix).ok()
}

/// Case-insensitive prefix strip: `slice` beginning with `prefix` in
/// either all-lowercase or all-uppercase form yields the remainder.
fn strip_radix_prefix<'a>(slice: &'a str, prefix: &str) -> Option<&'a str> {
    if slice.len() > prefix.len() && slice[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&slice[prefix.len()..])
    } else {
        None
    }
}

/// The callback for every [`Token::FloatLit`] regex: strips `_` digit
/// separators and parses the remainder as `f64`. A literal whose value
/// would be infinite fails the callback — recovery classifies the region
/// as [`LexError::FloatOverflow`].
fn float_lit<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Result<f64, ()> {
    let cleaned: String = lex.slice().chars().filter(|c| *c != '_').collect();
    cleaned
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or(())
}

/// The callback for [`Token::BlockComment`]: after the regex matches the
/// `/*` opener, consumes through the first `*/`. `None` (failing the token)
/// for an unterminated comment — everything from the opener on is comment.
fn block_comment<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Option<()> {
    let rest = lex.remainder();
    let end = rest.find("*/")?;
    lex.bump(end + 2);
    Some(())
}

/// The callback for [`Token::Ident`]: passes the matched slice through
/// unless it matches the reserved `mega<N>` shape, in which case the token
/// fails and recovery classifies the region as
/// [`LexError::ReservedIdent`].
fn ident<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Option<&'src str> {
    let slice = lex.slice();
    if is_reserved_mega_ident(slice) {
        None
    } else {
        Some(slice)
    }
}

/// True when `slice` is exactly `mega` followed by one or more digits —
/// the identifier shape reserved for future use by the megaprogramming
/// machinery (§8): `mega0`, `mega1`, `mega42`. `mega` alone, `mega0x`,
/// and `mega_0` are ordinary identifiers.
pub(crate) fn is_reserved_mega_ident(slice: &str) -> bool {
    let Some(rest) = slice.strip_prefix("mega") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

#[derive(Logos, Debug, PartialEq, Clone, Copy)]
#[logos(skip r"[ \t\f]+")]
#[logos(skip r"//[^\r\n]*")]
pub enum Token<'a> {
    #[regex(r"[\r\n]+")] // one or more line breaks -> one token
    Newline,

    // String Literals. At the match level a backslash pairs with any
    // non-newline character (so an escaped quote does not close the
    // literal, and a bare trailing backslash still consumes to the line
    // end); the callback then restricts escapes to the accepted set
    // (`n`, `t`, `\\`, `"`), failing the token on any other pair.
    // Recovery classifies the failed region as `InvalidEscape` or
    // `UnterminatedString` by rescanning the source.
    #[regex(r#""(?:\\[^\r\n]|\\|[^"\r\n\\])*""#, str_lit)]
    StrLit(&'a str),

    // Interpolated string literals (§2.8/§4.1): a `$` sigil before the
    // opening quote. The regex matches only the opener; the callback scans
    // the rest by hand so a `{...}` island may contain string literals
    // (`$"ages: { table["ana"] }"`) whose quotes do not close the literal.
    // Escape validation is identical to plain strings; `{expr}` islands are
    // recognized by the parser, not the lexer.
    #[regex(r#"\$\""#, interp_str_lit)]
    InterpStrLit(&'a str),

    // Block comments (§2.2). The regex matches the opener and the callback
    // consumes through the first `*/` with hand-written scanning: logos has
    // no backtracking, so the pure-regex formulations either over-consume
    // (`/***/` swallows following code) or dead-end. The token is filtered
    // out of the stream like whitespace; an unterminated comment fails the
    // callback, and recovery classifies it as `UnterminatedBlockComment`.
    #[regex(r"/\*", block_comment)]
    BlockComment,

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
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token(":")]
    Colon,
    #[token("=>")]
    FatArrow,
    #[token("?")]
    Question,

    // Numeric literals (§2.4). These variants are declared BEFORE `Ident`
    // on purpose: logos breaks same-length matches in declaration order,
    // and every numeric form collides with the digit-led identifier rule
    // at equal length (`0x1F`, `1_000`, `1e10`). A digit-led word is a
    // number only when the WHOLE maximal word matches a numeric grammar;
    // anything else (`0xFFg`, `0b12`, `1e`, `1__0`) falls back to the
    // longest-match identifier — no backtracking, no new diagnostics.
    //
    // Float literals: a fractional part (with optional `_` separators on
    // both sides) and/or an exponent (`e`/`E`, optional sign, with
    // separators). `1.5e3` and `1e10` are floats; `1.5e` degrades to the
    // float `1.5` followed by the identifier `e`. An infinite value fails
    // the callback — recovery reports `FloatOverflow`.
    // `priority` beats the digit-led identifier rule on LENGTH TIES
    // (logos prefers the longest match; equal lengths resolve by priority,
    // and every pattern here would otherwise tie with the identifier at
    // the same specificity). Longer identifier matches still win by
    // length, which is exactly the `0xFFg`-stays-a-name fallback.
    // Distinct priorities per pattern: patterns sharing a priority can
    // merge their automaton states in a way that drops the shared integer
    // leaf (observed as `1.length` failing to fall back to `IntLit(1)`).
    // Length still dominates, so these numbers only break true ties.
    #[regex(
        r"[0-9](_[0-9]|[0-9])*\.[0-9](_[0-9]|[0-9])*([eE][+-]?[0-9](_[0-9]|[0-9])*)?",
        float_lit,
        priority = 12
    )]
    #[regex(
        r"[0-9](_[0-9]|[0-9])*[eE][+-]?[0-9](_[0-9]|[0-9])*",
        float_lit,
        priority = 11
    )]
    FloatLit(f64),

    // Integer literals: radix prefixes with separators, then decimal. A
    // digit run too large for i64 fails the callback, which turns the
    // token into a lexing error instead of panicking — recovery then
    // classifies the literal like any other invalid region.
    #[regex(r"0[xX][0-9a-fA-F](_?[0-9a-fA-F])*", int_lit, priority = 10)]
    #[regex(r"0[oO][0-7](_?[0-7])*", int_lit, priority = 9)]
    #[regex(r"0[bB][01](_?[01])*", int_lit, priority = 8)]
    #[regex(r"[0-9](_[0-9]|[0-9])*", int_lit, priority = 7)]
    IntLit(i64),

    // Identifiers (e.g., variable names, function names)
    // Two shapes match: a letter or underscore followed by word characters,
    // and (§2.4 identifiers) a digit-led run containing at least one
    // non-digit character (`3Vector`, `2D`, `2_D`) — a pure digit run is an
    // integer literal, never an identifier. The numeric literal rules above
    // claim their shapes first; this fallback keeps every other digit-led
    // word a name, so `0xFFg` and `123abc` stay identifiers by longest
    // match. The callback fails the reserved `mega<N>` shape (see
    // [`is_reserved_mega_ident`]); recovery reports the dedicated diagnostic.
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", ident)]
    #[regex(r"[0-9]+[a-zA-Z_][a-zA-Z0-9_]*", ident)]
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
    #[token("if")]
    KwIf,
    #[token("else")]
    KwElse,
    #[token("while")]
    KwWhile,
    #[token("void")]
    KwVoid,
    #[token("str")]
    KwStr,
    #[token("bool")]
    KwBool,
    #[token("byte")]
    KwByte,
    #[token("true")]
    KwTrue,
    #[token("false")]
    KwFalse,
    #[token("struct")]
    KwStruct,
    #[token("enum")]
    KwEnum,
    #[token("match")]
    KwMatch,
    #[token("for")]
    KwFor,
    #[token("in")]
    KwIn,
    #[token("impl")]
    KwImpl,
    #[token("import")]
    KwImport,

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
    /// A `$"` with no closing `"` before the end of the line/file.
    UnterminatedInterpolatedString { span: Span },
    /// A `/*` with no closing `*/` before the end of the file. The whole
    /// remaining source is comment.
    UnterminatedBlockComment { span: Span },
    /// A backslash inside a string literal that is not followed by one of
    /// the four accepted escape characters (`n`, `t`, `\\`, `"`).
    InvalidEscape { span: Span },
    /// An integer literal whose digit run does not fit in `i64` (decimal,
    /// or any radix prefix: hex `0x`, octal `0o`, binary `0b`).
    IntegerOverflow { span: Span },
    /// A float literal that would parse to infinity (fractional and/or
    /// exponent form, `_` separators allowed).
    FloatOverflow { span: Span },
    /// An identifier spelled `mega` followed only by digits (`mega0`,
    /// `mega1`, `mega42`, …): reserved for future use by the megaprogramming
    /// machinery (§8), never a legal name.
    ReservedIdent { span: Span, name: String },
}

impl LexError {
    pub fn span(&self) -> Span {
        match self {
            LexError::InvalidCharacter { span }
            | LexError::UnterminatedString { span }
            | LexError::UnterminatedInterpolatedString { span }
            | LexError::UnterminatedBlockComment { span }
            | LexError::InvalidEscape { span }
            | LexError::IntegerOverflow { span }
            | LexError::FloatOverflow { span }
            | LexError::ReservedIdent { span, .. } => *span,
        }
    }
}

impl<'a> Token<'a> {
    /// A human-readable name for use in diagnostics.
    pub fn describe(&self) -> String {
        match self {
            Token::Ident(name) => format!("identifier `{name}`"),
            Token::StrLit(_) => "string literal".into(),
            Token::InterpStrLit(_) => "interpolated string literal".into(),
            Token::BlockComment => "block comment".into(),
            Token::IntLit(value) => format!("integer literal `{value}`"),
            Token::FloatLit(value) => format!("float literal `{value}`"),
            Token::Newline => "end of statement".into(),
            Token::KwInt => "`int`".into(),
            Token::KwFloat => "`float`".into(),
            Token::KwStr => "`str`".into(),
            Token::KwBool => "`bool`".into(),
            Token::KwByte => "`byte`".into(),
            Token::KwInfer => "`infer`".into(),
            Token::KwReturn => "`return`".into(),
            Token::KwIf => "`if`".into(),
            Token::KwElse => "`else`".into(),
            Token::KwWhile => "`while`".into(),
            Token::KwVoid => "`void`".into(),
            Token::KwTrue => "`true`".into(),
            Token::KwFalse => "`false`".into(),
            Token::KwStruct => "`struct`".into(),
            Token::KwEnum => "`enum`".into(),
            Token::KwMatch => "`match`".into(),
            Token::KwFor => "`for`".into(),
            Token::KwIn => "`in`".into(),
            Token::KwImpl => "`impl`".into(),
            Token::KwImport => "`import`".into(),
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
            Token::LBrace => "`{`".into(),
            Token::RBrace => "`}`".into(),
            Token::LBracket => "`[`".into(),
            Token::RBracket => "`]`".into(),
            Token::Comma => "`,`".into(),
            Token::Dot => "`.`".into(),
            Token::Colon => "`:`".into(),
            Token::FatArrow => "`=>`".into(),
            Token::Question => "`?`".into(),
            Token::Eof => "end of file".into(),
        }
    }

    /// The keywords that can start a variable declaration.
    pub(crate) fn is_type_keyword(&self) -> bool {
        matches!(
            self,
            Token::KwInt
                | Token::KwFloat
                | Token::KwStr
                | Token::KwBool
                | Token::KwByte
                | Token::KwInfer
        )
    }

    /// The tokens that can head a statement so decisively that a newline
    /// before them must stay significant even after a dangling fragment
    /// (the strip pass uses this to keep broken lines from fusing with the
    /// declaration typed below them).
    pub(crate) fn starts_statement(&self) -> bool {
        self.is_type_keyword()
            || matches!(
                self,
                Token::KwVoid
                    | Token::KwStruct
                    | Token::KwEnum
                    | Token::KwMatch
                    | Token::KwFor
                    | Token::KwImpl
                    | Token::KwImport
            )
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let LexError::ReservedIdent { name, .. } = self {
            return write!(f, "identifier `{name}` is reserved for future use");
        }
        let msg = match self {
            LexError::InvalidCharacter { .. } => "invalid character",
            LexError::UnterminatedString { .. } => "unterminated string literal",
            LexError::UnterminatedInterpolatedString { .. } => {
                "unterminated interpolated string literal"
            }
            LexError::UnterminatedBlockComment { .. } => "unterminated block comment",
            LexError::InvalidEscape { .. } => "invalid escape sequence in string literal",
            LexError::IntegerOverflow { .. } => "integer literal is too large",
            LexError::FloatOverflow { .. } => "float literal is too large",
            LexError::ReservedIdent { .. } => unreachable!("handled above"),
        };
        f.write_str(msg)
    }
}

/// Chooses the `LexError` variant for a failed region by inspecting the source
/// text: a leading `"` means the failure is inside a string literal (an
/// invalid escape, or an unterminated literal); an all-digit run is integer
/// overflow; a `digits.digits` shape is float overflow; anything else is a bad
/// character.
fn classify_error(source: &str, span: Span) -> LexError {
    let text = &source[span.start..span.end];
    // A failed region beginning with `/*` is a block comment the regex
    // could not terminate: everything from there on is comment.
    if text.starts_with("/*") {
        return LexError::UnterminatedBlockComment { span };
    }
    // A failed region shaped like the reserved `mega<N>` identifier is the
    // dedicated reservation error, not a generic bad character.
    if is_reserved_mega_ident(text) {
        return LexError::ReservedIdent {
            span,
            name: text.to_string(),
        };
    }
    if source[span.start..].starts_with("$\"") {
        return classify_interp_string_error(source, span.start);
    }
    if text.starts_with('"')
        && let Some(error) = classify_string_error(source, span.start)
    {
        return error;
    }
    // The scan reached a closing quote without incident, so the failure
    // lies outside this literal; fall through to the shape rules below.
    if is_int_shape(text) {
        LexError::IntegerOverflow { span }
    } else if is_float_shape(text) {
        LexError::FloatOverflow { span }
    } else {
        LexError::InvalidCharacter { span }
    }
}

/// Rescans a string literal beginning at `start` (the opening quote) to find
/// why the string regex failed. A backslash not followed by one of `n`, `t`,
/// `\`, `"` is an [`LexError::InvalidEscape`] covering the escape pair; a
/// line break or the end of file before the closing quote is an
/// [`LexError::UnterminatedString`] covering the quoted region. Returns `None`
/// when the literal closes cleanly (the failure then lies elsewhere).
fn classify_string_error(source: &str, start: usize) -> Option<LexError> {
    let mut cursor = start + 1;
    let mut chars = source[cursor..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return None,
            '\n' | '\r' => {
                return Some(LexError::UnterminatedString {
                    span: Span::new(start, cursor),
                });
            }
            '\\' => {
                let escape_start = cursor;
                cursor += 1;
                let next = chars.next();
                let accepted = matches!(next, Some('n') | Some('t') | Some('\\') | Some('"'));
                if !accepted {
                    let end = match next {
                        Some(c) => cursor + c.len_utf8(),
                        None => cursor,
                    };
                    return Some(LexError::InvalidEscape {
                        span: Span::new(escape_start, end.min(source.len())),
                    });
                }
                cursor += next.map_or(0, |c| c.len_utf8());
            }
            _ => cursor += c.len_utf8(),
        }
    }
    Some(LexError::UnterminatedString {
        span: Span::new(start, source.len()),
    })
}

/// Decodes a raw string-literal token slice (including the delimiting
/// quotes) into its value: strips the quotes and replaces the four accepted
/// escape pairs (`\n`, `\t`, `\\`, `\"`) with the characters they denote.
/// The lexer guarantees valid escapes, so a stray backslash is kept verbatim
/// rather than panicking on unexpected input.
pub fn unescape_str_lit(raw: &str) -> String {
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('n') => {
                chars.next();
                out.push('\n');
            }
            Some('t') => {
                chars.next();
                out.push('\t');
            }
            Some('\\') => {
                chars.next();
                out.push('\\');
            }
            Some('"') => {
                chars.next();
                out.push('"');
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// True when `text` is a full integer-literal shape (§2.4): a decimal run
/// with `_` separators between digits, or a radix-prefixed run (`0x`
/// hex, `0o` octal, `0b` binary). Used by [`classify_error`] to pin a
/// failed callback to `IntegerOverflow` — the only way a well-shaped
/// integer token can fail is an `i64` overflow.
fn is_int_shape(text: &str) -> bool {
    let (radix, digits) = if let Some(rest) = text.get(2..) {
        match &text[..2] {
            "0x" | "0X" => (16u32, rest),
            "0o" | "0O" => (8, rest),
            "0b" | "0B" => (2, rest),
            _ => (10, text),
        }
    } else {
        (10, text)
    };
    separated_digits(digits, radix)
}

/// True when `text` is a full float-literal shape (§2.4): a fractional
/// part with optional `_` separators and/or an exponent (`e`/`E`, optional
/// sign, digit field with separators). Used by [`classify_error`] to pin a
/// failed callback to `FloatOverflow` — the only way a well-shaped float
/// token can fail is an infinite value. Only reached after
/// [`is_int_shape`] declined, so a bare digit run (which is both a valid
/// decimal integer shape and the mantissa-only float form) is already
/// reported as an integer overflow before this can see it.
fn is_float_shape(text: &str) -> bool {
    let (mantissa, exponent) = match split_float_exponent(text) {
        Some((m, e)) => (m, e),
        None => (text, ""),
    };
    if !exponent.is_empty()
        && !separated_digits(exponent.strip_prefix(['+', '-']).unwrap_or(exponent), 10)
    {
        return false;
    }
    // The mantissa is either `digits.digits` or bare digits (the
    // exponent-only float form).
    match mantissa.split_once('.') {
        Some((int_part, frac_part)) => {
            !int_part.is_empty()
                && !frac_part.is_empty()
                && separated_digits(int_part, 10)
                && separated_digits(frac_part, 10)
        }
        None => separated_digits(mantissa, 10),
    }
}

/// Splits `text` at the `e`/`E` that starts a float exponent, returning
/// `(mantissa, exponent)`. `None` when there is no exponent (the `e`
/// boundary search must only fire at a real exponent: it requires digits
/// or a sign-and-digits tail).
fn split_float_exponent(text: &str) -> Option<(&str, &str)> {
    let byte_index = text
        .char_indices()
        .skip(1) // an exponent cannot start the literal
        .find(|(_, c)| matches!(c, 'e' | 'E'))
        .map(|(index, _)| index)?;
    let (mantissa, exponent) = (&text[..byte_index], &text[byte_index + 1..]);
    let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
        return None;
    }
    Some((mantissa, exponent))
}

/// True when `text` is a nonempty digit run in `radix` where every `_`
/// sits strictly between two digits — the separator discipline of §2.4.
fn separated_digits(text: &str, radix: u32) -> bool {
    let mut previous_was_digit = false;
    let mut any_digit = false;
    for c in text.chars() {
        match c {
            '_' if previous_was_digit => previous_was_digit = false,
            _ => {
                if !c.is_digit(radix) {
                    return false;
                }
                previous_was_digit = true;
                any_digit = true;
            }
        }
    }
    // A trailing `_` (separator not followed by a digit) is not a number.
    any_digit && previous_was_digit
}

/// Recognizes the dead fractional-float extent: separated decimal digits
/// followed by exactly one `.` (the dot the float pattern consumed while
/// waiting for a fraction digit that never came — `1.length`, `1.`,
/// `1..2`). Returns the integer value of the digit run so the driver can
/// split the region into `IntLit` + `Dot` without a diagnostic. Radix
/// prefixes never reach here: `0x1.` keeps its hex leaf and loses only at
/// the dot, where the integer leaf still covers the extent.
fn split_member_access_digits(text: &str) -> Option<i64> {
    let digits = text.strip_suffix('.')?;
    if digits.is_empty() || !separated_digits(digits, 10) {
        return None;
    }
    parse_int_slice(digits)
}

/// Recognizes the dead incomplete-exponent extent: a valid numeric prefix
/// followed by an exponent opener that never got its digits (`1.5e`,
/// `1.5E+`). Returns the byte length of the valid prefix so the driver can
/// emit it as one token and let the `e`/`E` (plus any sign) lex as an
/// identifier and operator. `1e` alone does NOT reach here — the digit-led
/// identifier leaf covers that extent, so it stays one name.
fn split_incomplete_exponent(text: &str) -> Option<usize> {
    let (head, tail) = text.split_once(['e', 'E'])?;
    // The tail must be exactly the optional sign (or nothing): any digit
    // after the opener means the float pattern itself accepted or a real
    // error lies elsewhere.
    let tail = tail.strip_prefix(['+', '-']).unwrap_or(tail);
    if !tail.is_empty() {
        return None;
    }
    if head.contains('.') {
        let (int_part, frac_part) = head.split_once('.')?;
        if separated_digits(int_part, 10)
            && separated_digits(frac_part, 10)
            && parse_f64_prefix(head).is_some()
        {
            return Some(head.len());
        }
        None
    } else if separated_digits(head, 10) && parse_int_slice(head).is_some() {
        Some(head.len())
    } else {
        None
    }
}

/// Parses a cleaned numeric prefix for [`split_incomplete_exponent`].
fn parse_f64_prefix(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// The token kind for a numeric prefix: float when the text has a dot
/// (fractional form), integer otherwise.
fn numeric_prefix_token(prefix: &str) -> Token<'_> {
    if prefix.contains('.') {
        Token::FloatLit(prefix.parse::<f64>().unwrap_or(f64::NAN))
    } else {
        Token::IntLit(parse_int_slice(prefix).unwrap_or(0))
    }
}

/// Fails fast on the first lex error. For tolerant parsing, prefer
/// [`crate::parse_source`].
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
            // Block comments are transparent to the token stream (§2.2).
            Ok(Token::BlockComment) => {}
            Ok(token) => tokens.push(SpannedToken { token, span }),
            Err(()) => {
                // The fractional-float pattern walks through `digits.` while
                // waiting for a fraction digit; when none follows (`1.length`,
                // `1.`, `1..2`), the pattern dies over the `digits.` extent.
                // No identifier leaf can cover that extent (identifiers cannot
                // contain the dot), so logos reports it as one error region —
                // even though the integer leaf one character earlier is the
                // match the user meant. Split the extent back into the integer
                // literal and the dot, and resume lexing right after it.
                if let Some(value) = split_member_access_digits(&source[span.start..span.end]) {
                    tokens.push(SpannedToken {
                        token: Token::IntLit(value),
                        span: Span::new(span.start, span.end - 1),
                    });
                    tokens.push(SpannedToken {
                        token: Token::Dot,
                        span: Span::new(span.end - 1, span.end),
                    });
                    continue;
                }
                // The same walk happens through the exponent opener: `1.5e`
                // (optionally `1.5e+`) dies waiting for exponent digits. No
                // leaf covers the extent, so split off the float the user
                // wrote and let the `e` (and any sign) lex as an identifier
                // and operator, exactly the graceful degradation of the
                // decimal-only lexer.
                if let Some(prefix_len) = split_incomplete_exponent(&source[span.start..span.end]) {
                    let prefix = &source[span.start..span.start + prefix_len];
                    tokens.push(SpannedToken {
                        token: numeric_prefix_token(prefix),
                        span: Span::new(span.start, span.start + prefix_len),
                    });
                    tokens.push(SpannedToken {
                        token: Token::Ident(
                            if source.as_bytes()[span.start + prefix_len] == b'e' {
                                "e"
                            } else {
                                "E"
                            },
                        ),
                        span: Span::new(span.start + prefix_len, span.start + prefix_len + 1),
                    });
                    continue;
                }
                let error = classify_error(source, span);
                let is_unterminated_comment =
                    matches!(error, LexError::UnterminatedBlockComment { .. });
                let is_unterminated_interp =
                    matches!(error, LexError::UnterminatedInterpolatedString { .. });
                errors.push(error);
                // An unterminated block comment swallows the rest of the
                // file: stop lexing rather than recovering into the middle
                // of the comment.
                if is_unterminated_comment {
                    break;
                }
                if is_unterminated_interp {
                    // The interpolated error already spans the rest of the
                    // line: resynchronize silently so the `"` remainder does
                    // not report a second unterminated-string error.
                    if let Some(newline_span) = skip_to_line_end_silent(&mut lexer) {
                        tokens.push(SpannedToken {
                            token: Token::Newline,
                            span: newline_span,
                        });
                    }
                    continue;
                }
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

/// Silent variant for an already-spanning interpolated-string error: drops
/// the rest of the line without recording further lexer errors.
fn skip_to_line_end_silent<'src>(lexer: &mut logos::Lexer<'src, Token<'src>>) -> Option<Span> {
    while let Some(result) = lexer.next() {
        let span = Span::new(lexer.span().start, lexer.span().end);
        if matches!(result, Ok(Token::Newline)) {
            return Some(span);
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
        let source = "int float infer return if else while void str bool true false";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::KwInt,
                Token::KwFloat,
                Token::KwInfer,
                Token::KwReturn,
                Token::KwIf,
                Token::KwElse,
                Token::KwWhile,
                Token::KwVoid,
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
        let source = "= ( ) { } , [ ] . : => ?";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::Assign,
                Token::LParen,
                Token::RParen,
                Token::LBrace,
                Token::RBrace,
                Token::Comma,
                Token::LBracket,
                Token::RBracket,
                Token::Dot,
                Token::Colon,
                Token::FatArrow,
                Token::Question,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_new_keywords() {
        let source = "struct enum match for in impl import";
        assert_eq!(
            lex_ok(source),
            vec![
                Token::KwStruct,
                Token::KwEnum,
                Token::KwMatch,
                Token::KwFor,
                Token::KwIn,
                Token::KwImpl,
                Token::KwImport,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn import_takes_precedence_over_identifiers() {
        // `import` is a keyword only when it stands alone; a prefixed or
        // suffixed identifier keeps its spelling.
        assert_eq!(
            lex_ok("imports import_ imported"),
            vec![
                Token::Ident("imports"),
                Token::Ident("import_"),
                Token::Ident("imported"),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn lexes_interpolated_string_literals() {
        assert_eq!(
            lex_tokens("$\"hello\""),
            vec![Token::InterpStrLit("$\"hello\""), Token::Eof]
        );
        // A `$` that does not open a string is still a bad character.
        assert!(lex("$").is_err());
        // Escape validation is identical to plain strings (§2.2).
        let (tokens, errors) = crate::lexer::lex_with_errors("$\"bad \\q\"");
        assert_eq!(errors.len(), 1);
        assert!(matches!(tokens.last().map(|t| t.token), Some(Token::Eof)));
    }

    #[test]
    fn interpolated_string_islands_may_contain_string_literals() {
        // A quoted string inside an island must not close the outer
        // literal: the whole `$"..."` form is one token.
        assert_eq!(
            lex_tokens("$\"ages: { table[\"ana\"] }\""),
            vec![
                Token::InterpStrLit("$\"ages: { table[\"ana\"] }\""),
                Token::Eof
            ]
        );
        // Braces inside the island's strings do not affect balancing.
        assert_eq!(
            lex_tokens("$\"a { m[\"}\"] } b\""),
            vec![Token::InterpStrLit("$\"a { m[\"}\"] } b\""), Token::Eof]
        );
        // Escaped quotes inside island strings are honored.
        assert_eq!(
            lex_tokens("$\"x { s[\"a\\\"b\"] } y\""),
            vec![
                Token::InterpStrLit("$\"x { s[\"a\\\"b\"] } y\""),
                Token::Eof
            ]
        );
        // Escapes outside islands validate exactly like plain strings.
        let (tokens, errors) = lex_with_errors("$\"ok { a[\"n\"] } \\t\"");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            tokens.first().map(|t| t.token),
            Some(Token::InterpStrLit("$\"ok { a[\"n\"] } \\t\""))
        );
    }

    #[test]
    fn broken_interp_string_recovery_is_line_granular() {
        // One diagnostic for a bad escape: the failed token swallows the
        // rest of the line so the `q` and the dangling quote behind the
        // backslash produce no phantom errors.
        let (tokens, errors) = lex_with_errors("$\"bad \\q\" tail");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0],
            LexError::InvalidEscape {
                span: Span::new(6, 8)
            }
        );
        // The recovery newline carries the rest of the line; nothing else.
        assert!(matches!(tokens.last().map(|t| t.token), Some(Token::Eof)));

        // Unterminated literal: one diagnostic, resync at the line end.
        let (tokens, errors) = lex_with_errors("$\"never { closes\nint x = 1\n");
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0],
            LexError::UnterminatedInterpolatedString { .. }
        ));
        assert!(
            tokens.iter().any(|t| t.token == Token::KwInt),
            "the next line's tokens survive recovery"
        );
    }

    #[test]
    fn block_comments_are_transparent() {
        assert_eq!(
            lex_tokens("a /* skip me */ b"),
            vec![Token::Ident("a"), Token::Ident("b"), Token::Eof]
        );
        // Multi-line comments vanish, newline tokens included.
        assert_eq!(
            lex_tokens("a /* multi\nline\nblock */ b"),
            vec![Token::Ident("a"), Token::Ident("b"), Token::Eof]
        );
        // Star-only comments and comments whose content contains stars.
        assert_eq!(
            lex_tokens("x /***/ y /* a ** b */ z"),
            vec![
                Token::Ident("x"),
                Token::Ident("y"),
                Token::Ident("z"),
                Token::Eof
            ]
        );
        // The comment ends at the FIRST `*/`; the rest is code.
        assert_eq!(
            lex_tokens("/* c */ mid */"),
            vec![Token::Ident("mid"), Token::Star, Token::Slash, Token::Eof]
        );
        // Comments do not disturb statement-terminating newlines.
        let (tokens, _) = crate::lexer::lex_with_errors("int a = 1 /* tail */\nint b = 2\n");
        assert!(tokens.iter().any(|spanned| spanned.token == Token::Newline));
    }

    #[test]
    fn unterminated_block_comment_is_one_error_and_stops_lexing() {
        let (tokens, errors) =
            crate::lexer::lex_with_errors("int a = 1\n/* never closed\nint b = 2\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0],
            LexError::UnterminatedBlockComment {
                span: Span::new(10, 12)
            }
        );
        // The comment swallows the rest of the file: only the tokens before
        // it (plus Eof) survive.
        assert_eq!(tokens.len(), 6); // int a = 1 newline eof
    }

    #[test]
    fn line_comment_star_slash_stay_lexable() {
        // `//` comments already ignore `/*` inside them (the skip regex eats
        // to end of line), and division still lexes next to `*`-free text.
        assert_eq!(
            lex_tokens("// /* not a block\nint x"),
            vec![Token::Newline, Token::KwInt, Token::Ident("x"), Token::Eof]
        );
    }

    #[test]
    fn describes_every_token_for_diagnostics() {
        let cases: Vec<(Token<'_>, &str)> = vec![
            (Token::Ident("x"), "identifier `x`"),
            (Token::StrLit("\"x\""), "string literal"),
            (Token::InterpStrLit("$\"x\""), "interpolated string literal"),
            (Token::IntLit(42), "integer literal `42`"),
            (Token::FloatLit(4.2), "float literal `4.2`"),
            (Token::Newline, "end of statement"),
            (Token::KwInt, "`int`"),
            (Token::KwFloat, "`float`"),
            (Token::KwStr, "`str`"),
            (Token::KwBool, "`bool`"),
            (Token::KwInfer, "`infer`"),
            (Token::KwReturn, "`return`"),
            (Token::KwIf, "`if`"),
            (Token::KwElse, "`else`"),
            (Token::KwWhile, "`while`"),
            (Token::KwVoid, "`void`"),
            (Token::KwTrue, "`true`"),
            (Token::KwFalse, "`false`"),
            (Token::KwStruct, "`struct`"),
            (Token::KwEnum, "`enum`"),
            (Token::KwMatch, "`match`"),
            (Token::KwFor, "`for`"),
            (Token::KwIn, "`in`"),
            (Token::KwImpl, "`impl`"),
            (Token::KwImport, "`import`"),
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
            (Token::LBrace, "`{`"),
            (Token::RBrace, "`}`"),
            (Token::Comma, "`,`"),
            (Token::Dot, "`.`"),
            (Token::Colon, "`:`"),
            (Token::FatArrow, "`=>`"),
            (Token::Question, "`?`"),
            (Token::LBracket, "`[`"),
            (Token::RBracket, "`]`"),
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
        assert!(lex("@").is_err());
        // `1.2.3` used to fail at lex level because `.` had no token; with
        // field access (§2.6) `.` is a token, so the malformed shape now
        // lexes as `1.2` `.` `3` and is rejected later, at parse level.
        assert!(lex("1.2.3").is_ok());
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
    fn lexes_each_accepted_escape_inside_string_literals() {
        // The token carries the raw slice; unescaping happens in the parser.
        // The empty literal sits mid-source on purpose: a trailing `""`
        // would fuse with the raw-string delimiter.
        let source = r#""a\nb" "" "c\t" "d\\e" "f\"g" h"#;
        assert_eq!(
            lex_ok(source),
            vec![
                Token::StrLit("\"a\\nb\""),
                Token::StrLit("\"\""),
                Token::StrLit("\"c\\t\""),
                Token::StrLit("\"d\\\\e\""),
                Token::StrLit("\"f\\\"g\""),
                Token::Ident("h"),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn escaped_quote_does_not_terminate_a_string() {
        let source = r#""a\"b""#;
        assert_eq!(
            lex_ok(source),
            vec![Token::StrLit("\"a\\\"b\""), Token::Eof]
        );
    }

    #[test]
    fn invalid_escape_is_a_lex_error_pointing_at_the_escape_pair() {
        let source = "\"a\\qb\"\nint next = 1\n";
        let (tokens, errors) = lex_with_errors(source);
        assert_eq!(
            errors,
            vec![LexError::InvalidEscape {
                span: Span::new(2, 4)
            }]
        );
        // The damaged line degrades to a statement boundary; the next line
        // lexes untouched.
        assert_eq!(
            tokens
                .into_iter()
                .map(|spanned| spanned.token)
                .collect::<Vec<_>>(),
            vec![
                Token::Newline,
                Token::KwInt,
                Token::Ident("next"),
                Token::Assign,
                Token::IntLit(1),
                Token::Newline,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn trailing_bare_backslash_in_string_is_an_invalid_escape() {
        let source = "\"abc\\";
        let (_, errors) = lex_with_errors(source);
        assert_eq!(
            errors,
            vec![LexError::InvalidEscape {
                span: Span::new(4, 5)
            }]
        );
    }

    #[test]
    fn unescape_decodes_each_accepted_escape() {
        assert_eq!(super::unescape_str_lit(r#""a\nb""#), "a\nb");
        assert_eq!(super::unescape_str_lit(r#""c\t""#), "c\t");
        assert_eq!(super::unescape_str_lit(r#""d\\e""#), "d\\e");
        assert_eq!(super::unescape_str_lit(r#""f\"g""#), "f\"g");
        assert_eq!(super::unescape_str_lit("\"\""), "");
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

#[cfg(test)]
mod reserved_ident_tests {
    use crate::lexer::{LexError, lex, lex_with_errors};
    use cme_core::Span;

    #[test]
    fn reserved_mega_identifiers_fail_with_the_dedicated_diagnostic() {
        let (tokens, errors) = lex_with_errors("int mega0 = 1\nint b = 2\n");
        assert_eq!(errors.len(), 1, "one reserved-ident diagnostic");
        assert_eq!(
            errors[0],
            LexError::ReservedIdent {
                span: Span::new(4, 9),
                name: "mega0".to_string(),
            }
        );
        assert_eq!(
            errors[0].to_string(),
            "identifier `mega0` is reserved for future use"
        );
        // Line-granular recovery: the damaged line is dropped, the next
        // line survives.
        let surviving: Vec<_> = tokens.into_iter().map(|spanned| spanned.token).collect();
        assert!(surviving.contains(&crate::lexer::Token::Ident("b")));
        assert!(!surviving.contains(&crate::lexer::Token::Ident("int")));
    }

    #[test]
    fn every_mega_digit_spelling_is_reserved() {
        for name in ["mega0", "mega1", "mega42", "mega000"] {
            let (_, errors) = lex_with_errors(&format!("int {name} = 1\n"));
            assert!(
                matches!(&errors[0], LexError::ReservedIdent { name: n, .. } if n == name),
                "`{name}` must be reserved"
            );
        }
    }

    #[test]
    fn mega_prefixed_non_numeric_identifiers_stay_legal() {
        let tokens =
            lex("mega mega0x mega_0 mega42a megaX megaa1").expect("none of these are reserved");
        let names: Vec<_> = tokens
            .into_iter()
            .filter_map(|spanned| match spanned.token {
                crate::lexer::Token::Ident(name) => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            ["mega", "mega0x", "mega_0", "mega42a", "megaX", "megaa1"]
        );
    }

    #[test]
    fn reserved_identifiers_are_rejected_in_every_position() {
        // Function name, field name, and struct name — the reservation is
        // lexical, so every ident position rejects the shape.
        for source in ["void mega1() {}", "struct mega1 { int x }", "int mega2 = 0"] {
            let (_, errors) = lex_with_errors(source);
            assert!(
                matches!(&errors[0], LexError::ReservedIdent { .. }),
                "`{source}` must report the reserved shape"
            );
        }
    }
}

#[cfg(test)]
mod digit_start_ident_tests {
    use crate::lexer::{Token, lex, lex_with_errors};

    #[test]
    fn digit_led_identifiers_with_a_non_digit_lex_as_one_ident() {
        let tokens = lex("int 3Vector = 1\nfloat 2D = 2.0\nbool _2fast = true\n")
            .expect("digit-led idents are legal");
        let names: Vec<_> = tokens
            .into_iter()
            .filter_map(|spanned| match spanned.token {
                Token::Ident(name) => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["3Vector", "2D", "_2fast"]);
    }

    #[test]
    fn pure_digit_runs_stay_integer_literals() {
        let tokens = lex("int x = 123").expect("a digit run is an int literal");
        assert!(
            tokens
                .iter()
                .any(|spanned| matches!(spanned.token, Token::IntLit(123)))
        );
        assert!(
            !tokens
                .iter()
                .any(|spanned| matches!(spanned.token, Token::Ident("123"))),
            "a pure digit run never scans as an identifier"
        );
    }

    #[test]
    fn float_literals_still_scan_through_a_digit_dot_digit() {
        let tokens = lex("float x = 2.5").expect("float literals unchanged");
        assert!(
            tokens.iter().any(
                |spanned| matches!(spanned.token, Token::FloatLit(v) if (v - 2.5).abs() < 1e-9)
            )
        );
    }

    #[test]
    fn a_digit_run_with_a_trailing_letter_becomes_one_ident() {
        // `123abc` used to lex as INT(123) + IDENT(abc); the longest-match
        // digit-led ident now consumes the whole run.
        let tokens = lex("int 123abc = 1").expect("123abc is an identifier");
        let names: Vec<_> = tokens
            .into_iter()
            .filter_map(|spanned| match spanned.token {
                Token::Ident(name) => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["123abc"]);
    }

    #[test]
    fn reserved_mega_shape_is_rejected_in_digit_form_too() {
        // `mega0` cannot start with a digit, but the digit-led ident path
        // still feeds the same callback: `0mega` is a legal ident.
        let (_, errors) = lex_with_errors("int mega0 = 1\n");
        assert!(!errors.is_empty(), "mega0 stays reserved");
        let tokens = lex("int 0mega = 1").expect("0mega is a legal identifier");
        assert!(
            tokens
                .iter()
                .any(|spanned| matches!(spanned.token, Token::Ident("0mega")))
        );
    }
}

#[cfg(test)]
mod numeric_literal_tests {
    use super::Token;
    use crate::lexer::{LexError, lex, lex_with_errors};
    use cme_core::Span;

    // ------------------------------------------------------------------
    // Numeric literals (§2): radix prefixes, digit separators, exponents,
    // and the collision rules against the digit-led identifier shape.
    // ------------------------------------------------------------------

    fn lex_ints(source: &str) -> Vec<i64> {
        lex(source)
            .expect("source should lex")
            .into_iter()
            .filter_map(|spanned| match spanned.token {
                Token::IntLit(value) => Some(value),
                _ => None,
            })
            .collect()
    }

    fn lex_ids(source: &str) -> Vec<&str> {
        lex(source)
            .expect("source should lex")
            .into_iter()
            .filter_map(|spanned| match spanned.token {
                Token::Ident(name) => Some(name),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn hex_integers_lex_in_both_prefix_cases() {
        assert_eq!(
            lex_ints("0x0 0xff 0XFF 0xAbCdEf 0x7F"),
            vec![0, 255, 255, 11259375, 127]
        );
    }

    #[test]
    fn octal_and_binary_integers_lex_in_both_prefix_cases() {
        assert_eq!(lex_ints("0o0 0o7 0O777"), vec![0, 7, 511]);
        assert_eq!(lex_ints("0b0 0b1 0B1010"), vec![0, 1, 10]);
    }

    #[test]
    fn i64_extremes_in_every_radix() {
        assert_eq!(lex_ints("9223372036854775807"), vec![i64::MAX]);
        assert_eq!(lex_ints("0x7FFFFFFFFFFFFFFF"), vec![i64::MAX]);
        assert_eq!(lex_ints("0o777777777777777777777"), vec![i64::MAX]);
        assert_eq!(
            lex_ints("0b0111111111111111111111111111111111111111111111111111111111111111"),
            vec![i64::MAX]
        );
    }

    #[test]
    fn separators_group_decimal_digits() {
        assert_eq!(
            lex_ints("1_000 1_000_000 0_1 12_345_678"),
            vec![1000, 1000000, 1, 12345678]
        );
    }

    #[test]
    fn separators_group_radix_digits() {
        assert_eq!(
            lex_ints("0xFF_FF 0x1_F 0o7_7 0b1_0_1"),
            vec![65535, 31, 63, 5]
        );
    }

    #[test]
    fn separator_discipline_falls_back_to_identifiers() {
        // A separator must sit strictly between two digits; anything else
        // degrades to the longest-match identifier, never a partial number.
        assert_eq!(
            lex_ids("1__0 1_ _1 1_000_"),
            vec!["1__0", "1_", "_1", "1_000_"]
        );
    }

    #[test]
    fn invalid_radix_digits_fall_back_to_identifiers() {
        assert_eq!(
            lex_ids("0b12 0b2 0o8 0o_7 0b_1 0x_1"),
            vec!["0b12", "0b2", "0o8", "0o_7", "0b_1", "0x_1"]
        );
    }

    #[test]
    fn a_broken_radix_word_never_splits_into_two_tokens() {
        // The whole digit-led word scans as ONE identifier: `0x1G` is not
        // the literal `0x1` followed by a name.
        assert_eq!(
            lex_ids("0x1G 0xFFg 0b101z"),
            vec!["0x1G", "0xFFg", "0b101z"]
        );
    }

    #[test]
    fn a_bare_prefix_stays_an_identifier() {
        assert_eq!(lex_ids("0x 0b 0o 0X"), vec!["0x", "0b", "0o", "0X"]);
    }

    #[test]
    fn whitepaper_digit_led_idents_survive_the_numeric_rules() {
        assert_eq!(
            lex_ids("3Vector 2D 2_D 123abc 0mega"),
            vec!["3Vector", "2D", "2_D", "123abc", "0mega"]
        );
    }

    #[test]
    fn floats_carry_exponents() {
        assert_eq!(lex_ints("1e10"), vec![]);
        let floats: Vec<f64> = lex("1e10 1E10 1e+10 1e-10 1.5e3 1.5E-3 2.5e0")
            .expect("floats lex")
            .into_iter()
            .filter_map(|spanned| match spanned.token {
                Token::FloatLit(value) => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(floats, vec![1e10, 1e10, 1e10, 1e-10, 1500.0, 0.0015, 2.5]);
    }

    #[test]
    fn floats_take_separators() {
        let floats: Vec<f64> = lex("1_000.5 1.500_001e1_0")
            .expect("floats lex")
            .into_iter()
            .filter_map(|spanned| match spanned.token {
                Token::FloatLit(value) => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(floats, vec![1000.5, 15000010000.0]);
    }

    #[test]
    fn an_incomplete_exponent_degrades_gracefully() {
        // `1.5e` is the float `1.5` followed by the identifier `e`; `1e`
        // stays one digit-led identifier; `1e+` is the identifier `1e`
        // plus a Plus token.
        assert_eq!(
            lex("1.5e")
                .expect("lexes")
                .iter()
                .map(|t| t.token)
                .collect::<Vec<_>>(),
            vec![Token::FloatLit(1.5), Token::Ident("e"), Token::Eof]
        );
        assert_eq!(lex_ids("1e 1E"), vec!["1e", "1E"]);
    }

    #[test]
    fn a_digit_dot_word_is_member_access_not_a_float() {
        // `1.length` must remain IntLit + Dot + Ident (field access on a
        // literal): the dead float extent is split by the driver, never
        // reported as an error.
        let tokens = lex("1.length").expect("member access lexes");
        assert_eq!(
            tokens.into_iter().map(|t| t.token).collect::<Vec<_>>(),
            vec![
                Token::IntLit(1),
                Token::Dot,
                Token::Ident("length"),
                Token::Eof
            ]
        );
    }

    #[test]
    fn radix_literal_dot_is_still_member_access() {
        let tokens = lex("0xFF.5").expect("lexes");
        assert_eq!(
            tokens.into_iter().map(|t| t.token).collect::<Vec<_>>(),
            vec![Token::IntLit(255), Token::Dot, Token::IntLit(5), Token::Eof]
        );
        let tokens = lex("1_000.5").expect("lexes");
        assert_eq!(
            tokens.into_iter().map(|t| t.token).collect::<Vec<_>>(),
            vec![Token::FloatLit(1000.5), Token::Eof]
        );
    }

    #[test]
    fn a_dangling_digit_dot_yields_int_then_dot() {
        assert_eq!(
            lex("return 1.\n")
                .expect("lexes")
                .into_iter()
                .map(|t| t.token)
                .collect::<Vec<_>>(),
            vec![
                Token::KwReturn,
                Token::IntLit(1),
                Token::Dot,
                Token::Newline,
                Token::Eof
            ]
        );
        assert_eq!(
            lex("1..2")
                .expect("lexes")
                .into_iter()
                .map(|t| t.token)
                .collect::<Vec<_>>(),
            vec![
                Token::IntLit(1),
                Token::Dot,
                Token::Dot,
                Token::IntLit(2),
                Token::Eof
            ]
        );
    }

    #[test]
    fn hex_overflow_is_a_compile_time_error() {
        let (_, errors) = lex_with_errors("0x8000000000000000");
        assert_eq!(
            errors,
            vec![LexError::IntegerOverflow {
                span: Span::new(0, 18)
            }]
        );
        let (_, errors) = lex_with_errors("0xFFFFFFFFFFFFFFFF");
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], LexError::IntegerOverflow { .. }));
        let (_, errors) =
            lex_with_errors("0b1000000000000000000000000000000000000000000000000000000000000000");
        assert!(matches!(errors[0], LexError::IntegerOverflow { .. }));
    }

    #[test]
    fn decimal_and_separator_overflow_is_a_compile_time_error() {
        let (_, errors) = lex_with_errors("99999999999999999999");
        assert!(matches!(errors[0], LexError::IntegerOverflow { .. }));
        let (_, errors) = lex_with_errors("9_999_999_999_999_999_999_9");
        assert!(matches!(errors[0], LexError::IntegerOverflow { .. }));
    }

    #[test]
    fn float_overflow_in_exponent_form_is_a_compile_time_error() {
        let (_, errors) = lex_with_errors("1e999");
        assert_eq!(
            errors,
            vec![LexError::FloatOverflow {
                span: Span::new(0, 5)
            }]
        );
        let (_, errors) = lex_with_errors("1.5e308x");
        assert!(
            errors.is_empty(),
            "`1.5e308x` is the identifier `1.5e308x`... no float dies here: {errors:?}"
        );
    }

    #[test]
    fn literal_spans_cover_the_whole_spelling() {
        let tokens = lex("0xFF_FF 1_000 1.5e-3").expect("lexes");
        let spans: Vec<(usize, usize)> = tokens
            .into_iter()
            .map(|t| (t.span.start, t.span.end))
            .collect();
        assert_eq!(spans[0], (0, 7));
        assert_eq!(spans[1], (8, 13));
        assert_eq!(spans[2], (14, 20));
    }

    #[test]
    fn numerics_win_length_ties_but_never_shorter_matches() {
        // Same-length ties resolve to the numeric rule (`0x1F`, `1_000`,
        // `1e10`); a longer identifier always wins (`0xFFg`).
        assert_eq!(lex_ints("0x1F 1_000 0b1_0 0o777"), vec![31, 1000, 2, 511]);
        assert_eq!(
            lex("1e10")
                .expect("lexes")
                .iter()
                .filter(|t| matches!(t.token, Token::FloatLit(_)))
                .count(),
            1,
            "`1e10` is a float"
        );
        assert_eq!(lex_ids("0xFFg 123abc"), vec!["0xFFg", "123abc"]);
    }

    #[test]
    fn leading_zero_decimals_keep_their_value() {
        assert_eq!(lex_ints("0 00 0123 007"), vec![0, 0, 123, 7]);
    }

    #[test]
    fn integer_describe_shows_the_value_not_the_spelling() {
        let described = Token::IntLit(255).describe();
        assert!(described.contains("255"), "{described}");
        assert!(!described.contains("0xFF"), "{described}");
    }
}
