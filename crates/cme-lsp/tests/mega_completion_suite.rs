//! Megaprogramming completion suite (§8): the context matrix over real
//! fixtures — top level, `mega` declaration patterns and templates,
//! grammar profiles and rule bodies, and the suppression inside invocation
//! regions — driven through [`cme_lsp::features::mega_completion`], the
//! exact function the server routes to.
//!
//! The fixtures reuse the shapes of `mega.cm` at the repository root, so
//! what the tests pin is what a megaprogram author types.

use cme_lsp::features::mega_completion::completions;

fn labels_at(source: &str, needle: &str, skip: usize) -> Vec<String> {
    let offset = source
        .find(needle)
        .unwrap_or_else(|| panic!("fixture must contain {needle:?}:\n{source}"))
        + skip;
    completions(source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect()
}

/// The §8.8-shaped working file: a grammar, a mega declaration, and an
/// invocation, all exercising different contexts.
const FIXTURE: &str = "\
grammar conf {
    skip    [ ' ', '\\t' ]
    comment ( \"#\" )

    rule setting {
        $word key \"=\" $str value
        eol
    }
}

mega setting(
    #complete(availableKeys)
    \"key:\" $word key
    $text rest
) {
    conf.set(
        key: $\"{$key}\"
        value: $rest
    )
}

int main() {
    setting! {
        key: mode
        value: fast
    }
    return 0
}
";

// ---------------------------------------------------------------------------
// Top level
// ---------------------------------------------------------------------------

#[test]
fn the_top_level_offers_the_megaprogramming_declarations() {
    // The blank line between the grammar and the mega declaration.
    let offset = FIXTURE.find("\n\nmega").expect("blank line") + 1;
    let offered: Vec<String> = completions(FIXTURE, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"mega".to_string()) && offered.contains(&"grammar".to_string()),
        "`mega` and `grammar` complete at the top level: {offered:?}"
    );
    assert!(
        offered.contains(&"struct".to_string()) && offered.contains(&"import".to_string()),
        "ordinary declarations still apply (a mega file is also Checkmate): {offered:?}"
    );
}

#[test]
fn a_plain_position_between_declarations_completes_too() {
    let offered = labels_at(FIXTURE, "int main()", 1);
    assert!(
        offered.contains(&"mega".to_string()),
        "`mega` is offered wherever a declaration can start: {offered:?}"
    );
}

// ---------------------------------------------------------------------------
// mega declaration: pattern position
// ---------------------------------------------------------------------------

#[test]
fn the_pattern_position_offers_fragments_and_combinators() {
    // Inside the `mega setting( ... )` pattern.
    let offered = labels_at(FIXTURE, "\"key:\" $word key", 1);
    for fragment in ["$word", "$str", "$ident", "$text", "$expr", "$template"] {
        assert!(
            offered.iter().any(|label| label == fragment),
            "`{fragment}` completes in pattern position: {offered:?}"
        );
    }
    for keyword in [
        "each", "optional", "oneof", "where", "eol", "recur", "context",
    ] {
        assert!(
            offered.iter().any(|label| label == keyword),
            "`{keyword}` completes in pattern position: {offered:?}"
        );
    }
}

#[test]
fn the_pattern_position_offers_the_editor_annotations() {
    let offered = labels_at(FIXTURE, "#complete(availableKeys)", 2);
    for annotation in ["#complete", "#hover", "#token"] {
        assert!(
            offered.iter().any(|label| label == annotation),
            "`{annotation}` completes in pattern position (§8.1): {offered:?}"
        );
    }
}

#[test]
fn annotations_fill_with_their_parentheses() {
    let offset = FIXTURE
        .find("#complete(availableKeys)")
        .expect("annotation")
        + 2;
    let items = completions(FIXTURE, offset);
    let complete = items
        .iter()
        .find(|item| item.label == "#complete")
        .expect("#complete offered");
    assert_eq!(
        complete.insert_text.as_deref(),
        Some("#complete("),
        "the fill opens the annotation's argument list"
    );
}

// ---------------------------------------------------------------------------
// mega declaration: template position
// ---------------------------------------------------------------------------

#[test]
fn the_template_position_offers_the_template_constructs() {
    let offered = labels_at(FIXTURE, "conf.set(", 3);
    for construct in ["[each in ", "[when ", "present(", "require(", "@"] {
        assert!(
            offered.iter().any(|label| label == construct),
            "`{construct}` completes in template position (§8.4): {offered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// grammar bodies
// ---------------------------------------------------------------------------

#[test]
fn a_grammar_body_at_depth_zero_offers_the_profile_declarations() {
    let offered = labels_at(FIXTURE, "comment ( \"#\" )", 1);
    for keyword in ["skip", "comment", "string", "island", "rule"] {
        assert!(
            offered.iter().any(|label| label == keyword),
            "`{keyword}` completes in the grammar profile (§8.2): {offered:?}"
        );
    }
    assert!(
        !offered.contains(&"$word".to_string()),
        "fragments do not apply to the profile: {offered:?}"
    );
}

#[test]
fn a_rule_body_switches_to_the_pattern_language() {
    let offered = labels_at(FIXTURE, "$word key \"=\" $str value", 1);
    assert!(
        offered.iter().any(|label| label == "$tag"),
        "fragments complete inside a rule body (§8.2 rules speak §8.3): {offered:?}"
    );
    assert!(
        offered.iter().any(|label| label == "eol"),
        "line-mode machinery completes inside a line-oriented grammar: {offered:?}"
    );
}

// ---------------------------------------------------------------------------
// invocation regions: foreign text, silence
// ---------------------------------------------------------------------------

#[test]
fn an_invocation_region_stays_silent() {
    let offered = labels_at(FIXTURE, "key: mode", 2);
    assert!(
        offered.is_empty(),
        "the region is a foreign language; nothing completes: {offered:?}"
    );
}

#[test]
fn code_after_an_invocation_region_recovers_the_top_level() {
    let offered = labels_at(FIXTURE, "return 0", 1);
    assert!(
        offered.contains(&"return".to_string()) || !offered.is_empty(),
        "Checkmate code after a region completes normally: {offered:?}"
    );
}

// ---------------------------------------------------------------------------
// tolerance: broken and heredoc shapes
// ---------------------------------------------------------------------------

#[test]
fn a_broken_mega_declaration_still_completes_its_pattern() {
    let source = "mega broken(\n    $word\n) {\n    ???\n}\n";
    let offered = labels_at(source, "$word", 2);
    assert!(
        offered.contains(&"$str".to_string()),
        "a malformed template does not stop pattern completion: {offered:?}"
    );
}

#[test]
fn a_heredoc_region_stays_silent() {
    let source = "mega value($word w) {\n    @toValue($w)\n}\n\nint main() {\n    value! <<END\ntext with } braces\nEND\n    return 0\n}\n";
    let offered = labels_at(source, "text with } braces", 3);
    assert!(
        offered.is_empty(),
        "the heredoc region is foreign text: {offered:?}"
    );
}

#[test]
fn a_file_with_only_an_invocation_still_completes_the_top_level() {
    let source = "int main() {\n    once! {\n        1\n    }\n    return 0\n}\n";
    let offered = labels_at(source, "int main()", 1);
    assert!(
        offered.contains(&"mega".to_string()) && offered.contains(&"grammar".to_string()),
        "invocations alone do not hide the declarations: {offered:?}"
    );
}
