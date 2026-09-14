//! Megaprogramming completion (§8): the pattern language, expansion
//! templates, grammar profiles, and the `mega`/`grammar` declarations
//! themselves.
//!
//! Position features analyze the ORIGINAL text for mega files (see
//! [`crate::db::parse_original`]), so the script analysis covers ordinary
//! code everywhere in the file. Completion keeps a dedicated mega path for
//! the contexts the pattern language owns: [`cme_compiler::mega::scan_mega`]
//! locates every declaration and region in the user's own coordinates, and
//! the pattern/template grammars are lexical contexts, not resolved names. A
//! suggestion can never contradict the compiler here, because the expander
//! re-checks everything the template generates anyway.
//!
//! Contexts (see [`MegaContext`]):
//!
//! - **top level** — the declarations plus the ordinary keywords;
//! - **`mega name( … )` pattern** — the §8.3 pattern language (fragments,
//!   combinators, annotations);
//! - **`mega name( … ) { … }` template** — the §8.4 template constructs;
//! - **`grammar name { … }`** — the §8.2 lexical profile declarations;
//! - **`rule … { … }` inside a grammar** — the pattern language again;
//! - **invocation regions (`name! { … }`)** — suppressed: the region is a
//!   foreign language, and suggestions there would be noise.

use tower_lsp_server::ls_types;

use cme_compiler::mega::scan::scan_mega;

/// The §8.3.3 fragments: typed matchers that bind a capture directly.
const FRAGMENTS: [(&str, &str); 13] = [
    ("$ident", "a Checkmate-valid identifier (§8.3.3)"),
    (
        "$word",
        "foreign identifier [A-Za-z_$][0-9A-Za-z_$]* (§8.3.3)",
    ),
    (
        "$tag",
        "relaxed foreign token: letters, digits, -, ., _ (§8.3.3)",
    ),
    ("$int", "numeric literal, int capture (§8.3.3)"),
    ("$float", "numeric literal, float capture (§8.3.3)"),
    ("$str", "double-quoted string with escapes (§8.3.3)"),
    ("$text", "effective tail, or the region remainder (§8.3.3)"),
    ("$raw", "effective tail parsed as code / by a rule (§8.3.6)"),
    ("$expr", "a live Checkmate expression island (§8.3.3)"),
    ("$type", "a live Checkmate type island (§8.3.3)"),
    ("$block", "a live Checkmate block island (§8.3.3)"),
    ("$template", "tail split at island delimiters (§8.3.3)"),
    (
        "$tt",
        "one token or balanced tree, parameterizable $tt<\"{\" \"}\"> (§8.3.3)",
    ),
];

/// The §8.3.2 combinators and §8.3.10 pattern keywords.
const PATTERN_KEYWORDS: [(&str, &str); 24] = [
    (
        "each",
        "repetition: each { p } / each sep \",\" { p } (§8.3.2)",
    ),
    ("sep", "repetition separator (§8.3.2)"),
    ("trailing", "allow a trailing separator (§8.3.2)"),
    ("optional", "p or nothing, atomically (§8.3.2)"),
    ("oneof", "ordered choice, first match wins (§8.3.2)"),
    (
        "where",
        "validate captures; failing fails the sequence (§8.3.4)",
    ),
    ("peek", "zero-width lookahead (§8.3.2)"),
    ("not", "zero-width negative lookahead (§8.3.2)"),
    ("label", "diagnostic context for failures inside (§8.3.9)"),
    (
        "until",
        "verbatim run stopping before a whole-match tail (§8.3.2)",
    ),
    ("scan", "maximal run of ≥ 1 characters from a set (§8.3.2)"),
    ("raw", "suspend the skipper within (§8.3.2)"),
    (
        "soft",
        "newlines join the skip set within, line mode (§8.3.2)",
    ),
    ("indent", "indentation-delimited block (§8.3.5)"),
    (
        "verbatim",
        "indent verbatim: every line ≥ B is content (§8.3.5)",
    ),
    (
        "eol",
        "consume the line end and its transparent tail (§8.3.2)",
    ),
    ("line", "zero-width: content remains on this line (§8.3.2)"),
    ("eof", "only skip-set characters remain (§8.3.2)"),
    ("recur", "the innermost enclosing rule (§8.3.2)"),
    ("as", "bind the match to a capture name (§8.3.2)"),
    ("with", "pass context into a rule reference (§8.3.7)"),
    ("context", "ancestor data, explicit and downward (§8.3.7)"),
    ("some", "`some x in xs { … }` over captures (§A.5, §8.3.4)"),
    ("all", "`all x in xs { … }` over captures (§A.5, §8.3.4)"),
];

/// The §8.1/§8.3.10 inert editor annotations.
const ANNOTATIONS: [(&str, &str); 3] = [
    ("#complete", "editor completion source, inert (§8.1)"),
    ("#hover", "editor hover text, inert (§8.1)"),
    ("#token", "editor token kind, inert (§8.1)"),
];

/// The §8.4 template constructs.
const TEMPLATE_CONSTRUCTS: [(&str, &str); 7] = [
    (
        "[each in ",
        "repeat for every element of a list capture (§8.4)",
    ),
    (
        "[when ",
        "conditional splicing with an optional else (§8.4)",
    ),
    ("match ($", "dispatch on a tagged `oneof` capture (§8.4)"),
    ("present(", "test an optional capture in when/where (§8.4)"),
    (
        "require(",
        "emit a compile-time error anchored at a capture (§8.4)",
    ),
    ("let", "bind a template-local value (§8.4)"),
    ("@", "invoke a pure compile-time function (§8.5)"),
];

/// The §8.2 lexical profile declarations of a grammar.
const GRAMMAR_KEYWORDS: [(&str, &str); 6] = [
    ("skip", "the skip set: `[ ' ', '\\t' ]` (§8.2)"),
    (
        "comment",
        "a comment form: `( \"//\" )` or `(\"/*\" until \"*/\")` (§8.2)",
    ),
    ("string", "a string form the scanner honors (§8.2)"),
    ("island", "an interpolation island inside strings (§8.2)"),
    ("rule", "a matching rule: `rule name { … }` (§8.2)"),
    (
        "extends",
        "inherit rules and profile: `grammar ts extends js` (§8.2)",
    ),
];

/// Computes completions at `offset` for a file that mentions megaprograms.
pub fn completions(text: &str, offset: usize) -> Vec<ls_types::CompletionItem> {
    match context_at(text, offset) {
        MegaContext::Pattern => pattern_items(),
        MegaContext::Template => template_items(),
        MegaContext::GrammarProfile => GRAMMAR_KEYWORDS
            .iter()
            .map(|(name, detail)| keyword(name, detail))
            .collect(),
        MegaContext::RuleBody | MegaContext::RuleParens => pattern_items(),
        // Invocation regions are foreign text: nothing to suggest.
        MegaContext::InvocationRegion => Vec::new(),
        // Ordinary code: the megaprogramming declarations lead, ordinary
        // keywords follow.
        MegaContext::None => {
            let mut items = vec![
                keyword(
                    "mega",
                    "megaprogram entry: `mega name(pattern) { template }` (§8.1)",
                ),
                keyword("grammar", "named library of matching rules (§8.2)"),
            ];
            for (name, detail) in crate::features::completion::SCRIPT_DECL_KEYWORDS {
                items.push(keyword(name, detail));
            }
            items
        }
    }
}

/// Where `offset` sits relative to the file's megaprogram constructs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MegaContext {
    /// Ordinary Checkmate code: no mega construct covers the offset.
    None,
    /// Inside a `mega name( … )` pattern.
    Pattern,
    /// Inside a `mega name( … ) { … }` template.
    Template,
    /// Between a grammar's profile declarations.
    GrammarProfile,
    /// Inside a grammar rule's pattern body.
    RuleBody,
    /// Inside a grammar rule's `( … )` header.
    RuleParens,
    /// Inside a `name! { … }` / heredoc invocation region.
    InvocationRegion,
}

/// Classifies `offset` against the §8 scan. `None` means the offset sits in
/// ordinary Checkmate code, where the script analysis (not the pattern
/// vocabulary) drives completion.
pub fn context_at(text: &str, offset: usize) -> MegaContext {
    let (scan, _) = scan_mega(text);

    // Mega declarations: pattern or template, by span.
    for mega in &scan.megas {
        if offset >= mega.pattern_span.start && offset <= mega.pattern_span.end {
            return MegaContext::Pattern;
        }
        if offset >= mega.template_span.start && offset <= mega.template_span.end {
            return MegaContext::Template;
        }
    }

    // Grammar bodies: profile declarations at brace depth 0, the pattern
    // language inside `rule` bodies.
    for grammar in &scan.grammars {
        if offset >= grammar.body_span.start && offset <= grammar.body_span.end {
            return match rule_nesting(&grammar.body, offset - grammar.body_span.start) {
                RuleNesting::Profile => MegaContext::GrammarProfile,
                RuleNesting::Pattern => MegaContext::RuleBody,
                RuleNesting::RuleParens => MegaContext::RuleParens,
            };
        }
    }

    // Invocation regions are foreign text.
    for invocation in &scan.invocations {
        if offset >= invocation.region_span.start && offset <= invocation.region_span.end {
            return MegaContext::InvocationRegion;
        }
    }

    MegaContext::None
}

/// Where the cursor sits inside a grammar body.
enum RuleNesting {
    /// Between the grammar's own declarations.
    Profile,
    /// Inside a rule's pattern body.
    Pattern,
    /// Inside a rule's `( … )` header (parameters, context).
    RuleParens,
}

/// Classifies an offset inside a grammar body: counts braces and parens
/// from the body start. Depth 0 → profile declarations; inside a `rule`'s
/// braces → the pattern language; inside its parens → the header language
/// (context declarations take the pattern vocabulary too).
fn rule_nesting(body: &str, relative: usize) -> RuleNesting {
    let mut brace_depth = 0i32;
    let mut paren_depth = 0i32;
    for byte in body.as_bytes()[..relative.min(body.len())].iter() {
        match byte {
            b'{' => brace_depth += 1,
            b'}' => brace_depth -= 1,
            b'(' => paren_depth += 1,
            b')' => paren_depth -= 1,
            _ => {}
        }
    }
    if paren_depth > 0 {
        RuleNesting::RuleParens
    } else if brace_depth > 0 {
        RuleNesting::Pattern
    } else {
        RuleNesting::Profile
    }
}

/// The §8.3 pattern language: fragments, combinators, annotations.
fn pattern_items() -> Vec<ls_types::CompletionItem> {
    let mut items = Vec::new();
    for (name, detail) in FRAGMENTS {
        items.push(item(
            name.to_string(),
            ls_types::CompletionItemKind::FUNCTION,
            detail.to_string(),
            None,
        ));
    }
    for (name, detail) in PATTERN_KEYWORDS {
        items.push(keyword(name, detail));
    }
    for (name, detail) in ANNOTATIONS {
        items.push(item(
            name.to_string(),
            ls_types::CompletionItemKind::PROPERTY,
            detail.to_string(),
            Some(format!("{name}(")),
        ));
    }
    items
}

/// The §8.4 template constructs plus the compile-time call prefix.
fn template_items() -> Vec<ls_types::CompletionItem> {
    let mut items = Vec::new();
    for (name, detail) in TEMPLATE_CONSTRUCTS {
        items.push(item(
            name.to_string(),
            ls_types::CompletionItemKind::KEYWORD,
            detail.to_string(),
            None,
        ));
    }
    items
}

fn keyword(name: &str, detail: &str) -> ls_types::CompletionItem {
    item(
        name.to_string(),
        ls_types::CompletionItemKind::KEYWORD,
        detail.to_string(),
        None,
    )
}

fn item(
    label: String,
    kind: ls_types::CompletionItemKind,
    detail: String,
    insert: Option<String>,
) -> ls_types::CompletionItem {
    ls_types::CompletionItem {
        label,
        kind: Some(kind),
        detail: Some(detail),
        insert_text: insert,
        ..ls_types::CompletionItem::default()
    }
}
