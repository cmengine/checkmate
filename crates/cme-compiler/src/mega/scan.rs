//! Magic detection (the front-end's only job for megaprograms, per the
//! owner's architecture mandate): locate `grammar` declarations, `magic`
//! declarations, and `magic(name) { … }` invocations in raw source text,
//! handing the code inside the magic blocks to the megaprogram pass.
//!
//! The scanner runs before the main lexer because magic regions are *not*
//! Checkmate — they are arbitrary foreign-language source that the main
//! lexer must never see. Regions are located by brace balancing under the
//! composed profile of the entry grammar (§8.6).

use crate::diagnostics::Diagnostic;
use crate::mega::profile::{
    balance, checkmate_scan_profile, comment_len, parse_profile, string_len, with_default_strings,
};
use cme_core::Span;
use cme_core::magic::LexProfile;

/// One `grammar name { … }` declaration: verbatim body text plus the profile
/// extracted from its `skip`/`comment`/`string` declarations.
#[derive(Debug, Clone)]
pub struct GrammarScan {
    pub name: String,
    /// The whole declaration, `grammar` through the closing `}`.
    pub span: Span,
    /// The text inside the braces.
    pub body_span: Span,
    pub body: String,
    pub profile: LexProfile,
}

/// One `magic name(pattern) { template }` declaration. Pattern and template
/// stay verbatim; the pattern/template parsers (Task 3) give them structure.
#[derive(Debug, Clone)]
pub struct MagicDeclScan {
    pub name: String,
    pub span: Span,
    pub pattern_span: Span,
    pub pattern: String,
    pub template_span: Span,
    pub template: String,
}

/// One `magic(name) { … }` invocation with its normalized region (§8.6):
/// one line terminator trimmed after `{` and before `}`, horizontal
/// whitespace trimmed at both edges. `region_span` stays anchored to the
/// original file so diagnostics keep pointing at the user's source.
#[derive(Debug, Clone)]
pub struct InvocationScan {
    pub name: String,
    /// The whole invocation, `magic` through the closing `}`.
    pub span: Span,
    /// The `magic ( name )` header, for diagnostics about the macro name.
    pub header_span: Span,
    /// The normalized region's extent in the original source.
    pub region_span: Span,
    pub region: String,
}

/// The result of scanning a source file for megaprogram constructs.
#[derive(Debug, Clone, Default)]
pub struct MagicScan {
    pub grammars: Vec<GrammarScan>,
    pub magics: Vec<MagicDeclScan>,
    pub invocations: Vec<InvocationScan>,
}

impl MagicScan {
    /// True when the file contains anything megaprogram-shaped. The CLI uses
    /// this as the fast path to skip expansion entirely.
    pub fn is_empty(&self) -> bool {
        self.grammars.is_empty() && self.magics.is_empty() && self.invocations.is_empty()
    }

    /// The profile of the named grammar.
    pub fn grammar_profile(&self, name: &str) -> Option<&LexProfile> {
        self.grammars
            .iter()
            .find(|grammar| grammar.name == name)
            .map(|grammar| &grammar.profile)
    }
}

/// Scans `source` for megaprogram constructs. Never fails: malformed
/// constructs produce diagnostics and the scan continues at the next line.
pub fn scan_magic(source: &str) -> (MagicScan, Vec<Diagnostic>) {
    let mut scan = MagicScan::default();
    let mut errors = Vec::new();
    let scanner = checkmate_scan_profile();

    let mut cursor = 0usize;
    while cursor < source.len() {
        // Transparent at the Checkmate level: comments and string literals
        // (so the word `magic` inside either is never detected).
        if let Some(len) = comment_len(source, cursor, &scanner) {
            cursor += len;
            continue;
        }
        if let Some(len) = string_len(source, cursor, &scanner) {
            cursor += len;
            continue;
        }
        if let Some(word) = keyword_at(source, cursor, "grammar") {
            cursor = scan_grammar(source, cursor, word, &scanner, &mut scan, &mut errors)
                .unwrap_or_else(|| advance_after_block(source, cursor));
            continue;
        }
        if let Some(word) = keyword_at(source, cursor, "magic") {
            let next = skip_inline_ws(source, cursor + word);
            let end = if source[next..].starts_with('(') {
                scan_invocation(source, cursor, &scanner, &mut scan, &mut errors)
            } else if starts_ident(source, next) {
                scan_magic_decl(source, cursor, &scanner, &mut scan, &mut errors)
            } else {
                errors.push(Diagnostic::parse(
                    "expected `(` or a macro name after `magic`",
                    Span::new(cursor, cursor + word),
                ));
                None
            };
            cursor = end.unwrap_or_else(|| advance_after_block(source, cursor));
            continue;
        }
        cursor += source[cursor..].chars().next().unwrap().len_utf8();
    }

    (scan, errors)
}

/// The keyword's length when `source[cursor..]` starts with `keyword` as a
/// whole word (identifier boundaries on both sides).
fn keyword_at(source: &str, cursor: usize, keyword: &str) -> Option<usize> {
    let rest = &source[cursor..];
    if !rest.starts_with(keyword) {
        return None;
    }
    if cursor > 0 {
        let prev = source[..cursor].chars().next_back().unwrap();
        if prev.is_ascii_alphanumeric() || prev == '_' {
            return None;
        }
    }
    let after = rest[keyword.len()..].chars().next();
    match after {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => None,
        _ => Some(keyword.len()),
    }
}

fn starts_ident(source: &str, cursor: usize) -> bool {
    match source[cursor..].chars().next() {
        Some(c) => c.is_ascii_alphabetic() || c == '_',
        None => false,
    }
}

/// Skips horizontal whitespace (spaces and tabs) only.
fn skip_inline_ws(source: &str, mut cursor: usize) -> usize {
    while matches!(source[cursor..].chars().next(), Some(' ') | Some('\t')) {
        cursor += 1;
    }
    cursor
}

/// Skips whitespace and comments from `cursor` onward.
fn skip_ws_and_comments(source: &str, mut cursor: usize, profile: &LexProfile) -> usize {
    loop {
        while let Some(c) = source[cursor..].chars().next() {
            if c.is_whitespace() {
                cursor += c.len_utf8();
            } else {
                break;
            }
        }
        if let Some(len) = comment_len(source, cursor, profile) {
            cursor += len;
            continue;
        }
        break;
    }
    cursor
}

/// The recovery point after a scanned (or failed) block: the start of the
/// next line, so a malformed header cannot swallow the next declaration.
fn advance_after_block(source: &str, cursor: usize) -> usize {
    match source[cursor..].find('\n') {
        Some(offset) => cursor + offset + 1,
        None => source.len(),
    }
}

/// Parses a dotted identifier path (`json`, `agent.spawn`).
fn dotted_path(source: &str, mut cursor: usize) -> Option<(Vec<String>, usize)> {
    let mut segments = Vec::new();
    loop {
        if !starts_ident(source, cursor) {
            return None;
        }
        let start = cursor;
        while matches!(
            source[cursor..].chars().next(),
            Some(c) if c.is_ascii_alphanumeric() || c == '_'
        ) {
            cursor += source[cursor..].chars().next().unwrap().len_utf8();
        }
        segments.push(source[start..cursor].to_string());
        if source[cursor..].starts_with('.') && starts_ident(source, cursor + 1) {
            cursor += 1;
            continue;
        }
        return Some((segments, cursor));
    }
}

/// Scans a `grammar name { … }` declaration. Returns the offset just past
/// the declaration, or `None` after recording a diagnostic.
fn scan_grammar(
    source: &str,
    keyword_pos: usize,
    keyword_len: usize,
    scanner: &LexProfile,
    scan: &mut MagicScan,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let mut cursor = skip_inline_ws(source, keyword_pos + keyword_len);
    cursor = skip_ws_and_comments(source, cursor, scanner);
    let Some((name, after_name)) = dotted_path(source, cursor) else {
        errors.push(Diagnostic::parse(
            "expected a grammar name after `grammar`",
            Span::new(cursor, cursor),
        ));
        return None;
    };
    if name.len() != 1 {
        errors.push(Diagnostic::parse(
            "a grammar name must be a single identifier",
            Span::new(cursor, after_name),
        ));
        return None;
    }
    let body_open = skip_ws_and_comments(source, after_name, scanner);
    if !source[body_open..].starts_with('{') {
        errors.push(Diagnostic::parse(
            "expected `{` to open the grammar body",
            Span::new(body_open, body_open),
        ));
        return None;
    }
    match balance(source, body_open, '{', '}', scanner) {
        Ok(body_close) => {
            let body_span = Span::new(body_open + 1, body_close);
            let body = source[body_span.start..body_span.end].to_string();
            let profile = parse_profile(&body, body_span).unwrap_or_else(|error| {
                errors.push(error);
                LexProfile::default()
            });
            scan.grammars.push(GrammarScan {
                name: name[0].clone(),
                span: Span::new(keyword_pos, body_close + 1),
                body_span,
                body,
                profile,
            });
            Some(body_close + 1)
        }
        Err(error) => {
            errors.push(Diagnostic::parse(error.message, error.span));
            None
        }
    }
}

/// Scans a `magic name(pattern) { template }` declaration. Returns the
/// offset just past the declaration, or `None` after recording a diagnostic.
fn scan_magic_decl(
    source: &str,
    keyword_pos: usize,
    scanner: &LexProfile,
    scan: &mut MagicScan,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let mut cursor = skip_inline_ws(source, keyword_pos + "magic".len());
    cursor = skip_ws_and_comments(source, cursor, scanner);
    let Some((name, after_name)) = dotted_path(source, cursor) else {
        errors.push(Diagnostic::parse(
            "expected a macro name after `magic`",
            Span::new(cursor, cursor),
        ));
        return None;
    };
    let name_span = Span::new(cursor, after_name);
    let pattern_open = skip_ws_and_comments(source, after_name, scanner);
    if !source[pattern_open..].starts_with('(') {
        errors.push(Diagnostic::parse(
            "expected `(` to open the magic pattern",
            Span::new(pattern_open, pattern_open),
        ));
        return None;
    }
    let pattern_close = match balance(source, pattern_open, '(', ')', scanner) {
        Ok(close) => close,
        Err(error) => {
            errors.push(Diagnostic::parse(error.message, error.span));
            return None;
        }
    };
    let pattern_span = Span::new(pattern_open + 1, pattern_close);
    let template_open = skip_ws_and_comments(source, pattern_close + 1, scanner);
    if !source[template_open..].starts_with('{') {
        errors.push(Diagnostic::parse(
            "expected `{` to open the magic template",
            Span::new(template_open, template_open),
        ));
        return None;
    }
    let template_close = match balance(source, template_open, '{', '}', scanner) {
        Ok(close) => close,
        Err(error) => {
            errors.push(Diagnostic::parse(error.message, error.span));
            return None;
        }
    };
    let template_span = Span::new(template_open + 1, template_close);
    scan.magics.push(MagicDeclScan {
        name: name.join("."),
        span: Span::new(keyword_pos, template_close + 1),
        pattern_span,
        pattern: source[pattern_span.start..pattern_span.end].to_string(),
        template_span,
        template: source[template_span.start..template_span.end].to_string(),
    });
    let _ = name_span;
    Some(template_close + 1)
}

/// Scans a `magic(name) { region }` invocation. The region is balanced under
/// the composed profile of the referenced macro's entry grammar (§8.6); an
/// unknown macro name falls back to the default profile (name resolution is
/// the expansion pass's job, not the scanner's). Returns the offset just
/// past the invocation, or `None` after recording a diagnostic.
fn scan_invocation(
    source: &str,
    keyword_pos: usize,
    scanner: &LexProfile,
    scan: &mut MagicScan,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let header_start = keyword_pos;
    let mut cursor = skip_inline_ws(source, keyword_pos + "magic".len());
    cursor += 1; // `(`
    cursor = skip_ws_and_comments(source, cursor, scanner);
    let Some((path, after_name)) = dotted_path(source, cursor) else {
        errors.push(Diagnostic::parse(
            "expected a macro name inside `magic(…)`",
            Span::new(cursor, cursor),
        ));
        return None;
    };
    let name_span = Span::new(cursor, after_name);
    cursor = skip_ws_and_comments(source, after_name, scanner);
    if !source[cursor..].starts_with(')') {
        errors.push(Diagnostic::parse(
            "expected `)` to close `magic(…)`",
            Span::new(cursor, cursor),
        ));
        return None;
    }
    cursor += 1;
    cursor = skip_ws_and_comments(source, cursor, scanner);
    let header_span = Span::new(header_start, cursor);
    if !source[cursor..].starts_with('{') {
        errors.push(Diagnostic::parse(
            "expected `{` to open the magic region",
            Span::new(cursor, cursor),
        ));
        return None;
    }
    let profile = composed_region_profile(scan, &path.join("."));
    let region_close = match balance(source, cursor, '{', '}', &profile) {
        Ok(close) => close,
        Err(error) => {
            errors.push(Diagnostic::parse(error.message, error.span));
            return None;
        }
    };
    let (region_span, region) = normalize_region(source, cursor, region_close);
    scan.invocations.push(InvocationScan {
        name: path.join("."),
        span: Span::new(header_start, region_close + 1),
        header_span,
        region_span,
        region,
    });
    let _ = name_span;
    Some(region_close + 1)
}

/// The composed profile for one invocation (§8.6): the entry grammar's
/// profile plus the default string form when the grammar declares none. The
/// entry grammar is recovered by peeking at the macro's declared pattern
/// (its first rule reference, e.g. `json.value`); unknown macros get the
/// default profile. When patterns are parsed (Task 3), this widens to the
/// full transitive reference closure of the entry pattern.
fn composed_region_profile(scan: &MagicScan, macro_name: &str) -> LexProfile {
    let entry_grammar = scan
        .magics
        .iter()
        .find(|magic| magic.name == macro_name)
        .and_then(|magic| entry_grammar_name(&magic.pattern))
        .and_then(|grammar| scan.grammar_profile(&grammar));
    match entry_grammar {
        Some(profile) => with_default_strings(profile),
        None => crate::mega::profile::default_profile(),
    }
}

/// The grammar name of a declared pattern's entry rule reference: the first
/// identifier segment of the pattern text (`json.value as v` → `json`).
/// Patterns are parsed structurally in Task 3; this lexical peek is enough
/// for profile composition because entry patterns begin with a rule ref.
fn entry_grammar_name(pattern: &str) -> Option<String> {
    let mut cursor = 0usize;
    while matches!(
        pattern[cursor..].chars().next(),
        Some(c) if c.is_whitespace()
    ) {
        cursor += 1;
    }
    if !starts_ident(pattern, cursor) {
        return None;
    }
    let (path, _) = dotted_path(pattern, cursor)?;
    Some(path[0].clone())
}

/// Region normalization (§8.6): excludes one line terminator immediately
/// after `{`, one immediately before `}`, and horizontal whitespace at the
/// region's start and end. As a robustness extension for line-oriented
/// grammars (whose skippers cannot cross line boundaries), any further blank
/// edge lines are trimmed too; interior whitespace and indentation stay
/// verbatim because `indent` blocks depend on them.
fn normalize_region(source: &str, open: usize, close: usize) -> (Span, String) {
    let mut start = open + 1;
    let mut end = close;
    loop {
        while matches!(
            source[start..end.max(start)].chars().next(),
            Some(' ') | Some('\t')
        ) {
            start += 1;
        }
        if source[start..].starts_with("\r\n") {
            start += 2;
        } else if source[start..].starts_with(['\n', '\r']) {
            start += 1;
        } else {
            break;
        }
    }
    loop {
        while end > start && matches!(source[..end].chars().next_back(), Some(' ') | Some('\t')) {
            end -= 1;
        }
        if end >= start + 2 && source[..end].ends_with("\r\n") {
            end -= 2;
        } else if end > start && source[..end].ends_with(['\n', '\r']) {
            end -= 1;
        } else {
            break;
        }
    }
    (
        Span::new(start, end.max(start)),
        source[start..end.max(start)].to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_ok(source: &str) -> MagicScan {
        let (scan, errors) = scan_magic(source);
        assert!(
            errors.is_empty(),
            "{source:?} should scan cleanly: {errors:?}"
        );
        scan
    }

    #[test]
    fn finds_invocation_and_normalizes_region() {
        let source = "int x = 1\nstr s = magic(json.value) {\n    \"host\"\n}\n";
        let scan = scan_ok(source);
        assert_eq!(scan.invocations.len(), 1);
        let invocation = &scan.invocations[0];
        assert_eq!(invocation.name, "json.value");
        assert_eq!(invocation.region, "\"host\"");
        // The region span points at the original text, not a copy.
        assert_eq!(
            &source[invocation.region_span.start..invocation.region_span.end],
            "\"host\""
        );
        // The invocation span covers `magic … }`.
        assert!(
            source[invocation.span.start..invocation.span.end].starts_with("magic(json.value)")
        );
    }

    #[test]
    fn braces_inside_region_strings_do_not_close_the_region() {
        let source = "magic(js.run) {\n    console.log(\"}\")\n}\nint x = 1\n";
        let scan = scan_ok(source);
        assert_eq!(scan.invocations.len(), 1);
        assert_eq!(scan.invocations[0].region, "console.log(\"}\")");
        // Everything after the region stays outside it.
        assert_eq!(&source[scan.invocations[0].span.end..], "\nint x = 1\n");
    }

    #[test]
    fn html_region_balances_strings_comments_and_prose_apostrophes() {
        let source = "magic(html.fragment) {\n\
            <!-- it's fine -->\n\
            <div class=\"hud\">x</div>\n\
            <p>don't</p>\n\
        }\n";
        let scan = scan_ok(source);
        assert_eq!(scan.invocations.len(), 1);
        assert_eq!(
            scan.invocations[0].region,
            "<!-- it's fine -->\n<div class=\"hud\">x</div>\n<p>don't</p>"
        );
    }

    #[test]
    fn region_normalization_trims_edges() {
        let source = "magic(m) {  \n\n   body text   \n\n  }\n";
        let scan = scan_ok(source);
        assert_eq!(scan.invocations[0].region, "body text");
    }

    #[test]
    fn scans_grammar_and_extracts_profile() {
        let source = "\
grammar yaml {
    skip    [ ' ' ]
    comment ( \"#\" )
    string  ( '\"' )
    string  ( '\\'' )

    rule document { oneof { blockDoc => blockNode } }
}

magic value(yaml.document as doc) {
    @toValue($doc)
}
";
        let scan = scan_ok(source);
        assert_eq!(scan.grammars.len(), 1);
        let grammar = &scan.grammars[0];
        assert_eq!(grammar.name, "yaml");
        assert!(!grammar.profile.is_flow_oriented());
        assert_eq!(grammar.profile.comments.len(), 1);
        assert_eq!(grammar.profile.comments[0].opener, "#");
        assert_eq!(grammar.profile.strings.len(), 2);
        assert_eq!(grammar.profile.strings[0].quote, '"');

        assert_eq!(scan.magics.len(), 1);
        let magic = &scan.magics[0];
        assert_eq!(magic.name, "value");
        assert_eq!(magic.pattern, "yaml.document as doc");
        assert_eq!(magic.template, "\n    @toValue($doc)\n");
    }

    #[test]
    fn parses_island_forms_in_string_declarations() {
        let source = "\
grammar js {
    string  ( '`' multiline island ( \"${\" \"}\" ) )
    rule program { each { statement } as stmts }
}
";
        let scan = scan_ok(source);
        let strings = &scan.grammars[0].profile.strings;
        assert_eq!(strings.len(), 1);
        assert!(strings[0].multiline);
        assert_eq!(strings[0].island, Some(("${".to_string(), "}".to_string())));
    }

    #[test]
    fn magic_word_inside_strings_and_comments_is_inert() {
        let source = "\
// magic(fake) { not real }
str s = \"magic(fake) { also not }\"
int x = 1
";
        let scan = scan_ok(source);
        assert!(scan.is_empty());
    }

    #[test]
    fn nested_magic_inside_a_region_stays_wholesale() {
        let source = "magic(js.run) {\n    magic(json.value) { 1 }\n}\n";
        let scan = scan_ok(source);
        assert_eq!(scan.invocations.len(), 1);
        assert_eq!(scan.invocations[0].region, "magic(json.value) { 1 }");
    }

    #[test]
    fn unknown_macro_region_uses_default_profile_and_still_scans() {
        let source = "magic(ghost) {\n    { \"brace\": 1 }\n}\n";
        let scan = scan_ok(source);
        assert_eq!(scan.invocations.len(), 1);
        assert_eq!(scan.invocations[0].name, "ghost");
        assert_eq!(scan.invocations[0].region, "{ \"brace\": 1 }");
    }

    #[test]
    fn composed_profile_uses_the_entry_grammar() {
        // `grammar toml` has a `#` comment form; a comment inside the region
        // must not misbalance the braces even though the text contains `{`.
        let source = "\
grammar toml {
    skip    [ ' ', '\\t' ]
    comment ( \"#\" )
    rule document { each { oneof { kv => keyval } } as items }
    rule keyval { $word key \"=\" value eol }
}

magic value(toml.document as doc) {
    @toValue($doc)
}

str[] lines = magic(value) {
    [table]  # { not a brace }
    x = 1
}
";
        let scan = scan_ok(source);
        assert_eq!(scan.invocations.len(), 1);
        // Interior indentation is preserved verbatim; only the edges trim.
        assert_eq!(
            scan.invocations[0].region,
            "[table]  # { not a brace }\n    x = 1"
        );
    }

    #[test]
    fn pattern_parens_containing_braces_and_strings_scan() {
        // The pattern text contains a brace literal and a paren literal in
        // strings; the declaration balancer must be string-aware.
        let source = "\
magic bound(re.pattern as p) {
    reQuantified($p)
}

magic reQuantified({\"{\" $int min \"}\"} as q) {
    $q
}
";
        let scan = scan_ok(source);
        assert_eq!(scan.magics.len(), 2);
        assert_eq!(scan.magics[1].pattern, "{\"{\" $int min \"}\"} as q");
    }

    #[test]
    fn malformed_constructs_report_and_recover() {
        let (scan, errors) = scan_magic("grammar {\nint x = 1\n");
        assert!(scan.grammars.is_empty());
        assert!(
            errors
                .iter()
                .any(|error| error.message().contains("grammar name"))
        );

        let (scan, errors) = scan_magic("magic {\nint x = 1\n");
        assert!(scan.magics.is_empty() && scan.invocations.is_empty());
        assert!(
            errors
                .iter()
                .any(|error| error.message().contains("macro name"))
        );

        let (_, errors) = scan_magic("magic(m) {\nnever closed\n");
        assert!(
            errors
                .iter()
                .any(|error| error.message().contains("unclosed"))
        );
    }

    #[test]
    fn empty_source_scans_to_nothing() {
        let scan = scan_ok("");
        assert!(scan.is_empty());
    }
}
