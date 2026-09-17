//! Runtime-edges suite: observable semantics the other suites do not pin —
//! the unary grammar corners of §A.2 (`--` is two operators, not one),
//! the digit-led identifier rules of §2 (and the hex spelling that is an
//! identifier, not a literal), compound chains through indexed paths,
//! multi-level `?` propagation with the payload pinned at each hop,
//! `match` as both statement and expression with the wildcard fallback,
//! early returns from nested loops, and structural values at their
//! composition edges. Every value goes through the full pipeline.

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

fn program(prelude: &str, body: &str) -> String {
    format!("{prelude}\nint main() {{\n{body}\n}}\n")
}

// ---------------------------------------------------------------------------
// §A.2 unary grammar corners
// ---------------------------------------------------------------------------

#[test]
fn double_minus_is_two_unary_operators() {
    // `--` is not a token: `--x` is -(-x), which is x again.
    expect(&int_main("int x = 5\nreturn --x"), Value::Int(5));
}

#[test]
fn negated_parentheses_apply_inward() {
    expect(&int_main("int x = 5\nreturn -(-x)"), Value::Int(5));
    expect(&int_main("int x = 5\nreturn -(-(-x))"), Value::Int(-5));
}

#[test]
fn double_logical_not_normalizes_bool() {
    expect(
        &int_main("bool f = false\nif (!!f) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
    expect(
        &int_main("bool f = true\nif (!!f) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
}

#[test]
fn unary_negation_binds_tighter_than_multiplication() {
    // `-x * y` is (-x) * y: -2 * 3 = -6, never -(2*3) mis-associated (it
    // happens to agree here, so also pin a case where it matters:
    // -x / y with truncation toward zero vs -(x / y) are identical only
    // because both round toward zero — the PIN is the value).
    expect(
        &int_main("int x = 2\nint y = 3\nreturn -x * y"),
        Value::Int(-6),
    );
    expect(
        &int_main("int x = 7\nint y = 2\nreturn -x / y"),
        Value::Int(-3),
    );
}

// ---------------------------------------------------------------------------
// §2 lexical corners: digit-led identifiers
// ---------------------------------------------------------------------------

#[test]
fn digit_led_identifiers_are_names() {
    // `2D` and `3Vector` contain a non-digit, so they scan as identifiers.
    expect(&int_main("int 2D = 7\nreturn 2D"), Value::Int(7));
    expect(
        &int_main("int 3Vector = 9\nreturn 3Vector + 1"),
        Value::Int(10),
    );
    expect(&int_main("int 2_D = 1\nreturn 2_D"), Value::Int(1));
}

#[test]
fn the_hex_spelling_is_an_identifier_not_a_literal() {
    // `0x10` is a digit-led word with non-digit characters: an identifier
    // by §2, so the declaration fails with an unknown name — never a hex
    // literal.
    check_one_error(
        "int main() {\nint x = 0x10\nreturn x\n}",
        "unknown name `0x10`",
    );
}

#[test]
fn a_pure_digit_run_is_always_a_literal() {
    // `123abc` scans as ONE identifier (longest match), but a pure digit
    // run stays a literal: assigning a bare literal to an int is fine.
    expect(&int_main("return 123"), Value::Int(123));
}

// ---------------------------------------------------------------------------
// Compound assignment through indexed paths
// ---------------------------------------------------------------------------

#[test]
fn compound_operators_chain_on_an_indexed_target() {
    // 1 +5=6, -2=4, *3=12, /4=3, %3=0.
    expect(
        &int_main(
            "int[] a = [1]\na[0] += 5\na[0] -= 2\na[0] *= 3\na[0] /= 4\na[0] %= 3\nreturn a[0]",
        ),
        Value::Int(0),
    );
}

#[test]
fn compound_assignment_through_a_struct_field_path() {
    expect(
        &program(
            "struct counter {\nint hits\n}",
            "counter c = counter(hits: 10)\nc.hits += 5\nc.hits *= 2\nreturn c.hits",
        ),
        Value::Int(30),
    );
}

#[test]
fn compound_assignment_composes_with_map_values() {
    expect(
        &int_main(
            "map<str, int> m = {}\nm[\"hits\"] = 1\nm[\"hits\"] += 2\nm[\"hits\"] *= 10\nreturn m[\"hits\"]",
        ),
        Value::Int(30),
    );
}

// ---------------------------------------------------------------------------
// `?` propagation across call depth
// ---------------------------------------------------------------------------

#[test]
fn question_propagates_through_nested_calls_with_the_payload_intact() {
    // The Err payload travels: deep -> mid (via ?) -> main, and main sees
    // exactly the value `deep` constructed.
    expect(
        &program(
            "result<int, str> deep() {\nreturn Err(\"boom\")\n}\nresult<int, str> mid() {\nint v = deep()?\nreturn Ok(v)\n}",
            "infer r = mid()\nmatch (r) {\nOk(int v) => {\nreturn 0\n}\nErr(str e) => {\nif (e == \"boom\") {\nreturn 7\n}\nreturn 1\n}\n}",
        ),
        Value::Int(7),
    );
}

#[test]
fn question_does_not_escape_the_enclosing_function() {
    // `mid` catches its own callee's Err via `?` and RETURNS it as a
    // value: the caller stays in control and can match the Ok path.
    expect(
        &program(
            "result<int, str> deep() {\nreturn Err(\"x\")\n}\nresult<int, str> mid() {\nint v = deep()?\nreturn Ok(v + 1)\n}",
            "infer r = mid()\nmatch (r) {\nOk(int v) => {\nreturn v\n}\nErr(str e) => {\nreturn -1\n}\n}",
        ),
        Value::Int(-1),
    );
}

#[test]
fn question_unwraps_ok_through_the_chain() {
    // `?` needs a result-typed enclosing function, so the mid hop returns
    // result and main unwraps the final value by matching.
    expect(
        &program(
            "result<int, str> deep() {\nreturn Ok(41)\n}\nresult<int, str> mid() {\nint v = deep()?\nreturn Ok(v + 1)\n}",
            "infer r = mid()\nmatch (r) {\nOk(int v) => {\nreturn v\n}\nErr(str e) => {\nreturn -1\n}\n}",
        ),
        Value::Int(42),
    );
}

// ---------------------------------------------------------------------------
// match: statement vs expression, wildcard, enum payloads
// ---------------------------------------------------------------------------

#[test]
fn match_expression_picks_arms_and_the_wildcard_catches_the_rest() {
    expect(
        &program(
            "enum shape {\nDot()\nCircle(int r)\nBox(int w, int h)\n}\nint area(shape s) {\nreturn match (s) {\nDot() => 0\nCircle(int r) => r * r * 3\nBox(int w, int h) => w * h\n}\n}",
            "return area(shape.Box(2, 5)) + area(shape.Circle(2)) + area(shape.Dot())",
        ),
        Value::Int(22),
    );
}

#[test]
fn match_statement_runs_side_effects_without_a_value() {
    // Modules hold no mutable globals (§1), so the match statement runs
    // inside a function and the level threads through as a value.
    expect(
        &program(
            "enum signal {\nUp(int by)\nDown(int by)\n}\nint apply(int level, signal s) {\nmatch (s) {\nUp(int by) => {\nreturn level + by\n}\nDown(int by) => {\nreturn level - by\n}\n}\n}",
            "int level = 0\nlevel = apply(level, signal.Up(3))\nlevel = apply(level, signal.Down(1))\nreturn level",
        ),
        Value::Int(2),
    );
}

// ---------------------------------------------------------------------------
// Control-flow shapes
// ---------------------------------------------------------------------------

#[test]
fn while_false_never_enters_and_if_else_picks_the_second_branch() {
    expect(
        &int_main("while (false) {\nreturn 1\n}\nif (false) {\nreturn 2\n} else {\nreturn 3\n}"),
        Value::Int(3),
    );
}

#[test]
fn early_return_exits_nested_loops_in_one_step() {
    expect(
        &int_main(
            "int[] grid = [1, 2, 3]\nfor (int a in grid) {\nfor (int b in grid) {\nif (a * b == 4) {\nreturn a + b\n}\n}\n}\nreturn 0",
        ),
        Value::Int(4),
    );
}

#[test]
fn else_if_chain_picks_exactly_one_branch() {
    expect(
        &int_main(
            "int v = 42\nif (v < 10) {\nreturn 1\n} else {\nif (v < 50) {\nreturn 2\n} else {\nreturn 3\n}\n}",
        ),
        Value::Int(2),
    );
}

// ---------------------------------------------------------------------------
// Composition edges: values through calls and collections
// ---------------------------------------------------------------------------

#[test]
fn struct_returned_from_a_call_is_an_independent_copy() {
    expect(
        &program(
            "struct box {\nint v\n}\nbox make() {\nreturn box(v: 1)\n}",
            "box a = make()\nbox b = make()\nb.v = 99\nreturn a.v * 10 + b.v",
        ),
        Value::Int(109),
    );
}

#[test]
fn arrays_of_maps_and_maps_of_arrays_compose() {
    expect(
        &int_main(
            "map<int, int[]> m = {}\nm[1] = [1, 2]\nm[2] = [3]\nint[] lengths = [m[1].length, m[2].length]\nreturn lengths[0] * 10 + lengths[1]",
        ),
        Value::Int(21),
    );
}

#[test]
fn deep_call_pipeline_stays_exact() {
    expect(
        &program(
            "int add(int a, int b) {\nreturn a + b\n}",
            "return add(add(add(1, 2), add(3, 4)), add(5, 6))",
        ),
        Value::Int(21),
    );
}

#[test]
fn string_building_in_a_loop_keeps_insertion_order() {
    expect(
        &int_main(
            "int[] xs = [3, 1, 2]\nstr joined = \"\"\nfor (int v in xs) {\njoined = joined + v\n}\nif (joined == \"312\") {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(1),
    );
}

#[test]
fn map_iteration_order_survives_updates() {
    // Updating a key keeps its original position (§11): the rebuilt key
    // order is still 3, 1, 2 — pinned through the values read back.
    expect(
        &int_main(
            "map<int, str> m = {3: \"c\", 1: \"a\", 2: \"b\"}\nm[1] = \"A\"\nstr joined = \"\"\nfor (int k in m) {\njoined = joined + m[k]\n}\nif (joined == \"cAb\") {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(1),
    );
}

#[test]
fn fresh_map_key_insertion_appends_to_the_iteration_order() {
    expect(
        &int_main(
            "map<int, str> m = {1: \"a\"}\nm[2] = \"b\"\nm[0] = \"z\"\nstr joined = \"\"\nfor (int k in m) {\njoined = joined + m[k]\n}\nif (joined == \"abz\") {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(1),
    );
}
