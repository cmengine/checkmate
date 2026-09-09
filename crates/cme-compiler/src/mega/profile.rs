//! Lexical profiles and profile-aware scanning (WHITEPAPER §8.2, §8.6).
//!
//! A grammar's profile — its skip set, comment forms, and string forms —
//! drives two different consumers: the packrat matcher (§8.3) and the
//! invocation-region scanner (§8.6). This module provides the shared
//! mechanics: profile construction, profile extraction from grammar bodies,
//! composed-profile union, and delimiter balancing that honors comments,
//! strings, and string-island forms.

use crate::diagnostics::Diagnostic;
use cme_core::Span;
use cme_core::magic::{CharItem, CharSet, CommentForm, LexProfile, StringForm};

/// The default profile (§8.2, §8.6): horizontal and newline skipping, `"`
/// strings, no comments. Used for magics with inline entry patterns and as
/// the fallback for regions whose grammar profile declares no string forms.
pub fn default_profile() -> LexProfile {
    LexProfile {
        skip: CharSet {
            negated: false,
            items: vec![
                CharItem::Char(' '),
                CharItem::Char('\t'),
                CharItem::Char('\r'),
                CharItem::Char('\n'),
            ],
        },
        comments: Vec::new(),
        strings: vec![StringForm {
            quote: '"',
            multiline: false,
            island: None,
        }],
    }
}

/// The profile used to balance Checkmate-shaped regions: grammar bodies and
/// magic-declaration headers (pattern parens, template braces). It covers
/// every quoting form Checkmate pattern text uses, plus line and block
/// comments, so pattern literals like `"("` never confuse the balance.
pub fn checkmate_scan_profile() -> LexProfile {
    LexProfile {
        skip: default_profile().skip,
        comments: vec![CommentForm::line("//"), CommentForm::block("/*", "*/")],
        strings: vec![
            StringForm {
                quote: '"',
                multiline: false,
                island: None,
            },
            StringForm {
                quote: '\'',
                multiline: false,
                island: None,
            },
        ],
    }
}

/// Unions profiles: the base's forms plus every extra's forms. Used for the
/// composed profile of an invocation (§8.6): the entry grammar's profile and
/// the profiles of every grammar its entry pattern references.
pub fn compose(base: &LexProfile, extras: &[&LexProfile]) -> LexProfile {
    let mut composed = LexProfile {
        skip: base.skip.clone(),
        comments: base.comments.clone(),
        strings: base.strings.clone(),
    };
    for extra in extras {
        for comment in &extra.comments {
            if !composed.comments.contains(comment) {
                composed.comments.push(comment.clone());
            }
        }
        for string in &extra.strings {
            if !composed.strings.contains(string) {
                composed.strings.push(string.clone());
            }
        }
    }
    composed
}

/// Ensures a profile can balance regions that contain ordinary quoted text:
/// a grammar that declares no string forms still gets the default `"` form.
/// (JSON regions, for instance, need `"host": "…"` to be string-transparent
/// even though `grammar json` declares no `string` form.)
pub fn with_default_strings(profile: &LexProfile) -> LexProfile {
    if !profile.strings.is_empty() {
        return profile.clone();
    }
    let mut composed = profile.clone();
    composed.strings.push(StringForm {
        quote: '"',
        multiline: false,
        island: None,
    });
    composed
}

/// A delimiter-balancing failure: the opener's span plus a message.
pub struct BalanceError {
    pub span: Span,
    pub message: String,
}

/// Finds the byte offset of the delimiter matching the `open` character at
/// `open_pos`, honoring the profile's comment forms (longest opener first),
/// string forms (same rule; islands transparent), and brace counting.
pub fn balance(
    source: &str,
    open_pos: usize,
    open: char,
    close: char,
    profile: &LexProfile,
) -> Result<usize, BalanceError> {
    let mut cursor = open_pos + open.len_utf8();
    let mut depth = 1usize;
    while cursor < source.len() {
        if let Some(len) = comment_len(source, cursor, profile) {
            cursor += len;
            continue;
        }
        if let Some(len) = string_len(source, cursor, profile) {
            cursor += len;
            continue;
        }
        let c = source[cursor..].chars().next().unwrap_or(open);
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Ok(cursor);
            }
        }
        cursor += c.len_utf8();
    }
    Err(BalanceError {
        span: Span::new(open_pos, open_pos + open.len_utf8()),
        message: format!("unclosed `{open}`: no matching `{close}` before the end of the file"),
    })
}

/// The length of the comment form matching at `pos`, if any. Longest matching
/// opener wins; line comments run to (not including) the line terminator,
/// block comments through their closer (an unterminated block comment
/// swallows the rest of the text, mirroring the Checkmate lexer).
pub fn comment_len(source: &str, pos: usize, profile: &LexProfile) -> Option<usize> {
    let rest = &source[pos..];
    let form = profile
        .comments
        .iter()
        .filter(|form| rest.starts_with(&form.opener))
        .max_by_key(|form| form.opener.len())?;
    let after_opener = &rest[form.opener.len()..];
    match &form.closer {
        None => Some(
            form.opener.len()
                + after_opener
                    .find(['\n', '\r'])
                    .unwrap_or(after_opener.len()),
        ),
        Some(closer) => match after_opener.find(&**closer) {
            Some(index) => Some(form.opener.len() + index + closer.len()),
            None => Some(rest.len()),
        },
    }
}

/// The length of the string form whose quote character sits at `pos`, if any.
/// A single-line form that does not close before the line terminator is
/// ordinary text (§8.6 — an apostrophe in prose cannot swallow the file); a
/// multiline form that never closes swallows the rest of the text. Islands
/// are transparent: their content is skipped with brace balancing so nested
/// macro invocations inside interpolations cannot break the balance.
pub fn string_len(source: &str, pos: usize, profile: &LexProfile) -> Option<usize> {
    let first = source[pos..].chars().next()?;
    let form = profile.strings.iter().find(|form| form.quote == first)?;
    let mut cursor = pos + first.len_utf8();

    while cursor < source.len() {
        let rest = &source[cursor..];
        let c = rest.chars().next().unwrap();
        match c {
            '\\' => {
                let next = rest[1..].chars().next();
                match next {
                    // An escaped character pairs with the backslash; an
                    // escaped line break still breaks a single-line string.
                    Some(escaped) if escaped != '\n' && escaped != '\r' => {
                        cursor += 1 + escaped.len_utf8();
                    }
                    _ => return None,
                }
            }
            '\n' | '\r' if !form.multiline => return None,
            _ if c == form.quote => return Some(cursor + c.len_utf8() - pos),
            _ => {}
        }
        if let Some((open, close)) = &form.island
            && rest.starts_with(open.as_str())
        {
            let close_pos = balance_island(source, cursor + open.len(), close)?;
            cursor = close_pos + close.len();
            continue;
        }
        cursor += c.len_utf8();
    }
    if form.multiline {
        Some(source.len() - pos)
    } else {
        None
    }
}

/// Skips an island's content: from just after the island opener to just
/// before its closer, counting braces so nested Checkmate blocks (a nested
/// `magic(…) { … }` inside a template-literal island) stay transparent.
/// Public because the scanner pierces islands a second time to DISCOVER the
/// nested invocations inside them (§8.6).
pub fn balance_island(source: &str, start: usize, closer: &str) -> Option<usize> {
    let mut cursor = start;
    let mut depth = 1usize;
    while cursor < source.len() {
        // The island's own closer is the brace matching the opener: check
        // it BEFORE counting a `}` as a nested-brace decrement, or the
        // island swallows one `}` too many and never closes (§8.6).
        if source[cursor..].starts_with(closer) && depth == 1 {
            return Some(cursor);
        }
        if source[cursor..].starts_with('{') {
            depth += 1;
        } else if source[cursor..].starts_with('}') {
            depth -= 1;
        }
        cursor += source[cursor..].chars().next().unwrap().len_utf8();
    }
    None
}

// ---------------------------------------------------------------------------
// Profile extraction from grammar bodies
// ---------------------------------------------------------------------------

/// Parses the lexical profile declarations (`skip`, `comment`, `string`) from
/// a grammar body, skipping `rule` declarations wholesale. Bodies are
/// Checkmate-shaped, so `//` and `/* */` comments are transparent here.
pub fn parse_profile(body: &str, span: Span) -> Result<LexProfile, Diagnostic> {
    let scanner = checkmate_scan_profile();
    let mut profile = LexProfile::default();
    let mut cursor = 0usize;

    while cursor < body.len() {
        skip_ws_and_comments(body, &mut cursor, &scanner);
        if cursor >= body.len() {
            break;
        }
        let word = next_word(body, cursor);
        match word {
            "skip" => {
                cursor += word.len();
                skip_ws_and_comments(body, &mut cursor, &scanner);
                let set = parse_bracket_set(body, &mut cursor, span)?;
                profile.skip = set;
            }
            "comment" => {
                cursor += word.len();
                skip_ws_and_comments(body, &mut cursor, &scanner);
                let form = parse_comment_form(body, &mut cursor, span)?;
                profile.comments.push(form);
            }
            "string" => {
                cursor += word.len();
                skip_ws_and_comments(body, &mut cursor, &scanner);
                let form = parse_string_form(body, &mut cursor, span)?;
                profile.strings.push(form);
            }
            "rule" => {
                skip_rule_declaration(body, &mut cursor, &scanner);
            }
            _ => {
                // Unknown top-level word (a future profile declaration):
                // skip it so the scan stays robust to forward-compatible
                // grammar text.
                cursor += body[cursor..].chars().next().unwrap().len_utf8();
            }
        }
    }
    Ok(profile)
}

/// Skips whitespace and Checkmate-shaped comments from `cursor` onward.
pub(crate) fn skip_ws_and_comments(text: &str, cursor: &mut usize, profile: &LexProfile) {
    loop {
        while let Some(c) = text[*cursor..].chars().next() {
            if profile.skip.matches(c) {
                *cursor += c.len_utf8();
            } else {
                break;
            }
        }
        if let Some(len) = comment_len(text, *cursor, profile) {
            *cursor += len;
            continue;
        }
        break;
    }
}

/// The identifier-like word starting at `cursor`.
pub(crate) fn next_word(text: &str, cursor: usize) -> &str {
    let rest = &text[cursor..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    &rest[..end]
}

/// A quoted char literal (`'a'`, `'\t'`) at `cursor`; returns the decoded
/// character and the consumed length.
pub(crate) fn parse_char_literal(text: &str, cursor: usize) -> Option<(char, usize)> {
    let rest = &text[cursor..];
    let mut chars = rest.chars();
    if chars.next()? != '\'' {
        return None;
    }
    let mut consumed = 1usize;
    let value = match chars.next()? {
        '\\' => {
            consumed += 1;
            let escaped = chars.next()?;
            consumed += escaped.len_utf8();
            match escaped {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '0' => '\0',
                other => other,
            }
        }
        plain => {
            consumed += plain.len_utf8();
            plain
        }
    };
    if chars.next()? != '\'' {
        return None;
    }
    Some((value, consumed + 1))
}

/// A double-quoted string literal at `cursor` (with backslash escapes);
/// returns the decoded text and the consumed length.
pub(crate) fn parse_string_literal(text: &str, cursor: usize) -> Option<(String, usize)> {
    let rest = &text[cursor..];
    if !rest.starts_with('"') {
        return None;
    }
    let mut value = String::new();
    let mut cursor = 1usize;
    while let Some(c) = rest[cursor..].chars().next() {
        match c {
            '"' => return Some((value, cursor + 1)),
            '\\' => {
                let escaped = rest[cursor + 1..].chars().next()?;
                value.push(match escaped {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => other,
                });
                cursor += 1 + escaped.len_utf8();
            }
            other => {
                value.push(other);
                cursor += other.len_utf8();
            }
        }
    }
    None
}

/// The `[ … ]` character set of a `skip` declaration: quoted char literals
/// with optional commas, optionally `^`-negated.
fn parse_bracket_set(text: &str, cursor: &mut usize, span: Span) -> Result<CharSet, Diagnostic> {
    if !text[*cursor..].starts_with('[') {
        return Err(Diagnostic::parse(
            "expected `[` to open a skip set",
            offset_span(span, *cursor),
        ));
    }
    *cursor += 1;
    let mut set = CharSet::empty();
    loop {
        skip_ws_and_comments(text, cursor, &checkmate_scan_profile());
        match text[*cursor..].chars().next() {
            Some(']') => {
                *cursor += 1;
                return Ok(set);
            }
            Some('^') if set.items.is_empty() && !set.negated => {
                set.negated = true;
                *cursor += 1;
            }
            Some(',') => {
                *cursor += 1;
            }
            Some(_) => match parse_char_literal(text, *cursor) {
                Some((value, len)) => {
                    *cursor += len;
                    set.items.push(CharItem::Char(value));
                }
                None => {
                    return Err(Diagnostic::parse(
                        "expected a character literal in the skip set",
                        offset_span(span, *cursor),
                    ));
                }
            },
            None => {
                return Err(Diagnostic::parse(
                    "unterminated skip set: expected `]`",
                    offset_span(span, *cursor),
                ));
            }
        }
    }
}

/// A `comment ( "opener" [ until "closer" ] )` declaration.
fn parse_comment_form(
    text: &str,
    cursor: &mut usize,
    span: Span,
) -> Result<CommentForm, Diagnostic> {
    let scanner = checkmate_scan_profile();
    if !text[*cursor..].starts_with('(') {
        return Err(Diagnostic::parse(
            "expected `(` after `comment`",
            offset_span(span, *cursor),
        ));
    }
    *cursor += 1;
    skip_ws_and_comments(text, cursor, &scanner);
    let (opener, len) = parse_string_literal(text, *cursor).ok_or_else(|| {
        Diagnostic::parse(
            "expected a string literal for the comment opener",
            offset_span(span, *cursor),
        )
    })?;
    *cursor += len;
    skip_ws_and_comments(text, cursor, &scanner);
    let closer = if next_word(text, *cursor) == "until" {
        *cursor += "until".len();
        skip_ws_and_comments(text, cursor, &scanner);
        let (closer, len) = parse_string_literal(text, *cursor).ok_or_else(|| {
            Diagnostic::parse(
                "expected a string literal for the comment closer",
                offset_span(span, *cursor),
            )
        })?;
        *cursor += len;
        Some(closer)
    } else {
        None
    };
    skip_ws_and_comments(text, cursor, &scanner);
    if !text[*cursor..].starts_with(')') {
        return Err(Diagnostic::parse(
            "expected `)` to close the comment form",
            offset_span(span, *cursor),
        ));
    }
    *cursor += 1;
    Ok(match closer {
        Some(closer) => CommentForm::block(&opener, &closer),
        None => CommentForm::line(&opener),
    })
}

/// A `string ( '"' [multiline] [island ( "open" "close" )] )` declaration.
fn parse_string_form(text: &str, cursor: &mut usize, span: Span) -> Result<StringForm, Diagnostic> {
    let scanner = checkmate_scan_profile();
    if !text[*cursor..].starts_with('(') {
        return Err(Diagnostic::parse(
            "expected `(` after `string`",
            offset_span(span, *cursor),
        ));
    }
    *cursor += 1;
    skip_ws_and_comments(text, cursor, &scanner);
    // The delimiter is one character, spelled either as a char literal
    // (`'\''`) or as a one-character string literal (`"'"` — the §8.8
    // grammars use this spelling for quote characters).
    let (quote, len) = match parse_char_literal(text, *cursor) {
        Some((value, len)) => (value, len),
        None => match parse_string_literal(text, *cursor) {
            Some((value, len)) if value.chars().count() == 1 => {
                (value.chars().next().unwrap(), len)
            }
            _ => {
                return Err(Diagnostic::parse(
                    "expected a character literal for the string delimiter",
                    offset_span(span, *cursor),
                ));
            }
        },
    };
    *cursor += len;
    let mut form = StringForm {
        quote,
        multiline: false,
        island: None,
    };
    loop {
        skip_ws_and_comments(text, cursor, &scanner);
        // `)` is punctuation, not a word — check it before word matching.
        if text[*cursor..].starts_with(')') {
            *cursor += 1;
            return Ok(form);
        }
        let word = next_word(text, *cursor);
        match word {
            "multiline" => {
                form.multiline = true;
                *cursor += word.len();
            }
            "island" => {
                *cursor += word.len();
                skip_ws_and_comments(text, cursor, &scanner);
                if !text[*cursor..].starts_with('(') {
                    return Err(Diagnostic::parse(
                        "expected `(` after `island`",
                        offset_span(span, *cursor),
                    ));
                }
                *cursor += 1;
                skip_ws_and_comments(text, cursor, &scanner);
                let (open, len) = parse_string_literal(text, *cursor).ok_or_else(|| {
                    Diagnostic::parse(
                        "expected a string literal for the island opener",
                        offset_span(span, *cursor),
                    )
                })?;
                *cursor += len;
                skip_ws_and_comments(text, cursor, &scanner);
                let (close, len) = parse_string_literal(text, *cursor).ok_or_else(|| {
                    Diagnostic::parse(
                        "expected a string literal for the island closer",
                        offset_span(span, *cursor),
                    )
                })?;
                *cursor += len;
                skip_ws_and_comments(text, cursor, &scanner);
                if !text[*cursor..].starts_with(')') {
                    return Err(Diagnostic::parse(
                        "expected `)` to close the island form",
                        offset_span(span, *cursor),
                    ));
                }
                *cursor += 1;
                form.island = Some((open, close));
            }
            _ => {
                return Err(Diagnostic::parse(
                    format!("unexpected `{word}` in a string form"),
                    offset_span(span, *cursor),
                ));
            }
        }
    }
}

/// Skips a whole `rule name [( context … )] { body }` declaration, balancing
/// parens and braces so nested structures cannot confuse the profile scan.
fn skip_rule_declaration(text: &str, cursor: &mut usize, profile: &LexProfile) {
    *cursor += "rule".len();
    // The rule name sits between `rule` and the signature/body.
    skip_ws_and_comments(text, cursor, profile);
    *cursor += next_word(text, *cursor).len();
    // Optional `( context … )` signature (balanced parens).
    skip_ws_and_comments(text, cursor, profile);
    if text[*cursor..].starts_with('(') {
        let mut depth = 0i32;
        while *cursor < text.len() {
            if let Some(len) = string_len(text, *cursor, profile) {
                *cursor += len;
                continue;
            }
            let c = match text[*cursor..].chars().next() {
                Some(c) => c,
                None => return,
            };
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    *cursor += c.len_utf8();
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                _ => {}
            }
            *cursor += c.len_utf8();
        }
        skip_ws_and_comments(text, cursor, profile);
    }
    // The rule body: balanced braces down to the matching `}`.
    let mut depth = 0i32;
    while *cursor < text.len() {
        if let Some(len) = comment_len(text, *cursor, profile) {
            *cursor += len;
            continue;
        }
        if let Some(len) = string_len(text, *cursor, profile) {
            *cursor += len;
            continue;
        }
        let c = match text[*cursor..].chars().next() {
            Some(c) => c,
            None => return,
        };
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                *cursor += c.len_utf8();
                if depth == 0 {
                    return;
                }
                continue;
            }
            _ => {}
        }
        *cursor += c.len_utf8();
    }
}

/// Shifts `span` to point `offset` bytes into the region it covers, so
/// profile diagnostics point at the offending character inside the grammar
/// body instead of at the whole body.
fn offset_span(span: Span, offset: usize) -> Span {
    let start = (span.start + offset).min(span.end);
    Span::new(start, start)
}
