//! The completion suite: every context the language server completes in,
//! with the insert ("filling") behavior pinned alongside the suggestions.
//!
//! Contexts: member access (fields, variants, built-in enums, `.length`,
//! chains, indexing, calls), argument lists (named arguments in comma and
//! newline lists, positional mode, declarations), import paths, scope
//! completion (locals, scoping, top-level declarations, keywords), and the
//! suppression of comments and string literals.

mod common;

use common::*;
use tower_lsp_server::ls_types::CompletionItemKind;

// ---------------------------------------------------------------------------
// Member access — right after the dot
// ---------------------------------------------------------------------------

#[test]
fn fields_after_dot() {
    let src = "struct player {\n    str name\n    int health\n}\n\nint main() {\n    player p = player(name: \"h\", health: 1)\n    p.\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "p."));
    assert!(labels.contains(&"name".to_string()), "{labels:?}");
    assert!(labels.contains(&"health".to_string()), "{labels:?}");
    // Nothing else leaks into a member context.
    assert!(!labels.contains(&"main".to_string()), "{labels:?}");
    assert!(!labels.contains(&"int".to_string()), "{labels:?}");
}

#[test]
fn fields_are_typed_in_detail() {
    let src = "struct player {\n    str name\n    int health\n}\n\nint main() {\n    player p = player(name: \"h\", health: 1)\n    p.\n    return 0\n}\n";
    let a = analysis(src);
    let details = details_at(&a, src, last_offset_of(src, "p."));
    let name = details
        .iter()
        .find(|(l, _)| l == "name")
        .expect("name item");
    assert_eq!(name.1.as_deref(), Some("str"));
}

#[test]
fn member_completion_while_typing_the_member() {
    // The dot is followed by a partial member name: fields, not keywords.
    let src = "struct player {\n    str name\n    int health\n}\n\nint main() {\n    player p = player(name: \"h\", health: 1)\n    p.na\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "p.na"));
    assert!(labels.contains(&"name".to_string()), "{labels:?}");
    assert!(labels.contains(&"health".to_string()), "{labels:?}");
    assert!(!labels.contains(&"while".to_string()), "{labels:?}");
}

#[test]
fn member_completion_with_space_after_dot() {
    let src = "struct player {\n    str name\n}\n\nint main() {\n    player p = player(name: \"h\")\n    p. \n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "p. "));
    assert!(labels.contains(&"name".to_string()), "{labels:?}");
}

#[test]
fn member_completion_with_partial_name_and_trailing_cursor_mid_word() {
    // The cursor sits INSIDE the typed word: `p.na|me`.
    let src = "struct player {\n    str name\n}\n\nint main() {\n    player p = player(name: \"h\")\n    p.name\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "p.name", "p.na".len());
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"name".to_string()), "{labels:?}");
}

#[test]
fn no_member_completions_for_primitive_receivers() {
    let src = "int main() {\n    int x = 1\n    x.\n    return 0\n}\n";
    let a = analysis(src);
    assert!(labels_at(&a, src, last_offset_of(src, "x.")).is_empty());
}

#[test]
fn no_member_completions_for_str_receivers() {
    let src = "int main() {\n    str s = \"x\"\n    s.\n    return 0\n}\n";
    let a = analysis(src);
    assert!(labels_at(&a, src, last_offset_of(src, "s.")).is_empty());
}

#[test]
fn no_member_completions_for_map_receivers() {
    let src = "int main() {\n    map<str, int> m = map<str, int>{}\n    m.\n    return 0\n}\n";
    let a = analysis(src);
    assert!(labels_at(&a, src, last_offset_of(src, "m.")).is_empty());
}

#[test]
fn length_property_on_arrays() {
    let src = "int main() {\n    int[] xs = [1, 2]\n    xs.\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "xs."));
    assert_eq!(labels, vec!["length".to_string()]);
}

#[test]
fn length_property_through_a_param() {
    let src = "int first(int[] xs) {\n    return xs.\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "xs."));
    assert_eq!(labels, vec!["length".to_string()]);
}

#[test]
fn nested_field_chain() {
    let src = "struct vec2 {\n    float x\n    float y\n}\nstruct player {\n    vec2 position\n    int hp\n}\n\nint main() {\n    player p = player(position: vec2(x: 1.0, y: 2.0), hp: 10)\n    p.position.\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "p.position."));
    assert!(labels.contains(&"x".to_string()), "{labels:?}");
    assert!(labels.contains(&"y".to_string()), "{labels:?}");
    assert!(!labels.contains(&"hp".to_string()), "{labels:?}");
}

#[test]
fn chain_through_indexing() {
    let src = "struct player {\n    str name\n}\n\nint main() {\n    player[] ps = [player(name: \"a\"), player(name: \"b\")]\n    ps[0].\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "ps[0]."));
    assert_eq!(labels, vec!["name".to_string()]);
}

#[test]
fn chain_through_map_lookup() {
    let src = "struct player {\n    str name\n}\n\nint main() {\n    map<str, player> table = map<str, player>{}\n    table[\"ana\"].\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "table[\"ana\"]."));
    assert_eq!(labels, vec!["name".to_string()]);
}

#[test]
fn chain_through_call_result() {
    let src = "struct vec2 {\n    float x\n}\n\nvec2 origin() {\n    return vec2(x: 0.0)\n}\n\nint main() {\n    origin().\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "origin()."));
    assert_eq!(labels, vec!["x".to_string()]);
}

#[test]
fn parenthesized_receiver() {
    let src = "struct vec2 {\n    float x\n}\n\nint main() {\n    vec2 v = vec2(x: 1.0)\n    (v).\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "(v)."));
    assert_eq!(labels, vec!["x".to_string()]);
}

#[test]
fn infer_local_crystallizes_for_members() {
    let src = "struct vec2 {\n    float x\n}\n\nint main() {\n    infer v = vec2(x: 1.0)\n    v.\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "v."));
    assert_eq!(labels, vec!["x".to_string()]);
}

#[test]
fn enum_variants_after_enum_name() {
    let src = "enum gameEvent {\n    Damage(int amount)\n    Heal(int amount)\n    PlayerDied()\n}\n\nint main() {\n    gameEvent.\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "gameEvent."));
    assert_eq!(
        labels,
        vec![
            "Damage".to_string(),
            "Heal".to_string(),
            "PlayerDied".to_string()
        ]
    );
}

#[test]
fn enum_payload_identifiers_are_not_variants() {
    // Regression: `Damage(int amount)` must not register `amount` as a
    // variant of the enum.
    let src = "enum gameEvent {\n    Damage(int amount)\n    Heal(int amount)\n}\n\nint main() {\n    gameEvent.\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "gameEvent."));
    assert!(!labels.contains(&"amount".to_string()), "{labels:?}");
    assert!(!labels.contains(&"int".to_string()), "{labels:?}");
}

#[test]
fn variant_items_fill_with_the_variant_name() {
    // Regression: the fill used to be the payload text, so accepting
    // `Damage` inserted `amount: int` (and variants are POSITIONAL-only,
    // so a named fill could never work).
    let src = "enum gameEvent {\n    Damage(int amount)\n}\n\nint main() {\n    gameEvent.\n    return 0\n}\n";
    let a = analysis(src);
    let fills = fills_at(&a, src, last_offset_of(src, "gameEvent."));
    let damage = fills
        .iter()
        .find(|(l, _)| l == "Damage")
        .expect("Damage item");
    assert_eq!(damage.1, None, "the label is the fill: {fills:?}");
}

#[test]
fn variant_detail_is_qualified_with_payload() {
    let src = "enum gameEvent {\n    Damage(int amount)\n}\n\nint main() {\n    gameEvent.\n    return 0\n}\n";
    let a = analysis(src);
    let details = details_at(&a, src, last_offset_of(src, "gameEvent."));
    let damage = details
        .iter()
        .find(|(l, _)| l == "Damage")
        .expect("Damage item");
    assert_eq!(damage.1.as_deref(), Some("gameEvent.Damage(amount: int)"));
}

#[test]
fn generic_struct_fields_show_substituted_types() {
    let src = "struct pair<A, B> {\n    A first\n    B second\n}\n\nint main() {\n    pair<int, str> p = pair(first: 1, second: \"s\")\n    p.\n    return 0\n}\n";
    let a = analysis(src);
    let details = details_at(&a, src, last_offset_of(src, "p."));
    let first = details
        .iter()
        .find(|(l, _)| l == "first")
        .expect("first item");
    assert_eq!(first.1.as_deref(), Some("int"), "{details:?}");
    let second = details
        .iter()
        .find(|(l, _)| l == "second")
        .expect("second item");
    assert_eq!(second.1.as_deref(), Some("str"), "{details:?}");
}

#[test]
fn builtin_option_constructors_after_option_dot() {
    let src = "int main() {\n    option<int> m = option.\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "option."));
    assert_eq!(labels, vec!["Some".to_string(), "None".to_string()]);
}

#[test]
fn builtin_result_constructors_after_result_dot() {
    let src = "int main() {\n    result<int, str> r = result.\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "result."));
    assert_eq!(labels, vec!["Ok".to_string(), "Err".to_string()]);
}

#[test]
fn dot_at_line_end_does_not_leak_into_the_next_line() {
    // A trailing dot is member context only while the cursor stays on
    // that line; from the next line the receiver chain is broken.
    let src = "struct vec2 {\n    float x\n}\n\nint main() {\n    vec2 v = vec2(x: 1.0)\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "return 0", 0);
    let labels = labels_at(&a, src, offset);
    assert!(!labels.contains(&"x".to_string()), "{labels:?}");
}

// ---------------------------------------------------------------------------
// Member access — assignment targets and other statement shapes
// ---------------------------------------------------------------------------

#[test]
fn member_completion_in_assignment_target() {
    let src = "struct player {\n    str name\n}\n\nint main() {\n    player p = player(name: \"h\")\n    p.\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "p."));
    assert_eq!(labels, vec!["name".to_string()]);
}

#[test]
fn member_completion_on_a_param_receiver() {
    let src = "struct player {\n    int hp\n}\n\nint damage(player p) {\n    p.\n    return 0\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "p."));
    assert_eq!(labels, vec!["hp".to_string()]);
}

#[test]
fn member_completion_after_return_member_prefix() {
    let src = "struct player {\n    int hp\n}\n\nint get(player p) {\n    return p.\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "p."));
    assert_eq!(labels, vec!["hp".to_string()]);
}

// ---------------------------------------------------------------------------
// Argument lists — named arguments and fills
// ---------------------------------------------------------------------------

#[test]
fn first_named_argument_of_a_construction() {
    let src = "struct vec2 {\n    float x\n    float y\n}\n\nint main() {\n    vec2 v = vec2()\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "vec2()", "vec2(".len());
    let fills = fills_at(&a, src, offset);
    let x = fills.iter().find(|(l, _)| l == "x").expect("x item");
    assert_eq!(x.1.as_deref(), Some("x: "));
    let y = fills.iter().find(|(l, _)| l == "y").expect("y item");
    assert_eq!(y.1.as_deref(), Some("y: "));
}

#[test]
fn named_arguments_while_typing_a_partial_name() {
    let src = "struct vec2 {\n    float x\n    float y\n}\n\nint main() {\n    vec2 v = vec2(x\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "vec2(x", "vec2(x".len());
    let labels = labels_at(&a, src, offset);
    assert!(
        labels.contains(&"x".to_string()) && labels.contains(&"y".to_string()),
        "{labels:?}"
    );
}

#[test]
fn second_named_argument_skips_the_used_one() {
    let src = "struct vec2 {\n    float x\n    float y\n}\n\nint main() {\n    vec2 v = vec2(x: 1.0, )\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "x: 1.0, )", "x: 1.0, ".len());
    let labels = labels_at(&a, src, offset);
    assert_eq!(labels, vec!["y".to_string()], "x was used; {labels:?}");
}

#[test]
fn second_named_argument_in_a_newline_list() {
    // Newline-delimited named arguments inside parens (§2.6, §2.12).
    let src = "struct player {\n    str name\n    int health\n}\n\nint main() {\n    player p = player(\n        name: \"h\"\n        \n    )\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(
        src,
        "name: \"h\"\n        \n",
        "name: \"h\"\n        ".len(),
    );
    let labels = labels_at(&a, src, offset);
    assert_eq!(labels, vec!["health".to_string()], "{labels:?}");
}

#[test]
fn named_arguments_of_a_function_call() {
    let src = "int heal(int amount, int times) {\n    return amount\n}\n\nint main() {\n    heal()\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "heal()", "heal(".len());
    let fills = fills_at(&a, src, offset);
    assert_eq!(
        fills,
        vec![
            ("amount".to_string(), Some("amount: ".to_string())),
            ("times".to_string(), Some("times: ".to_string())),
        ]
    );
}

#[test]
fn named_argument_fill_for_a_param_shows_the_type() {
    let src = "int heal(int amount) {\n    return amount\n}\n\nint main() {\n    heal()\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "heal()", "heal(".len());
    let details = details_at(&a, src, offset);
    assert_eq!(
        details,
        vec![("amount".to_string(), Some("amount: int".to_string()))]
    );
}

#[test]
fn used_names_are_skipped_across_the_list() {
    let src = "int heal(int amount, int times) {\n    return amount\n}\n\nint main() {\n    heal(amount: 1, )\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "amount: 1, )", "amount: 1, ".len());
    let labels = labels_at(&a, src, offset);
    assert_eq!(labels, vec!["times".to_string()], "{labels:?}");
}

#[test]
fn positional_call_switches_named_suggestions_off() {
    // §2.12: positional and named are exclusive; after a positional
    // argument no named fills may appear.
    let src = "int heal(int amount, int times) {\n    return amount\n}\n\nint main() {\n    heal(1, )\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "heal(1, )", "heal(1, ".len());
    let fills = fills_at(&a, src, offset);
    assert!(
        !fills
            .iter()
            .any(|(l, f)| (l == "times" || l == "amount") && f.is_some()),
        "named fills in a positional call: {fills:?}"
    );
}

#[test]
fn typing_a_named_value_falls_back_to_scope() {
    // `vec2(x: ` — the user is typing the VALUE; scope completions apply,
    // not the remaining parameter names.
    let src = "struct vec2 {\n    float x\n    float y\n}\n\nint main() {\n    vec2 v = vec2(x: \n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "vec2(x: \n", "vec2(x: ".len());
    let fills = fills_at(&a, src, offset);
    assert!(!fills.iter().any(|(l, _)| l == "y"), "{fills:?}");
}

#[test]
fn innermost_call_wins_for_nested_calls() {
    let src = "struct vec2 {\n    float x\n    float y\n}\n\nint outer(int v) {\n    return v\n}\n\nint main() {\n    outer(1)\n    return 0\n}\n";
    let src = src.replace("outer(1)", "outer(vec2(");
    let a = analysis(&src);
    let offset = last_offset_of(&src, "outer(vec2(");
    let labels = labels_at(&a, &src, offset);
    assert!(
        labels.contains(&"x".to_string()) && labels.contains(&"y".to_string()),
        "{labels:?}"
    );
    assert!(!labels.contains(&"v".to_string()), "{labels:?}");
}

#[test]
fn impl_member_named_arguments() {
    let src = "struct vec2 {\n    float x\n    float y\n}\n\nimpl vec2 {\n    float dot(vec2 other) {\n        return 0.0\n    }\n}\n\nint main() {\n    vec2 v = vec2(x: 1.0, y: 2.0)\n    float d = vec2.dot()\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "vec2.dot()", "vec2.dot(".len());
    let fills = fills_at(&a, src, offset);
    assert_eq!(
        fills,
        vec![("other".to_string(), Some("other: ".to_string()))],
        "{fills:?}"
    );
}

#[test]
fn variant_constructor_payloads_are_detail_only() {
    // Variants take POSITIONAL payloads; the fields render as detail and
    // never as `name: ` fills.
    let src = "enum gameEvent {\n    Spawn(str enemyKind)\n}\n\nint main() {\n    gameEvent e = gameEvent.Spawn()\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "gameEvent.Spawn()", "gameEvent.Spawn(".len());
    let fills = fills_at(&a, src, offset);
    assert_eq!(fills, vec![("enemyKind".to_string(), None)], "{fills:?}");
}

#[test]
fn zero_parameter_call_offers_nothing_special() {
    let src = "int main() {\n    main()\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "main()", "main(".len());
    let fills = fills_at(&a, src, offset);
    assert!(
        !fills
            .iter()
            .any(|(_, f)| f.as_deref().is_some_and(|t| t.ends_with(": "))),
        "no named fills for a zero-param call: {fills:?}"
    );
}

#[test]
fn unknown_callee_falls_back_to_scope() {
    let src = "int main() {\n    unknownFn()\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "unknownFn()", "unknownFn(".len());
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"main".to_string()), "{labels:?}");
}

#[test]
fn control_flow_parens_are_not_calls() {
    for keyword in ["if", "while", "for", "match"] {
        let src = format!("int main() {{\n    {keyword} (heal)\n    return 0\n}}\n");
        let a = analysis(&src);
        let offset = offset_of(&src, "(heal)", "(".len());
        let fills = fills_at(&a, &src, offset);
        assert!(
            !fills.iter().any(|(l, f)| l == "heal" && f.is_some()),
            "{keyword} must not offer call fills: {fills:?}"
        );
    }
}

#[test]
fn declaration_paren_is_not_a_call() {
    // Regression: writing `int add(` used to offer add's own parameters
    // as named arguments.
    let src = "int add(int a, int b) {\n    return a + b\n}\n\nint add(\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "int add(");
    let fills = fills_at(&a, src, offset);
    assert!(
        !fills
            .iter()
            .any(|(l, f)| (l == "a" || l == "b") && f.is_some()),
        "declaration site offered named arguments: {fills:?}"
    );
}

#[test]
fn declaration_with_array_return_is_not_a_call() {
    let src = "int[] make(int n) {\n    return []\n}\n\nint[] make(\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "int[] make(");
    let fills = fills_at(&a, src, offset);
    assert!(!fills.iter().any(|(l, _)| l == "n"), "{fills:?}");
}

#[test]
fn declaration_with_generic_return_is_not_a_call() {
    let src = "struct pair<A, B> {\n    A first\n    B second\n}\n\npair<int, int> make(int n) {\n    return pair(first: 0, second: 0)\n}\n\npair<int, int> make(\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "pair<int, int> make(");
    let fills = fills_at(&a, src, offset);
    assert!(!fills.iter().any(|(l, _)| l == "n"), "{fills:?}");
}

#[test]
fn calls_after_assignments_and_returns_are_calls() {
    let src =
        "struct vec2 {\n    float x\n}\n\nint main() {\n    vec2 v = vec2()\n    return 0\n}\n";
    let a = analysis(src);
    let offset = src.find("vec2()").unwrap() + "vec2(".len();
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"x".to_string()), "{labels:?}");
}

// ---------------------------------------------------------------------------
// Import paths
// ---------------------------------------------------------------------------

#[test]
fn fresh_import_statement_offers_self_and_known_roots() {
    let src = "import engine.graphics\nimport \nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = src.rfind("import ").unwrap() + "import ".len();
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"self".to_string()), "{labels:?}");
    assert!(labels.contains(&"engine".to_string()), "{labels:?}");
}

#[test]
fn import_after_dot_offers_deeper_segments() {
    // Regression: `import engine.` used to return NOTHING because the
    // member-context branch answered first with an empty list.
    let src = "import engine.graphics\nimport engine.\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = src.rfind("import engine.").unwrap() + "import engine.".len();
    let labels = labels_at(&a, src, offset);
    assert_eq!(labels, vec!["graphics".to_string()], "{labels:?}");
}

#[test]
fn import_self_depth_uses_self_rooted_imports() {
    let src = "import self.gamemode.rules\nimport self.\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = src.rfind("import self.").unwrap() + "import self.".len();
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"gamemode".to_string()), "{labels:?}");
    assert!(
        !labels.contains(&"self".to_string()),
        "`self` only at the root: {labels:?}"
    );
}

#[test]
fn import_partial_segment_still_suggests() {
    let src = "import engine.graphics\nimport engine.g\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = src.rfind("import engine.g").unwrap() + "import engine.g".len();
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"graphics".to_string()), "{labels:?}");
}

#[test]
fn import_prefix_mismatch_has_no_candidates() {
    let src = "import engine.graphics\nimport zzz.\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = src.rfind("import zzz.").unwrap() + "import zzz.".len();
    assert!(labels_at(&a, src, offset).is_empty());
}

#[test]
fn non_import_lines_are_not_import_context() {
    let src = "import engine.graphics\n\nint main() {\n    int x = \n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "int x = ");
    let labels = labels_at(&a, src, offset);
    assert!(!labels.contains(&"self".to_string()), "{labels:?}");
}

#[test]
fn importx_is_not_the_import_keyword() {
    let src = "int main() {\n    return 0\n}\n\nimportx en\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "importx en");
    let labels = labels_at(&a, src, offset);
    assert!(!labels.contains(&"self".to_string()), "{labels:?}");
}

// ---------------------------------------------------------------------------
// Suppression: comments and strings
// ---------------------------------------------------------------------------

#[test]
fn nothing_inside_a_line_comment() {
    let src = "int main() {\n    // hello wor\n    return 0\n}\n";
    let a = analysis(src);
    assert!(labels_at(&a, src, last_offset_of(src, "// hello wor")).is_empty());
}

#[test]
fn nothing_in_a_comment_after_code_on_the_same_line() {
    let src = "int main() {\n    int hp = 100 // note to sel\n    return hp\n}\n";
    let a = analysis(src);
    assert!(labels_at(&a, src, last_offset_of(src, "// note to sel")).is_empty());
}

#[test]
fn slashes_in_a_string_are_not_comments() {
    // `//` inside a string is content; the cursor is suppressed by the
    // STRING rule, but the important part is that nothing is offered.
    let src = "int main() {\n    str url = \"http://x\"\n    return 0\n}\n";
    let a = analysis(src);
    let inside = offset_of(src, "//x\"", 0) - 1;
    assert!(labels_at(&a, src, inside).is_empty());
}

#[test]
fn nothing_inside_a_block_comment() {
    let src = "int main() {\n    /* still insi\n    return 0\n}\n";
    let a = analysis(src);
    assert!(labels_at(&a, src, last_offset_of(src, "still insi")).is_empty());
}

#[test]
fn nothing_right_after_a_block_comment_opener() {
    let src = "int main() {\n    /*\n    return 0\n}\n";
    let a = analysis(src);
    assert!(labels_at(&a, src, last_offset_of(src, "/*")).is_empty());
}

#[test]
fn code_after_a_closed_block_comment_completes() {
    let src = "/* header */\n\nint main() {\n    int hp = 1\n    return hp\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "return hp"));
    assert!(labels.contains(&"hp".to_string()), "{labels:?}");
}

#[test]
fn nothing_inside_a_plain_string() {
    let src = "int main() {\n    str s = \"hello wor\"\n    return 0\n}\n";
    let a = analysis(src);
    // The cursor sits right before the closing quote.
    let inside = last_offset_of(src, "hello wor");
    assert!(labels_at(&a, src, inside).is_empty());
}

#[test]
fn nothing_in_the_literal_part_of_an_interpolated_string() {
    let src = "int main() {\n    int hp = 10\n    str s = $\"HP: {hp} and mor\"\n    return 0\n}\n";
    let a = analysis(src);
    assert!(labels_at(&a, src, last_offset_of(src, "and mor")).is_empty());
}

#[test]
fn interpolation_islands_stay_completable() {
    let src = "int main() {\n    int hp = 10\n    str s = $\"HP: {hp\"\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, last_offset_of(src, "{hp"));
    assert!(labels.contains(&"hp".to_string()), "{labels:?}");
}

#[test]
fn cursor_between_islands_is_suppressed() {
    let src = "int main() {\n    int hp = 10\n    str s = $\"{hp} gloal text\"\n    return 0\n}\n";
    let a = analysis(src);
    assert!(labels_at(&a, src, last_offset_of(src, "gloal text")).is_empty());
}

#[test]
fn empty_document_completes_keywords_without_panicking() {
    let a = analysis("");
    let labels = labels_at(&a, "", 0);
    assert!(labels.contains(&"int".to_string()), "{labels:?}");
}

#[test]
fn offset_past_end_is_clamped_without_panicking() {
    let src = "int main() {\n    return 0\n}\n";
    let a = analysis(src);
    let labels = labels_at(&a, src, src.len() + 500);
    assert!(labels.contains(&"int".to_string()), "{labels:?}");
}

#[test]
fn unicode_before_the_cursor_does_not_break_completion() {
    let src = "int main() {\n    str s = \"日本語テキスト\"\n    player p = player(name: \"h\")\n    return 0\n}\n";
    let src = src.replace("player p", "int q = 1\n    player p");
    let a = analysis(&src);
    let offset = last_offset_of(&src, "return 0");
    let labels = labels_at(&a, &src, offset);
    assert!(labels.contains(&"q".to_string()), "{labels:?}");
}

// ---------------------------------------------------------------------------
// Scope completion: locals, scoping, declarations, keywords
// ---------------------------------------------------------------------------

#[test]
fn locals_and_params_complete_inside_their_function() {
    let src = "int heal(int amount) {\n    int boost = 2\n    return amount\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return amount");
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"amount".to_string()), "{labels:?}");
    assert!(labels.contains(&"boost".to_string()), "{labels:?}");
    // Locals of OTHER functions never leak.
    assert!(!labels.iter().any(|l| l == "main_local"), "{labels:?}");
}

#[test]
fn top_level_declarations_complete_everywhere() {
    let src = "int score = 0\n\nint heal(int amount) {\n    return amount\n}\n\nstruct vec2 {\n    float x\n}\n\nenum gameEvent {\n    Damage(int amount)\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let labels = labels_at(&a, src, offset);
    for expected in ["heal", "main", "vec2", "gameEvent"] {
        assert!(
            labels.contains(&expected.to_string()),
            "missing {expected}: {labels:?}"
        );
    }
}

#[test]
fn impl_members_do_not_complete_as_top_level_functions() {
    let src = "struct vec2 {\n    float x\n}\n\nimpl vec2 {\n    float dot(vec2 other) {\n        return 0.0\n    }\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let labels = labels_at(&a, src, offset);
    assert!(
        !labels.contains(&"dot".to_string()),
        "impl member in scope completion: {labels:?}"
    );
}

#[test]
fn locals_declared_after_the_cursor_do_not_complete() {
    let src = "int main() {\n    return 0\n    int later = 1\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let labels = labels_at(&a, src, offset);
    assert!(!labels.contains(&"later".to_string()), "{labels:?}");
}

#[test]
fn for_bindings_complete_in_the_loop_body() {
    let src = "int total(int[] xs) {\n    int sum = 0\n    for (int x in xs) {\n        sum = sum + x\n    }\n    return sum\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "sum + x");
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"x".to_string()), "{labels:?}");
}

#[test]
fn for_bindings_do_not_survive_the_loop() {
    // Regression: block scoping — the binding is scoped to the loop body.
    let src = "int total(int[] xs) {\n    int sum = 0\n    for (int x in xs) {\n        sum = sum + x\n    }\n    return sum\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return sum");
    let labels = labels_at(&a, src, offset);
    assert!(
        !labels.contains(&"x".to_string()),
        "binding leaked out of the loop: {labels:?}"
    );
}

#[test]
fn if_branch_locals_do_not_survive_the_branch() {
    // Regression: block scoping for if bodies.
    let src = "int main() {\n    if (true) {\n        int inner = 1\n    }\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let labels = labels_at(&a, src, offset);
    assert!(
        !labels.contains(&"inner".to_string()),
        "if-local leaked out: {labels:?}"
    );
}

#[test]
fn while_body_locals_do_not_survive_the_loop() {
    let src = "int main() {\n    while (false) {\n        int inner = 1\n    }\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let labels = labels_at(&a, src, offset);
    assert!(!labels.contains(&"inner".to_string()), "{labels:?}");
}

#[test]
fn match_bindings_complete_in_their_arm() {
    let src = "enum gameEvent {\n    Damage(int amount)\n}\n\nint apply(gameEvent e) {\n    match (e) {\n        Damage(int amount) => {\n            return amount\n        }\n    }\n    return 0\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return amount");
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"amount".to_string()), "{labels:?}");
}

#[test]
fn match_bindings_do_not_survive_their_arm() {
    let src = "enum gameEvent {\n    Damage(int amount)\n}\n\nint apply(gameEvent e) {\n    match (e) {\n        Damage(int amount) => {\n            return 0\n        }\n    }\n    return 0\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "    return 0\n}\n\nint main", "    return 0".len());
    let labels = labels_at(&a, src, offset);
    assert!(
        !labels.contains(&"amount".to_string()),
        "binding leaked out of the arm: {labels:?}"
    );
}

#[test]
fn else_if_chain_locals_are_collected() {
    let src = "int pick(int v) {\n    if (v > 10) {\n        int big = 1\n        return big\n    } else if (v > 5) {\n        int mid = 2\n        return mid\n    } else {\n        return 0\n    }\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return mid");
    let labels = labels_at(&a, src, offset);
    assert!(labels.contains(&"mid".to_string()), "{labels:?}");
}

#[test]
fn keyword_list_includes_in() {
    let src = "int main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let labels = labels_at(&a, src, offset);
    for keyword in [
        "in", "for", "while", "match", "infer", "impl", "import", "return",
    ] {
        assert!(
            labels.contains(&keyword.to_string()),
            "missing keyword {keyword}: {labels:?}"
        );
    }
}

#[test]
fn builtin_constructors_complete_at_statement_position() {
    let src = "int main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let labels = labels_at(&a, src, offset);
    for constructor in ["Ok", "Err", "Some", "None"] {
        assert!(
            labels.contains(&constructor.to_string()),
            "missing {constructor}: {labels:?}"
        );
    }
}

#[test]
fn function_fills_open_the_call() {
    let src = "int heal(int amount) {\n    return amount\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let fills = fills_at(&a, src, offset);
    let heal = fills.iter().find(|(l, _)| l == "heal").expect("heal item");
    assert_eq!(heal.1.as_deref(), Some("heal("), "{fills:?}");
}

#[test]
fn zero_parameter_function_fills_the_whole_call() {
    let src = "int main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let fills = fills_at(&a, src, offset);
    let main = fills.iter().find(|(l, _)| l == "main").expect("main item");
    assert_eq!(main.1.as_deref(), Some("main()"), "{fills:?}");
}

#[test]
fn type_declarations_fill_as_their_name() {
    // Regression: accepting a struct used to insert its field list.
    let src = "struct vec2 {\n    float x\n}\n\nenum gameEvent {\n    Damage(int amount)\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let fills = fills_at(&a, src, offset);
    let vec2 = fills.iter().find(|(l, _)| l == "vec2").expect("vec2 item");
    assert_eq!(vec2.1, None, "struct fill is the label: {fills:?}");
    let event = fills
        .iter()
        .find(|(l, _)| l == "gameEvent")
        .expect("gameEvent item");
    assert_eq!(event.1, None, "enum fill is the label: {fills:?}");
}

#[test]
fn local_fills_are_their_name_and_typed_in_detail() {
    let src = "int main() {\n    int hp = 100\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let fills = fills_at(&a, src, offset);
    let hp = fills.iter().find(|(l, _)| l == "hp").expect("hp item");
    assert_eq!(hp.1, None, "local fill is the label: {fills:?}");
    let details = details_at(&a, src, offset);
    let hp_detail = details.iter().find(|(l, _)| l == "hp").expect("hp detail");
    assert_eq!(hp_detail.1.as_deref(), Some("hp: int"));
}

#[test]
fn items_carry_kind_hints() {
    let src = "struct vec2 {\n    float x\n}\n\nint heal(int amount) {\n    return amount\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = last_offset_of(src, "return 0");
    let items = complete_at(&a, src, offset);
    let by_label = |name: &str| items.iter().find(|i| i.label == name).expect(name);
    assert_eq!(by_label("heal").kind, Some(CompletionItemKind::FUNCTION));
    assert_eq!(by_label("vec2").kind, Some(CompletionItemKind::STRUCT));
    assert_eq!(by_label("int").kind, Some(CompletionItemKind::KEYWORD));
    assert_eq!(by_label("Some").kind, Some(CompletionItemKind::ENUM_MEMBER));
}
