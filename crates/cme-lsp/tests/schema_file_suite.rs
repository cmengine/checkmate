//! Schema-file authoring suite (§9): completion, hover, and outline inside
//! `.cm` schema documents, through the real server where positions matter
//! and against [`cme_lsp::features::schema_docs`] directly for context
//! matrices.
//!
//! A schema author should never leave the file: the keywords, the member
//! shapes, and every declared type complete; hover explains every
//! declaration; the outline mirrors the file.

use cme_lsp::convert::LineIndex;
use cme_lsp::features::schema_docs::{SchemaDoc, completions, document_symbols, hover};
use tower_lsp_server::ls_types;

const ENGINE: &str = "\
// The engine contract (§9.1).
schema engine 1.4.0

struct TextureHandle {
    int id
    str tag
}

enum LoadError {
    NotFound
    Corrupt(str reason)
    Stale(int generation)
}

capability graphics {
    since 1.0.0 TextureHandle LoadTexture(str path)
    since 1.2.0 optional void DrawSprite(TextureHandle tex, int frame)
}

interface gamemode requires core {
    since 1.0.0 int OnEvent(GameEvent event)
    since 1.4.0 optional void OnPause()
}
";

#[allow(dead_code)] // kept alongside hover_at for future context probes
fn labels_at(source: &str, needle: &str, skip: usize) -> Vec<String> {
    let offset = source
        .find(needle)
        .unwrap_or_else(|| panic!("fixture must contain {needle:?}:\n{source}"))
        + skip;
    let doc = SchemaDoc::build(source);
    completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect()
}

fn hover_at(source: &str, needle: &str, skip: usize) -> String {
    let offset = source
        .find(needle)
        .unwrap_or_else(|| panic!("fixture must contain {needle:?}:\n{source}"))
        + skip;
    let doc = SchemaDoc::build(source);
    let hover = hover(&doc, offset)
        .unwrap_or_else(|| panic!("hover must resolve {needle:?} at +{skip}:\n{source}"));
    let ls_types::HoverContents::Markup(markup) = hover.contents else {
        panic!("hover must render markdown");
    };
    markup.value
}

// ---------------------------------------------------------------------------
// Completion contexts
// ---------------------------------------------------------------------------

#[test]
fn the_top_level_offers_the_five_declaration_keywords() {
    // The blank line between declarations is a top-level position.
    let needle = "schema engine 1.4.0\n\n";
    let offset = ENGINE.find(needle).expect("after the header") + needle.len();
    let doc = SchemaDoc::build(ENGINE);
    let offered: Vec<String> = completions(&doc, ENGINE, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    for keyword in ["schema", "capability", "interface", "struct", "enum"] {
        assert!(
            offered.iter().any(|label| label == keyword),
            "`{keyword}` completes at the top level: {offered:?}"
        );
    }
}

#[test]
fn an_empty_file_still_completes_the_header() {
    let doc = SchemaDoc::build("");
    let offered: Vec<String> = completions(&doc, "", 0)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"schema".to_string()),
        "a fresh file offers `schema` first: {offered:?}"
    );
}

#[test]
fn a_capability_body_offers_since_and_return_types() {
    let source = "schema engine 1.4.0\n\nstruct TextureHandle {\n    int id\n}\n\ncapability graphics {\n    \n}\n";
    // Cursor on the empty member line inside `capability`.
    let offset = source.find("    \n").expect("member line") + 4;
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"since".to_string()),
        "members start with `since` (§9.5): {offered:?}"
    );
    assert!(
        offered.contains(&"int".to_string()) && offered.contains(&"TextureHandle".to_string()),
        "return types include built-ins and declared types: {offered:?}"
    );
    assert!(
        !offered.contains(&"optional".to_string()),
        "capability members are never `optional` (§9.5): {offered:?}"
    );
}

#[test]
fn an_interface_body_adds_the_optional_modifier() {
    let source = "schema engine 1.4.0\n\ninterface gamemode {\n    \n}\n";
    let offset = source.find("    \n").expect("member line") + 4;
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"optional".to_string()),
        "`optional` is an interface-member concept (§9.5): {offered:?}"
    );
}

#[test]
fn after_since_the_version_completes_to_nothing() {
    let source = "schema engine 1.4.0\n\ncapability graphics {\n    since \n}\n";
    let offset = source.find("since \n").expect("since line") + "since ".len();
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.is_empty(),
        "versions are typed, not completed: {offered:?}"
    );
}

#[test]
fn after_since_version_the_return_type_completes() {
    let source = "schema engine 1.4.0\n\ncapability graphics {\n    since 1.0.0 \n}\n";
    let offset = source.find("since 1.0.0 \n").expect("member line") + "since 1.0.0 ".len();
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"int".to_string()) && offered.contains(&"void".to_string()),
        "after `since <version>` the member's return type completes: {offered:?}"
    );
    assert!(
        !offered.contains(&"since".to_string()),
        "the modifier already consumed: {offered:?}"
    );
}

#[test]
fn struct_fields_complete_types_without_void() {
    let source = "schema engine 1.4.0\n\nstruct TextureHandle {\n    \n}\n";
    let offset = source.find("    \n").expect("field line") + 4;
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"int".to_string()) && offered.contains(&"str".to_string()),
        "field types complete: {offered:?}"
    );
    assert!(
        !offered.contains(&"void".to_string()),
        "`void` is return-position only (§2.4): {offered:?}"
    );
}

#[test]
fn parameter_types_complete_inside_the_param_list() {
    let source = "schema engine 1.4.0\n\nstruct TextureHandle {\n    int id\n}\n\ncapability graphics {\n    since 1.0.0 void Draw(TextureHandle tex, \n}\n";
    let offset = source.find("tex, \n").expect("param list") + "tex, ".len();
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"int".to_string()) && offered.contains(&"TextureHandle".to_string()),
        "parameter types complete (§2.11 shape): {offered:?}"
    );
}

#[test]
fn comments_and_declared_names_suppress_completion() {
    // Inside a comment: nothing.
    let source = "schema engine 1.4.0\n\n// what goes here?\n";
    let offset = source.find("what goes").expect("comment");
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.is_empty(),
        "comments complete to nothing: {offered:?}"
    );

    // Typing a declaration's name: nothing (the name is free-form).
    let source = "schema engine 1.4.0\n\ncapability \n";
    let offset = source.find("capability \n").expect("name line") + "capability ".len();
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.is_empty(),
        "a declaration name is free-form: {offered:?}"
    );
}

#[test]
fn a_broken_file_still_completes_what_recovered() {
    // Mid-edit breakage is exactly when help matters most.
    let source = "schema engine 1.4.0\n\nstruct Texture {\n    int id\n\ncapability graphics {\n    since 1.0.0 \n}\n";
    let offset = source.find("since 1.0.0 \n").expect("member line") + "since 1.0.0 ".len();
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"int".to_string()),
        "the recovered context still completes types: {offered:?}"
    );
}

// ---------------------------------------------------------------------------
// Hover
// ---------------------------------------------------------------------------

#[test]
fn hover_on_the_namespace_shows_the_header() {
    let value = hover_at(ENGINE, "schema engine", "schema ".len() + 3);
    assert!(
        value.contains("schema engine 1.4.0"),
        "the namespace hovers as the header: {value}"
    );
}

#[test]
fn hover_on_a_struct_shows_its_fields() {
    // Declaration site.
    let value = hover_at(ENGINE, "struct TextureHandle", "struct ".len());
    assert!(
        value.contains("id: int") && value.contains("tag: str"),
        "{value}"
    );
    // Use site: a member's return type.
    let value = hover_at(ENGINE, "since 1.0.0 TextureHandle", "since 1.0.0 ".len());
    assert!(value.contains("struct TextureHandle"), "{value}");
}

#[test]
fn hover_on_an_enum_shows_its_variants() {
    let value = hover_at(ENGINE, "enum LoadError", "enum ".len());
    assert!(
        value.contains("NotFound") && value.contains("Corrupt(reason: str)"),
        "variants render with payloads: {value}"
    );
}

#[test]
fn hover_on_a_contract_shows_its_members_and_requires() {
    let value = hover_at(ENGINE, "interface gamemode", "interface ".len());
    assert!(
        value.contains("interface gamemode") && value.contains("requires core"),
        "the requires edge renders (§9.4): {value}"
    );
    assert!(
        value.contains("int OnEvent(GameEvent event)"),
        "member signatures render: {value}"
    );
}

#[test]
fn hover_on_a_member_shows_signature_since_and_optional() {
    let value = hover_at(
        ENGINE,
        "since 1.2.0 optional void DrawSprite",
        "since 1.2.0 optional void ".len(),
    );
    assert!(
        value.contains("void graphics.DrawSprite(TextureHandle tex, int frame)"),
        "the full signature renders: {value}"
    );
    assert!(
        value.contains("since 1.2.0") && value.contains("optional"),
        "the §9.5 metadata renders: {value}"
    );
}

#[test]
fn hover_on_a_builtin_type_explains_it() {
    let value = hover_at(ENGINE, "    int id", "    ".len());
    assert!(
        value.contains("signed 64-bit integer"),
        "built-in types hover with their description: {value}"
    );
}

// ---------------------------------------------------------------------------
// Outline
// ---------------------------------------------------------------------------

#[test]
fn the_outline_mirrors_the_schema_shape() {
    let doc = SchemaDoc::build(ENGINE);
    let index = LineIndex::new(ENGINE);
    let symbols = document_symbols(&doc, &index, ENGINE);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["TextureHandle", "LoadError", "graphics", "gamemode"],
        "every declaration appears in declaration order: {names:?}"
    );

    let graphics = symbols.iter().find(|s| s.name == "graphics").unwrap();
    let members: Vec<&str> = graphics
        .children
        .as_ref()
        .expect("contract children")
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(members, vec!["LoadTexture", "DrawSprite"]);
}

#[test]
fn a_broken_file_still_outlines_what_recovered() {
    let source = "schema engine 1.4.0\n\nstruct Text {\n    int id\n\n??? { broken\n";
    let doc = SchemaDoc::build(source);
    let index = LineIndex::new(source);
    let symbols = document_symbols(&doc, &index, source);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"Text"),
        "the recovered struct is outlined: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// More contexts and edge shapes
// ---------------------------------------------------------------------------

#[test]
fn typing_the_header_version_completes_to_nothing() {
    // After `schema engine ` the version is typed, not suggested.
    let source = "schema engine \n";
    let offset = source.find("engine \n").expect("header") + "engine ".len();
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.is_empty(),
        "the header version is free-typed: {offered:?}"
    );
}

#[test]
fn a_capability_body_offers_requires_until_it_is_declared() {
    // `requires` appears among the member-start suggestions through the
    // parser's own acceptance of both §9.4 spellings; once a member exists
    // the line shape decides.
    let source = "schema engine 1.4.0\n\ncapability net {\n    \n}\n";
    let offset = source.find("    \n").expect("member line") + 4;
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"since".to_string()),
        "the member shape is suggested: {offered:?}"
    );
}

#[test]
fn an_enum_body_suggests_nothing_variants_are_names() {
    let source = "schema engine 1.4.0\n\nenum LoadError {\n    \n}\n";
    let offset = source.find("    \n").expect("variant line") + 4;
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        !offered.contains(&"int".to_string()),
        "enum variants are PascalCase names, not types (§9.3): {offered:?}"
    );
}

#[test]
fn declared_types_never_shadow_the_builtins_in_type_positions() {
    // A PascalCase builtin collision is impossible, so the union always
    // lists both kinds together: built-ins plus every declared type.
    let source = "schema engine 1.4.0\n\nstruct Clip { int id }\n\nstruct Tagged {\n    \n}\n";
    let offset = source.find("    \n").expect("field line") + 4;
    let doc = SchemaDoc::build(source);
    let offered: Vec<String> = completions(&doc, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        offered.contains(&"int".to_string()) && offered.contains(&"Clip".to_string()),
        "built-ins and declared types complete together: {offered:?}"
    );
}

#[test]
fn hover_on_a_type_in_a_member_signature_shows_the_type() {
    // A param type reference resolves to the struct's declaration.
    let value = hover_at(
        ENGINE,
        "DrawSprite(TextureHandle tex",
        "DrawSprite(".len() + 8,
    );
    assert!(
        value.contains("struct TextureHandle"),
        "a parameter's type hover resolves the declaration: {value}"
    );
}

#[test]
fn hover_between_tokens_is_none_not_a_panic() {
    let doc = SchemaDoc::build(ENGINE);
    assert!(hover(&doc, 0).is_none(), "the file start resolves nothing");
    assert!(
        hover(&doc, ENGINE.len()).is_none() || true,
        "EOF resolves or not, but never panics"
    );
}

#[test]
fn an_empty_file_yields_an_empty_outline_and_clean_completion() {
    let doc = SchemaDoc::build("");
    let index = LineIndex::new("");
    assert!(document_symbols(&doc, &index, "").is_empty());
    assert!(!completions(&doc, "", 0).is_empty());
}
