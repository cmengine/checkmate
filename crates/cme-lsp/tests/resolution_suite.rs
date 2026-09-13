//! Resolution suite: hover, go-to-definition, and find-references over
//! every `Resolved` shape, plus the block-scoping rules the checker
//! enforces and the enum symbol-table regression (payload identifiers are
//! not variants).

mod common;

use common::*;

// ---------------------------------------------------------------------------
// Hover — locals, params, and their qualifiers
// ---------------------------------------------------------------------------

#[test]
fn hover_on_local_shows_type_and_qualifier() {
    let src = "int main() {\n    int hp = 100\n    return hp\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "return hp")).expect("hp resolves");
    assert!(hover.contains("hp: int"), "{hover}");
    assert!(hover.contains("*variable*"), "{hover}");
}

#[test]
fn hover_on_param_shows_parameter_qualifier() {
    let src = "int heal(int amount) {\n    return amount\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "return amount")).expect("amount resolves");
    assert!(hover.contains("amount: int"), "{hover}");
    assert!(hover.contains("*parameter*"), "{hover}");
}

#[test]
fn hover_on_for_binding() {
    let src = "int total(int[] xs) {\n    int sum = 0\n    for (int x in xs) {\n        sum = sum + x\n    }\n    return sum\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "sum + x")).expect("x resolves");
    assert!(hover.contains("x: int"), "{hover}");
    assert!(hover.contains("*for binding*"), "{hover}");
}

#[test]
fn hover_on_match_binding() {
    let src = "enum gameEvent {\n    Damage(int amount)\n}\n\nint apply(gameEvent e) {\n    match (e) {\n        Damage(int amount) => {\n            return amount\n        }\n    }\n    return 0\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "return amount")).expect("binding resolves");
    assert!(hover.contains("amount: int"), "{hover}");
    assert!(hover.contains("*match binding*"), "{hover}");
}

#[test]
fn hover_on_function_shows_signature() {
    let src =
        "int add(int a, int b) {\n    return a + b\n}\n\nint main() {\n    return add(1, 2)\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, src.rfind("add(1, 2)").unwrap()).expect("add resolves");
    assert!(hover.contains("int add(int a, int b)"), "{hover}");
}

#[test]
fn hover_on_impl_member_shows_target_path() {
    let src = "struct vec2 {\n    float x\n    float y\n}\n\nimpl vec2 {\n    float dot(vec2 other) {\n        return 0.0\n    }\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "float dot(", "float ".len());
    let hover = hover_at(&a, offset).expect("dot resolves");
    assert!(hover.contains("impl vec2.dot"), "{hover}");
}

#[test]
fn hover_on_struct_declaration_name() {
    let src = "struct player {\n    str name\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "struct player", "struct ".len());
    let hover = hover_at(&a, offset).expect("player resolves");
    assert!(hover.contains("struct player"), "{hover}");
}

#[test]
fn hover_on_enum_declaration_name() {
    let src = "enum gameEvent {\n    Damage(int amount)\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "enum gameEvent", "enum ".len());
    let hover = hover_at(&a, offset).expect("gameEvent resolves");
    assert!(hover.contains("enum gameEvent"), "{hover}");
    assert!(hover.contains("Damage(amount: int)"), "{hover}");
}

#[test]
fn hover_on_struct_field_access_shows_field_type() {
    let src = "struct player {\n    int hp\n}\n\nint get(player p) {\n    return p.hp\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "p.hp")).expect("hp resolves");
    assert!(hover.contains("hp: int"), "{hover}");
    assert!(hover.contains("*field of* `player`"), "{hover}");
}

#[test]
fn hover_on_generic_field_access_substitutes() {
    let src = "struct pair<A, B> {\n    A first\n    B second\n}\n\nint main() {\n    pair<int, str> p = pair(first: 1, second: \"s\")\n    return p.first\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "p.first")).expect("first resolves");
    assert!(
        hover.contains("first: int"),
        "substituted type expected: {hover}"
    );
}

#[test]
fn hover_on_generic_field_declaration_keeps_parameter() {
    let src =
        "struct pair<A, B> {\n    A first\n    B second\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "A first", "A ".len());
    let hover = hover_at(&a, offset).expect("first resolves");
    assert!(hover.contains("first: A"), "{hover}");
}

#[test]
fn hover_on_qualified_variant() {
    let src = "enum gameEvent {\n    Spawn(str enemyKind)\n}\n\nint main() {\n    gameEvent e = gameEvent.Spawn(\"z\")\n    return 0\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "gameEvent.Spawn")).expect("Spawn resolves");
    assert!(hover.contains("gameEvent.Spawn(enemyKind: str)"), "{hover}");
}

#[test]
fn hover_on_import_segment() {
    let src = "import engine.graphics\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "engine.graphics", "engine.".len()) + 1;
    let hover = hover_at(&a, offset).expect("segment resolves");
    assert!(hover.contains("import engine.graphics"), "{hover}");
    assert!(hover.contains("path segment 1"), "{hover}");
}

#[test]
fn hover_on_builtin_type_keywords() {
    for (keyword, expected) in [
        ("int", "signed 64-bit integer"),
        ("float", "IEEE 754"),
        ("bool", "true"),
        ("str", "UTF-8"),
        ("void", "no value"),
        ("map", "map<K, V>"),
    ] {
        let src = format!("int main() {{\n    {keyword} x\n    return 0\n}}\n");
        // `void x` does not parse-check but the tolerant parse still lexes.
        let a = analysis(&src);
        let offset = offset_of(&src, keyword, keyword.len() - 1);
        let hover = hover_at(&a, offset).unwrap_or_else(|| panic!("{keyword} hover"));
        assert!(hover.contains(expected), "{keyword}: {hover}");
    }
}

#[test]
fn hover_on_builtin_constructors() {
    let src = "int main() {\n    option<int> m = Some(1)\n    return 0\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, src.rfind("Some").unwrap()).expect("Some resolves");
    assert!(hover.contains("Some"), "{hover}");
    assert!(hover.contains("option<T>"), "{hover}");
}

#[test]
fn hover_on_array_length() {
    let src = "int size(int[] xs) {\n    return xs.length\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "xs.length")).expect("length resolves");
    assert!(hover.contains("array.length"), "{hover}");
    assert!(hover.contains("int"), "{hover}");
}

#[test]
fn hover_infer_crystallizes_from_struct_construction() {
    let src = "struct vec2 {\n    float x\n}\n\nint main() {\n    infer v = vec2(x: 1.0)\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "infer v", "infer ".len());
    let hover = hover_at(&a, offset).expect("v resolves");
    assert!(hover.contains("v: vec2"), "{hover}");
}

#[test]
fn hover_infer_ambiguous_stays_infer() {
    let src = "int main() {\n    infer items = []\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "infer items", "infer ".len());
    let hover = hover_at(&a, offset).expect("items resolves");
    assert!(hover.contains("items: infer"), "{hover}");
}

#[test]
fn hover_on_unresolvable_identifier_is_none() {
    let src = "int main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "return 0", "return ".len());
    assert!(hover_at(&a, offset).is_none());
}

// ---------------------------------------------------------------------------
// Hover — block scoping (the checker scopes locals per block)
// ---------------------------------------------------------------------------

#[test]
fn if_local_is_invisible_after_the_branch() {
    let src = "int main() {\n    if (true) {\n        int inner = 1\n    }\n    return inner\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "return inner"));
    assert!(
        hover.is_none(),
        "if-local leaked out of its block: {hover:?}"
    );
}

#[test]
fn if_local_is_visible_inside_the_branch() {
    let src = "int main() {\n    if (true) {\n        int inner = 1\n        return inner\n    }\n    return 0\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "return inner")).expect("inner resolves");
    assert!(hover.contains("inner: int"), "{hover}");
}

#[test]
fn for_binding_is_invisible_after_the_loop() {
    let src = "int total(int[] xs) {\n    for (int x in xs) {\n        return x\n    }\n    return x\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let second_use = src.rfind("return x").unwrap() + "return ".len();
    assert!(
        hover_at(&a, second_use).is_none(),
        "binding leaked out of the loop"
    );
}

#[test]
fn while_local_is_invisible_after_the_loop() {
    let src =
        "int main() {\n    while (false) {\n        int inner = 1\n    }\n    return inner\n}\n";
    let a = analysis(src);
    assert!(hover_at(&a, last_offset_of(src, "return inner")).is_none());
}

#[test]
fn params_span_the_whole_body() {
    let src = "int heal(int amount) {\n    if (amount > 0) {\n        amount = amount - 1\n    }\n    return amount\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let hover = hover_at(&a, last_offset_of(src, "return amount")).expect("param visible");
    assert!(hover.contains("amount: int"), "{hover}");
}

// ---------------------------------------------------------------------------
// Hover — the enum symbol table (regression)
// ---------------------------------------------------------------------------

#[test]
fn enum_payload_identifiers_are_not_variants() {
    let src = "enum gameEvent {\n    Damage(int amount)\n    Spawn(str enemyKind, vec2 position)\n    PlayerDied()\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let event = a
        .enums
        .iter()
        .find(|e| e.name == "gameEvent")
        .expect("enum");
    let names: Vec<&str> = event.variants.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, vec!["Damage", "Spawn", "PlayerDied"], "{names:?}");
}

#[test]
fn enum_variant_payloads_are_aligned() {
    let src = "enum gameEvent {\n    Spawn(str enemyKind, vec2 position)\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let event = a
        .enums
        .iter()
        .find(|e| e.name == "gameEvent")
        .expect("enum");
    let spawn = event
        .variants
        .iter()
        .find(|v| v.name == "Spawn")
        .expect("Spawn");
    assert_eq!(spawn.fields.len(), 2, "{:?}", spawn.fields);
    assert_eq!(spawn.fields[0].0, "enemyKind");
    assert_eq!(spawn.fields[1].0, "position");
}

#[test]
fn hover_on_payload_identifier_does_not_claim_a_variant() {
    let src = "enum gameEvent {\n    Damage(int amount)\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    // The `amount` inside the payload is not a resolvable symbol.
    let offset = offset_of(src, "int amount)", "int ".len());
    assert!(
        hover_at(&a, offset).is_none(),
        "payload ident resolved as a variant"
    );
}

// ---------------------------------------------------------------------------
// Go-to-definition
// ---------------------------------------------------------------------------

#[test]
fn definition_of_local_jumps_to_its_declaration() {
    let src = "int main() {\n    int hp = 100\n    return hp\n}\n";
    let a = analysis(src);
    let span = definition_at(&a, last_offset_of(src, "return hp")).expect("hp resolves");
    let declared = offset_of(src, "int hp", "int ".len());
    assert_eq!(span.start, declared);
}

#[test]
fn definition_of_function_call_jumps_to_the_declaration() {
    let src =
        "int add(int a, int b) {\n    return a + b\n}\n\nint main() {\n    return add(1, 2)\n}\n";
    let a = analysis(src);
    let span = definition_at(&a, src.rfind("add").unwrap()).expect("add resolves");
    let declared = offset_of(src, "int add", "int ".len());
    assert_eq!(span.start, declared);
}

#[test]
fn definition_of_struct_use_jumps_to_the_declaration() {
    let src = "struct vec2 {\n    float x\n}\n\nint main() {\n    vec2 v = vec2(x: 1.0)\n    return 0\n}\n";
    let a = analysis(src);
    let span = definition_at(&a, src.rfind("vec2(").unwrap()).expect("vec2 resolves");
    let declared = offset_of(src, "struct vec2", "struct ".len());
    assert_eq!(span.start, declared);
}

#[test]
fn definition_of_field_access_jumps_to_the_field() {
    let src = "struct player {\n    int hp\n}\n\nint get(player p) {\n    return p.hp\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let span = definition_at(&a, last_offset_of(src, "p.hp")).expect("hp resolves");
    let declared = offset_of(src, "int hp", "int ".len());
    assert_eq!(span.start, declared);
}

#[test]
fn definition_of_qualified_variant_jumps_to_the_variant() {
    let src = "enum gameEvent {\n    Damage(int amount)\n}\n\nint main() {\n    gameEvent e = gameEvent.Damage(1)\n    return 0\n}\n";
    let a = analysis(src);
    let span = definition_at(
        &a,
        src.rfind("gameEvent.Damage").unwrap() + "gameEvent.".len(),
    )
    .expect("Damage resolves");
    let declared = offset_of(src, "Damage(", 0);
    assert_eq!(span.start, declared);
}

#[test]
fn definition_of_impl_member_jumps_to_the_member() {
    let src = "struct vec2 {\n    float x\n}\n\nimpl vec2 {\n    float dot(vec2 other) {\n        return 0.0\n    }\n}\n\nint main() {\n    vec2 v = vec2(x: 1.0)\n    return vec2.dot(v)\n}\n";
    let a = analysis(src);
    let span =
        definition_at(&a, src.rfind("vec2.dot(v)").unwrap() + "vec2.".len()).expect("dot resolves");
    let declared = offset_of(src, "float dot(", "float ".len());
    assert_eq!(span.start, declared);
}

#[test]
fn definition_of_import_segment_jumps_to_itself() {
    let src = "import engine.graphics\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let segment = offset_of(src, "engine.graphics", "engine.".len()) + 1;
    let span = definition_at(&a, segment).expect("segment resolves");
    assert_eq!(
        span.start,
        segment - 1,
        "jumps to the segment's own name token"
    );
}

#[test]
fn definition_of_builtins_is_none() {
    let src = "int main() {\n    option<int> m = Some(1)\n    return 0\n}\n";
    let a = analysis(src);
    assert!(definition_at(&a, src.rfind("Some").unwrap()).is_none());
    let keyword_offset = offset_of(src, "option<int>", "option".len() - 1);
    assert!(definition_at(&a, keyword_offset).is_none());
}

#[test]
fn definition_of_array_length_is_none() {
    let src = "int size(int[] xs) {\n    return xs.length\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    assert!(definition_at(&a, last_offset_of(src, "xs.length")).is_none());
}

// ---------------------------------------------------------------------------
// Find references
// ---------------------------------------------------------------------------

#[test]
fn references_include_declaration_and_uses() {
    let src = "int twice(int v) {\n    return v + v\n}\n\nint main() {\n    return twice(2)\n}\n";
    let a = analysis(src);
    let refs = references_at(&a, src.rfind("twice").unwrap(), true);
    assert_eq!(refs.len(), 2, "declaration plus call site");
    let without = references_at(&a, src.rfind("twice").unwrap(), false);
    assert_eq!(without.len(), 1, "call site only");
}

#[test]
fn references_respect_function_scoping_for_locals() {
    let src = "int twice(int v) {\n    return v + v\n}\n\nint main() {\n    int v = 3\n    return twice(v)\n}\n";
    let a = analysis(src);
    let param_use = offset_of(src, "return v + v", "return ".len());
    let param_refs = references_at(&a, param_use, true);
    assert_eq!(param_refs.len(), 3, "param declaration plus two body uses");
    let local_use = offset_of(src, "return twice(v)", "return twice(".len());
    let local_refs = references_at(&a, local_use, true);
    assert_eq!(local_refs.len(), 2, "local declaration plus one use");
}

#[test]
fn references_do_not_cross_blocks_into_dead_scopes() {
    let src = "int main() {\n    if (true) {\n        int inner = 1\n        return inner\n    }\n    return inner\n}\n";
    let a = analysis(src);
    let inside = src.find("return inner").unwrap() + "return ".len();
    let refs = references_at(&a, inside, true);
    assert_eq!(refs.len(), 2, "declaration plus the in-block use");
}

#[test]
fn references_to_a_struct_field_group_by_struct() {
    let src = "struct player {\n    int hp\n}\nstruct enemy {\n    int hp\n}\n\nint a(player p) {\n    return p.hp\n}\n\nint b(enemy e) {\n    return e.hp\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let player_hp = offset_of(src, "p.hp", "p.".len());
    let refs = references_at(&a, player_hp, true);
    assert_eq!(
        refs.len(),
        2,
        "player.hp declaration plus one use: {refs:?}"
    );
}

#[test]
fn references_to_qualified_variants() {
    let src = "enum gameEvent {\n    Damage(int amount)\n}\n\nint pick(gameEvent e) {\n    match (e) {\n        Damage(int amount) => return amount\n    }\n}\n\nint main() {\n    gameEvent e = gameEvent.Damage(1)\n    return pick(e)\n}\n";
    let a = analysis(src);
    let use_site = src.rfind("gameEvent.Damage(1)").unwrap() + "gameEvent.".len();
    let refs = references_at(&a, use_site, true);
    // Declaration + the bare name in the match pattern + the qualified use.
    assert_eq!(refs.len(), 3, "{refs:?}");
}

#[test]
fn references_of_an_unresolvable_name_are_empty() {
    let src = "int main() {\n    return 0\n}\n";
    let a = analysis(src);
    let offset = offset_of(src, "return 0", "return ".len());
    assert!(references_at(&a, offset, true).is_empty());
}

#[test]
fn references_span_across_functions_for_top_level_names() {
    let src = "struct vec2 {\n    float x\n}\n\nfloat abs(vec2 v) {\n    return v.x\n}\n\nfloat abs2(vec2 v) {\n    return v.x\n}\n\nint main() {\n    return 0\n}\n";
    let a = analysis(src);
    let first_x = offset_of(src, "v.x", "v.".len());
    let refs = references_at(&a, first_x, true);
    assert_eq!(
        refs.len(),
        3,
        "field declaration plus two accesses: {refs:?}"
    );
}
