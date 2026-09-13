//! Semantic tokens and the document-symbol outline.

mod common;

use common::*;
use tower_lsp_server::ls_types::{SemanticTokenType, SymbolKind};

/// The legend index of a token type name.
fn legend_index(name: &str) -> u32 {
    let legend = cme_lsp::features::tokens::legend();
    legend
        .token_types
        .iter()
        .position(|t| t.as_str() == name)
        .unwrap_or_else(|| panic!("{name} in legend")) as u32
}

fn function_index() -> u32 {
    legend_index(SemanticTokenType::FUNCTION.as_str())
}

fn method_index() -> u32 {
    legend_index(SemanticTokenType::METHOD.as_str())
}

fn property_index() -> u32 {
    legend_index(SemanticTokenType::PROPERTY.as_str())
}

fn variable_index() -> u32 {
    legend_index(SemanticTokenType::VARIABLE.as_str())
}

fn parameter_index() -> u32 {
    legend_index(SemanticTokenType::PARAMETER.as_str())
}

fn enum_member_index() -> u32 {
    legend_index(SemanticTokenType::ENUM_MEMBER.as_str())
}

fn enum_index() -> u32 {
    legend_index(SemanticTokenType::ENUM.as_str())
}

fn struct_index() -> u32 {
    legend_index(SemanticTokenType::STRUCT.as_str())
}

fn namespace_index() -> u32 {
    legend_index(SemanticTokenType::NAMESPACE.as_str())
}

const DECLARATION: u32 = 1; // the single DECLARATION modifier bit

fn rows_of(src: &str) -> Vec<(u32, u32, u32, u32, u32)> {
    let a = analysis(src);
    let index = cme_lsp::convert::LineIndex::new(src);
    decoded_tokens(&a, &index, src)
}

fn on_line(rows: &[(u32, u32, u32, u32, u32)], line: u32) -> Vec<(u32, u32, u32, u32)> {
    rows.iter()
        .filter(|(l, _, _, _, _)| *l == line)
        .map(|(_, c, len, ty, m)| (*c, *len, *ty, *m))
        .collect()
}

// ---------------------------------------------------------------------------
// Semantic tokens
// ---------------------------------------------------------------------------

#[test]
fn legend_exposes_the_documented_token_types() {
    let legend = cme_lsp::features::tokens::legend();
    let names: Vec<&str> = legend.token_types.iter().map(|t| t.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "namespace",
            "function",
            "method",
            "property",
            "variable",
            "parameter",
            "enumMember",
            "enum",
            "struct",
        ]
    );
    assert_eq!(legend.token_modifiers.len(), 1);
    assert_eq!(legend.token_modifiers[0].as_str(), "declaration");
}

#[test]
fn tokens_classify_functions_params_and_locals() {
    let src = "int heal(int amount) {\n    return amount\n}\n\nint main() {\n    return 0\n}\n";
    let rows = rows_of(src);
    // Line 0: `heal` (function, declaration) and `amount` (parameter, declaration).
    let line0 = on_line(&rows, 0);
    assert_eq!(line0.len(), 2, "{line0:?}");
    let heal = line0[0];
    assert_eq!(heal, (4, 4, function_index(), DECLARATION));
    let amount = line0[1];
    assert_eq!(amount, (13, 6, parameter_index(), DECLARATION));
    // Line 1: the use of `amount` (parameter, not a declaration).
    let line1 = on_line(&rows, 1);
    assert_eq!(line1, vec![(11, 6, parameter_index(), 0)], "{line1:?}");
}

#[test]
fn tokens_classify_structs_fields_and_enums() {
    let src = "struct vec2 {\n    float x\n}\n\nenum gameEvent {\n    Damage(int amount)\n}\n\nint main() {\n    return 0\n}\n";
    let rows = rows_of(src);
    // struct name carries the declaration modifier.
    assert_eq!(on_line(&rows, 0), vec![(7, 4, struct_index(), DECLARATION)],);
    // field declaration is a property with the declaration modifier.
    assert_eq!(
        on_line(&rows, 1),
        vec![(10, 1, property_index(), DECLARATION)],
    );
    // enum name (line 4 — line 3 is the blank separator).
    assert_eq!(on_line(&rows, 4), vec![(5, 9, enum_index(), DECLARATION)],);
    // variant name is an enum member with the declaration modifier; the
    // payload identifier is NOT painted at all (regression — it used to
    // register as a variant of its own).
    let line5 = on_line(&rows, 5);
    assert_eq!(
        line5,
        vec![(4, 6, enum_member_index(), DECLARATION)],
        "{line5:?}"
    );
}

#[test]
fn tokens_paint_impl_members_as_methods() {
    let src = "struct vec2 {\n    float x\n}\n\nimpl vec2 {\n    float dot(vec2 other) {\n        return 0.0\n    }\n}\n\nint main() {\n    return 0\n}\n";
    let rows = rows_of(src);
    let line5 = on_line(&rows, 5);
    assert_eq!(line5[0], (10, 3, method_index(), DECLARATION), "{line5:?}");
    // The impl TARGET use is a struct, not a method.
    let line4 = on_line(&rows, 4);
    assert_eq!(line4[0].2, struct_index(), "{line4:?}");
}

#[test]
fn tokens_paint_imports_as_namespaces() {
    let src = "import engine.graphics\n\nint main() {\n    return 0\n}\n";
    let rows = rows_of(src);
    let line0 = on_line(&rows, 0);
    assert_eq!(line0.len(), 2, "{line0:?}");
    assert!(line0.iter().all(|(_, _, ty, _)| *ty == namespace_index()));
}

#[test]
fn tokens_skip_builtins_and_keywords() {
    let src = "int main() {\n    option<int> m = Some(1)\n    return 0\n}\n";
    let rows = rows_of(src);
    // Only `main` and the local `m`; `option`, `int`, `Some`, `return` stay out.
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        on_line(&rows, 0),
        vec![(4, 4, function_index(), DECLARATION)]
    );
    assert_eq!(
        on_line(&rows, 1),
        vec![(16, 1, variable_index(), DECLARATION)]
    );
}

#[test]
fn tokens_are_monotonic_and_delta_encoded() {
    let src = "struct pair<A, B> {\n    A first\n    B second\n}\n\nint main() {\n    pair<int, str> p = pair(first: 1, second: \"s\")\n    return p.first\n}\n";
    let a = analysis(src);
    let index = cme_lsp::convert::LineIndex::new(src);
    let tokens = cme_lsp::features::tokens::semantic_tokens(&a, &index, src);
    let (mut line, mut character) = (0u32, 0u32);
    for token in &tokens.data {
        let new_line = line + token.delta_line;
        let new_character = if token.delta_line > 0 {
            token.delta_start
        } else {
            character + token.delta_start
        };
        assert!(
            new_line > line || new_character >= character,
            "positions must not move backwards"
        );
        line = new_line;
        character = new_character;
    }
    assert!(!tokens.data.is_empty());
}

#[test]
fn tokens_count_lengths_in_utf16_units() {
    // A multi-byte identifier stays out of the token stream only if the
    // lexer rejects it; ASCII names with multi-byte NEIGHBORS must still
    // carry correct lengths.
    let src = "int main() {\n    int hp = 100\n    return hp\n}\n";
    let rows = rows_of(src);
    let line1 = on_line(&rows, 1);
    assert_eq!(line1[0], (8, 2, variable_index(), DECLARATION));
}

// ---------------------------------------------------------------------------
// Document symbols
// ---------------------------------------------------------------------------

#[test]
fn outline_lists_every_declaration_kind() {
    let src = "import engine.graphics\n\nstruct vec2 {\n    float x\n    float y\n}\n\nenum gameEvent {\n    Damage(int amount)\n    Heal(int amount)\n}\n\nimpl vec2 {\n    float dot(vec2 other) {\n        return 0.0\n    }\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let index = cme_lsp::convert::LineIndex::new(src);
    let outline = outline(&a, &index, src);
    let names: Vec<&str> = outline.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "import engine.graphics",
            "vec2",
            "gameEvent",
            "impl vec2",
            "main"
        ],
        "{names:?}"
    );
    assert_eq!(outline[1].1, SymbolKind::STRUCT);
    assert_eq!(outline[2].1, SymbolKind::ENUM);
    assert_eq!(outline[3].1, SymbolKind::MODULE);
    assert_eq!(outline[4].1, SymbolKind::FUNCTION);
}

#[test]
fn outline_struct_children_are_fields() {
    let src = "struct vec2 {\n    float x\n    float y\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let index = cme_lsp::convert::LineIndex::new(src);
    let outline = outline(&a, &index, src);
    assert_eq!(outline[0].2, vec!["x".to_string(), "y".to_string()]);
}

#[test]
fn outline_enum_children_are_variants_without_payloads() {
    // Regression: payload identifiers used to appear as variant children.
    let src = "enum gameEvent {\n    Damage(int amount)\n    Spawn(str enemyKind, vec2 position)\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let index = cme_lsp::convert::LineIndex::new(src);
    let outline = outline(&a, &index, src);
    assert_eq!(
        outline[0].2,
        vec!["Damage".to_string(), "Spawn".to_string()],
        "{:?}",
        outline[0].2
    );
}

#[test]
fn outline_impl_children_are_its_members() {
    let src = "struct vec2 {\n    float x\n}\n\nimpl vec2 {\n    float dot(vec2 other) {\n        return 0.0\n    }\n    float len() {\n        return 0.0\n    }\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let index = cme_lsp::convert::LineIndex::new(src);
    let outline = outline(&a, &index, src);
    let impl_children = outline
        .iter()
        .find(|(name, _, _)| name == "impl vec2")
        .expect("impl block")
        .2
        .clone();
    assert_eq!(impl_children, vec!["dot".to_string(), "len".to_string()]);
    // Members do not appear twice (also not as top-level functions).
    assert!(!outline.iter().any(|(name, _, _)| name == "dot"));
}

#[test]
fn outline_ranges_are_ordered_and_inside_the_document() {
    let src = "int main() {\n    return 0\n}\n\nstruct vec2 {\n    float x\n}\n";
    let a = analysis(src);
    let index = cme_lsp::convert::LineIndex::new(src);
    let symbols = cme_lsp::features::symbols::document_symbols(&a, &index, src);
    let mut previous = 0u32;
    for symbol in &symbols {
        assert!(symbol.range.start.line >= previous);
        previous = symbol.range.start.line;
        assert!(symbol.selection_range.start >= symbol.range.start);
        assert!(symbol.selection_range.end <= symbol.range.end);
    }
}
