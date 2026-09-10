//! Runtime stress suite: the tree-walking interpreter's observable
//! semantics — Appendix A evaluation, overflow/termination, short-circuit
//! order, value semantics, and structural equality — every value pinned.

use cme_compiler::check::check;
use cme_interp::{InterpError, Interpreter, Value};

fn run_main(source: &str) -> Result<Value, InterpError> {
    let outcome = cme_compiler::parse_source(source);
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

fn expect_err(source: &str, message_contains: &str) {
    let error = run_main(source).expect_err("expected a runtime error");
    assert!(
        error.message.contains(message_contains),
        "error {:?} should mention {message_contains:?}",
        error.message
    );
}

fn int_main(body: &str) -> String {
    format!("int main() {{\n{body}\n}}\n")
}

/// Top-level declarations (structs, functions) plus a `main` body.
fn program(prelude: &str, body: &str) -> String {
    format!("{prelude}\nint main() {{\n{body}\n}}\n")
}

// ---------------------------------------------------------------------------
// Arithmetic edges (§2.4, §A.5)
// ---------------------------------------------------------------------------

#[test]
fn integer_overflow_terminates_the_invocation() {
    expect_err(
        &int_main("int big = 9223372036854775807\nreturn big + 1"),
        "overflow",
    );
    expect_err(
        &int_main("int small = 0 - 9223372036854775807\nreturn small - 2"),
        "overflow",
    );
    expect_err(
        &int_main("int a = 9223372036854775807\nreturn a * 2"),
        "overflow",
    );
    expect_err(&int_main("return -(9223372036854775807) - 2"), "overflow");
    // Multiplication overflow via small factors.
    expect_err(&int_main("return 3037000500 * 3037000500"), "overflow");
}

#[test]
fn division_and_remainder_by_zero_terminate() {
    expect_err(&int_main("return 1 / 0"), "division by zero");
    expect_err(&int_main("return 1 % 0"), "remainder by zero");
    expect_err(&int_main("int z = 0\nreturn 5 / z"), "division by zero");
    expect_err(&int_main("int z = 0\nreturn 5 % z"), "remainder by zero");
    // The one overflowing division: i64::MIN / -1.
    expect_err(
        &int_main("return (0 - 9223372036854775807 - 1) / -1"),
        "overflow",
    );
}

#[test]
fn float_edges_follow_ieee_754() {
    // Division by zero is inf, not an error (inf has the fixed-point
    // property: adding 1 changes nothing).
    expect(
        &int_main("float v = 1.0 / 0.0\nif (v + 1.0 == v) { return 0 }\nreturn 1"),
        Value::Int(0),
    );
    // NaN never equals itself.
    expect(
        &int_main("float nan = 0.0 / 0.0\nif (nan == nan) { return 1 }\nreturn 0"),
        Value::Int(0),
    );
    // NaN comparisons are all false.
    expect(
        &int_main("float nan = 0.0 / 0.0\nif (nan < nan || nan > nan) { return 1 }\nreturn 0"),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Short-circuiting order (§A.5)
// ---------------------------------------------------------------------------

#[test]
fn logical_operators_short_circuit() {
    // The right operand must not run when the left decides.
    expect(
        &program(
            "bool boom() {\nint x = 1 / 0\nreturn true\n}",
            "if (false && boom()) { return 1 }\nreturn 0",
        ),
        Value::Int(0),
    );
    expect(
        &program(
            "bool boom() {\nint x = 1 / 0\nreturn true\n}",
            "if (true || boom()) { return 0 }\nreturn 1",
        ),
        Value::Int(0),
    );
    // Side effects do not exist across calls (value semantics), so the
    // observable proof of short-circuiting is the ERROR that never happens:
    // `boom()` divides by zero, and the run above returns cleanly only
    // because the right operand was never evaluated.
}

#[test]
fn evaluation_order_is_left_to_right_through_calls() {
    // With no global state, the observable order is the VALUE order: a
    // left-to-right evaluation concatenates <a> before <b>.
    expect(
        &program(
            "str mark(str m) {\nreturn \"<\" + m + \">\"\n}",
            "str t = mark(\"a\") + mark(\"b\") + mark(\"c\")\nif (t != \"<a><b><c>\") { return 1 }\nreturn 0",
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Stringification (§A.6)
// ---------------------------------------------------------------------------

#[test]
fn stringification_canonical_forms() {
    expect(
        &int_main(
            r#"str s = "v" + 0
if (s != "v0") { return 1 }
return 0"#,
        ),
        Value::Int(0),
    );
    expect(
        &int_main(
            r#"str s = "v" + -123
if (s != "v-123") { return 1 }
return 0"#,
        ),
        Value::Int(0),
    );
    expect(
        &int_main(
            r#"str s = "v" + 0.5
if (s != "v0.5") { return 1 }
return 0"#,
        ),
        Value::Int(0),
    );
    expect(
        &int_main(
            r#"str s = "v" + 100000000000.0
if (s != "v100000000000") { return 1 }
return 0"#,
        ),
        Value::Int(0),
    );
    // Booleans stringify lowercase.
    expect(
        &int_main(
            r#"str s = "v" + true + false
if (s != "vtruefalse") { return 1 }
return 0"#,
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Compound assignment (§A.7)
// ---------------------------------------------------------------------------

#[test]
fn compound_assignment_through_paths() {
    expect(
        &int_main(
            "int[] a = [1, 2]\na[0] += 10\na[1] *= 3\nif (a[0] != 11 || a[1] != 6) { return 1 }\nreturn 0",
        ),
        Value::Int(0),
    );
    expect(
        &int_main(
            "map<str, int> m = { \"k\": 5 }\nm[\"k\"] -= 2\nif (m[\"k\"] != 3) { return 1 }\nreturn 0",
        ),
        Value::Int(0),
    );
    expect(
        &program(
            "struct s2 { int v }",
            "s2 x = s2(v: 4)\nx.v /= 2\nif (x.v != 2) { return 1 }\nreturn 0",
        ),
        Value::Int(0),
    );
    expect(
        &int_main("str s = \"a\"\ns += \"b\"\ns += 1\nif (s != \"ab1\") { return 1 }\nreturn 0"),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Call depth and recursion limits (§5.5)
// ---------------------------------------------------------------------------

#[test]
fn runaway_recursion_is_a_clean_error() {
    // A generous stack, so the interpreter's own DEPTH limit (the fixed
    // 1024) is what terminates the runaway recursion — the shape a
    // production host thread provides.
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let error = run_main(
                "int loop(int n) {\nreturn loop(n + 1)\n}\nint main() {\nreturn loop(0)\n}",
            )
            .expect_err("depth limit");
            error.message
        })
        .expect("spawn");
    let message = handle.join().expect("clean error, no panic");
    assert!(message.contains("call depth limit"), "got: {message:?}");
}

#[test]
fn deep_but_bounded_recursion_terminates() {
    // 400 nested calls terminate cleanly — run on a generous stack because
    // debug-build interpreter frames are wide (a production host gets the
    // same guarantee from optimized frames or its own §5.5 limits).
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            run_main(&program(
                "int count(int n) {\nif (n == 0) { return 0 }\nreturn 1 + count(n - 1)\n}",
                "return count(400)",
            ))
        })
        .expect("spawn");
    assert_eq!(handle.join().expect("clean run"), Ok(Value::Int(400)));
}

// ---------------------------------------------------------------------------
// Value semantics under mutation (§2.13)
// ---------------------------------------------------------------------------

#[test]
fn array_mutation_through_calls_is_isolated() {
    expect(
        &program(
            "int[] mutate(int[] xs) {\nxs[0] = 999\nreturn xs\n}",
            "int[] original = [1]\nint[] copy = mutate(original)\nif (original[0] != 1) { return 1 }\nif (copy[0] != 999) { return 2 }\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn map_mutation_is_isolated() {
    expect(
        &program(
            "map<str, int> mutate(map<str, int> m) {\nm[\"k\"] = 42\nreturn m\n}",
            "map<str, int> original = { \"k\": 1 }\nmap<str, int> copy = mutate(original)\nif (original[\"k\"] != 1) { return 1 }\nif (copy[\"k\"] != 42) { return 2 }\nreturn 0",
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Structural equality (§A.4)
// ---------------------------------------------------------------------------

#[test]
fn equality_is_structural_for_every_composite() {
    expect(
        &program(
            "struct s2 { int a\nint[] b }",
            "s2 x = s2(a: 1, b: [1, 2])\ns2 y = s2(a: 1, b: [1, 2])\nif ((x == y) != true) { return 1 }\ny.b[0] = 9\nif ((x == y) != false) { return 2 }\nreturn 0",
        ),
        Value::Int(0),
    );
    expect(
        &program(
            "enum e2 { A(int v)\nB() }",
            "if ((e2.A(1) == e2.A(1)) != true) { return 1 }\nif ((e2.A(1) == e2.B()) != false) { return 2 }\nif ((e2.B() == e2.B()) != true) { return 3 }\nreturn 0",
        ),
        Value::Int(0),
    );
    expect(
        &int_main(
            "if ((option.Some(1) == option.Some(1)) != true) { return 1 }\nif ((option.Some(1) == option.None()) != false) { return 2 }\nreturn 0",
        ),
        Value::Int(0),
    );
    // Cross-type equality is a TYPE error, not a runtime false.
    // (pinned in the checker suite)
}

// ---------------------------------------------------------------------------
// Runtime error spans and messages stay clean
// ---------------------------------------------------------------------------

#[test]
fn out_of_bounds_and_missing_keys_are_clean_errors() {
    expect_err(&int_main("int[] a = [1]\nreturn a[1]"), "out of bounds");
    expect_err(&int_main("int[] a = [1]\nreturn a[-1]"), "out of bounds");
    expect_err(
        &int_main("map<str, int> m = { \"a\": 1 }\nreturn m[\"b\"]"),
        "not found",
    );

    // The interp defends itself against unchecked trees with clean errors,
    // never panics — the ungated path is pinned in the crate's own tests.
}

// ---------------------------------------------------------------------------
// The ? operator end to end (§2.8)
// ---------------------------------------------------------------------------

#[test]
fn try_propagation_returns_early_through_frames() {
    expect(
        &program(
            "result<int, str> inner(int v) {\nif (v == 0) { return Err(\"zero\") }\nreturn Ok(v)\n}\nresult<int, str> outer(int v) {\nint x = inner(v)?\nreturn Ok(x * 2)\n}",
            "if (outer(0) != result.Err(\"zero\")) { return 1 }\nif (outer(3) != result.Ok(6)) { return 2 }\nreturn 0",
        ),
        Value::Int(0),
    );
    // ? inside a LOOP propagates out of the whole function.
    expect(
        &program(
            "result<int, str> inner(int v) {\nif (v == 3) { return Err(\"three\") }\nreturn Ok(v)\n}\nresult<int, str> scan() {\nfor (int i in [1, 2, 3, 4]) {\nint x = inner(i)?\n}\nreturn Ok(0)\n}",
            "if (scan() != result.Err(\"three\")) { return 1 }\nreturn 0",
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Map iteration order (insertion order, plan §1.4.10)
// ---------------------------------------------------------------------------

#[test]
fn map_iteration_yields_keys_in_insertion_order() {
    expect(
        &int_main(
            "map<int, str> m = {}\nm[3] = \"c\"\nm[1] = \"a\"\nm[2] = \"b\"\nm[1] = \"A\"\nstr out = \"\"\nfor (int k in m) {\nout += m[k]\n}\nif (out != \"cAb\") { return 1 }\nreturn 0",
        ),
        Value::Int(0),
    );
}
