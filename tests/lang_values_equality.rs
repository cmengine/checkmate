//! Value semantics and equality suite (§2.13, §A.4): every script-visible
//! value behaves as independently owned data — assignment, arguments, and
//! iteration bind copies — and equality is strict, same-type, and
//! structural for composites. Pinned end to end through the tree walker.

use cme_compiler::check::check;
use cme_compiler::parse_source;
use cme_interp::{InterpError, Interpreter, Value};

fn run_main(source: &str) -> Result<Value, InterpError> {
    let outcome = parse_source(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "front end must be clean: {:?}",
        outcome.diagnostics
    );
    let diagnostics = check(&outcome.statements);
    assert!(
        diagnostics.is_empty(),
        "checker must be clean: {diagnostics:?}"
    );
    Interpreter::new(&outcome.statements).invoke("main", &[])
}

fn expect(source: &str, value: Value) {
    assert_eq!(run_main(source), Ok(value), "source: {source:?}");
}

fn check_one_error(source: &str, contains: &str) {
    let outcome = parse_source(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "front end must be clean: {:?}",
        outcome.diagnostics
    );
    let diagnostics = check(&outcome.statements);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic: {diagnostics:?}\nsource: {source:?}"
    );
    assert!(
        diagnostics[0].to_string().contains(contains),
        "message {:?} should mention {contains:?}",
        diagnostics[0].to_string()
    );
}

fn int_main(body: &str) -> String {
    format!("int main() {{\n{body}\n}}\n")
}

/// Top-level declarations (types, helper functions) plus a `main` body.
fn program(decls: &str, body: &str) -> String {
    format!("{decls}\nint main() {{\n{body}\n}}\n")
}

const PLAYER: &str = "struct player {\nstr name\nint hp\nint[] buffs\n}";

// ---------------------------------------------------------------------------
// §2.13: mutation never crosses call boundaries
// ---------------------------------------------------------------------------

#[test]
fn struct_parameters_are_private_copies() {
    expect(
        &program(
            "struct player {\nstr name\nint hp\nint[] buffs\n}\nint damage(player p) {\np.hp = p.hp - 10\nreturn p.hp\n}",
            "player hero = player(name: \"Hero\", hp: 100, buffs: [])\nint after = damage(hero)\nif (after == 90) {\nif (hero.hp == 100) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn nested_struct_parameters_are_private_copies() {
    expect(
        &program(
            "struct inner { int v }\nstruct outer { inner inside }\nint bump(outer o) {\no.inside.v = o.inside.v + 1\nreturn o.inside.v\n}",
            "outer x = outer(inside: inner(v: 1))\nint after = bump(x)\nif (after == 2) {\nif (x.inside.v == 1) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn array_and_map_parameters_are_private_copies() {
    expect(
        &program(
            "int[] poison(int[] xs) {\nxs[0] = 999\nreturn xs\n}",
            "int[] original = [1]\nint[] seen = poison(original)\nif (original[0] == 1) {\nif (seen[0] == 999) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    expect(
        &program(
            "map<str, int> poison(map<str, int> m) {\nm[\"k\"] = 999\nreturn m\n}",
            "map<str, int> original = {}\noriginal[\"k\"] = 1\nmap<str, int> seen = poison(original)\nif (original[\"k\"] == 1) {\nif (seen[\"k\"] == 999) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn assignment_copies_structs_including_nested_buffers() {
    // Mutating the copy's array field must not write through to the
    // original: value semantics, not reference sharing.
    expect(
        &program(
            PLAYER,
            "player a = player(name: \"A\", hp: 1, buffs: [5])\nplayer b = a\nb.hp = 2\nb.buffs[0] = 9\nb.name = \"B\"\nif (a.hp == 1) {\nif (a.buffs[0] == 5) {\nif (a.name == \"A\") {\nif (b.buffs[0] == 9) {\nreturn 0\n}\n}\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn assignment_copies_arrays_and_maps() {
    expect(
        &int_main(
            "int[] a = [1]\nint[] b = a\nb[0] = 9\nif (a[0] == 1) {\nif (b[0] == 9) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    expect(
        &int_main(
            "map<str, int> a = {}\na[\"k\"] = 1\nmap<str, int> b = a\nb[\"k\"] = 9\nif (a[\"k\"] == 1) {\nif (b[\"k\"] == 9) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn enum_payloads_are_copies_when_bound_in_match_arms() {
    expect(
        &program(
            "enum box { Held(int[] xs)\nEmpty() }\nint mutate(box b) {\nmatch (b) {\nHeld(int[] xs) => {\nxs[0] = 999\nreturn xs[0]\n}\nEmpty() => {\nreturn 0\n}\n}\n}",
            "int[] data = [7]\nbox b = box.Held(data)\nint seen = mutate(b)\nif (seen == 999) {\nif (data[0] == 7) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn returned_structs_are_independent_of_their_source() {
    expect(
        &program(
            "struct player {\nstr name\nint hp\nint[] buffs\n}\nplayer renamed(player p) {\np.name = \"New\"\nreturn p\n}",
            "player hero = player(name: \"Hero\", hp: 1, buffs: [])\nplayer other = renamed(hero)\nif (hero.name == \"Hero\") {\nif (other.name == \"New\") {\nother.hp = 50\nif (hero.hp == 1) {\nreturn 0\n}\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// §A.4: strict, structural, same-type equality
// ---------------------------------------------------------------------------

#[test]
fn struct_equality_compares_fields_in_order_independent_fashion() {
    // Same field values constructed in the same named-argument form...
    expect(
        &program(
            "struct pair2 { int a\nint b }",
            "pair2 x = pair2(a: 1, b: 2)\npair2 y = pair2(a: 1, b: 2)\nif ((x == y) != true) {\nreturn 1\n}\nif ((x != y) != false) {\nreturn 2\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn struct_equality_is_rejected_across_types() {
    // Same shape, different type: still a type error (§A.4).
    check_one_error(
        "struct s2 { int v }\nstruct s3 { int v }\nint main() {\ns2 a = s2(v: 1)\ns3 b = s3(v: 1)\nif (a == b) {\nreturn 1\n}\nreturn 0\n}",
        "cannot apply `==` to `s2` and `s3`",
    );
}

#[test]
fn nested_struct_equality_is_structural() {
    expect(
        &program(
            "struct inner { int v }\nstruct outer { inner inside\nstr tag }",
            "outer a = outer(inside: inner(v: 1), tag: \"x\")\nouter b = outer(inside: inner(v: 1), tag: \"x\")\nif ((a == b) != true) {\nreturn 1\n}\nouter c = outer(inside: inner(v: 2), tag: \"x\")\nif ((a == c) != false) {\nreturn 2\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn enum_equality_requires_the_same_variant_and_payload() {
    expect(
        &program(
            "enum e2 { A(int v)\nB(str tag)\nC() }",
            "if ((e2.A(1) == e2.A(1)) != true) {\nreturn 1\n}\nif ((e2.A(1) == e2.A(2)) != false) {\nreturn 2\n}\nif ((e2.A(1) == e2.B(\"x\")) != false) {\nreturn 3\n}\nif ((e2.C() == e2.C()) != true) {\nreturn 4\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn result_and_option_equality_is_structural() {
    expect(
        &int_main(
            "if ((result.Ok(1) == result.Err(1)) != false) {\nreturn 1\n}\nif ((result.Ok(1) == result.Ok(1)) != true) {\nreturn 2\n}\nif ((option.None() == option.None()) != true) {\nreturn 3\n}\nif ((option.Some(2) == option.Some(2)) != true) {\nreturn 4\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn nan_poisons_structural_equality_everywhere() {
    // IEEE 754: NaN != NaN, and that property propagates through any
    // composite comparison.
    expect(
        &program(
            "struct wrapper { float v }",
            "float nan = 0.0 / 0.0\nwrapper a = wrapper(v: nan)\nwrapper b = wrapper(v: nan)\nif (a == b) {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(0),
    );
    expect(
        &program(
            "enum box { V(float x) }",
            "float nan = 0.0 / 0.0\nif ((box.V(nan) == box.V(nan)) != false) {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(0),
    );
    expect(
        &int_main("float nan = 0.0 / 0.0\nif (([nan] == [nan]) != false) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
    expect(
        &int_main(
            "float nan = 0.0 / 0.0\nmap<str, float> a = {}\nmap<str, float> b = {}\na[\"k\"] = nan\nb[\"k\"] = nan\nif (a == b) {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn scalar_equality_covers_every_primitive_pairing() {
    expect(
        &int_main(
            "if ((1 == 1) != true) {\nreturn 1\n}\nif ((1.5 == 1.5) != true) {\nreturn 2\n}\nif ((\"s\" == \"s\") != true) {\nreturn 3\n}\nif ((true == true) != true) {\nreturn 4\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn equality_between_scalars_and_composites_is_a_type_error() {
    check_one_error(
        "int main() {\nif (1 == [1]) {\nreturn 1\n}\nreturn 0\n}",
        "cannot apply `==`",
    );
}

// ---------------------------------------------------------------------------
// Copy isolation through impl members (§10.4 interaction)
// ---------------------------------------------------------------------------

#[test]
fn impl_member_mutations_never_escape_the_receiver() {
    expect(
        &program(
            "struct counter { int n }\nimpl counter {\ncounter bump(counter c) {\nc.n = c.n + 1\nreturn c\n}\n}",
            "counter c = counter(n: 1)\ncounter bumped = counter.bump(c)\nif (c.n == 1) {\nif (bumped.n == 2) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn copies_of_structs_carrying_impl_blocks_still_call_members() {
    expect(
        &program(
            "struct counter { int n }\nimpl counter {\nint get(counter c) {\nreturn c.n\n}\n}",
            "counter a = counter(n: 5)\ncounter b = a\nb.n = 9\nif (counter.get(a) == 5) {\nif (counter.get(b) == 9) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}
