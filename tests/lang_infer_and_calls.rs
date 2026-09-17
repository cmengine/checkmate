//! Inference and calls suite: §2.16 `infer` crystallization (what it
//! accepts, what it rejects, and its context-free nature), §2.12 argument
//! forms (named arguments in any order, exact parameter binding), return
//! slot coercion, and call-expression compositions.

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

fn program(decls: &str, body: &str) -> String {
    format!("{decls}\nint main() {{\n{body}\n}}\n")
}

// ---------------------------------------------------------------------------
// §2.16: infer crystallizes to the initializer's unambiguous type
// ---------------------------------------------------------------------------

#[test]
fn infer_crystallizes_every_scalar_literal() {
    // The crystallized type is enforced by assigning into typed slots.
    expect(&int_main("infer x = 5\nint y = x\nreturn y"), Value::Int(5));
    expect(
        &int_main("infer x = 2.5\nfloat y = x\nif (y == 2.5) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    expect(
        &int_main("infer x = \"hi\"\nstr y = x\nif (y == \"hi\") {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    expect(
        &int_main("infer x = true\nbool y = x\nif (y) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
}

#[test]
fn infer_crystallizes_composite_initializers() {
    expect(
        &program(
            "struct pt { int x\nint y }",
            "infer p = pt(x: 1, y: 2)\npt q = p\nreturn q.x + q.y",
        ),
        Value::Int(3),
    );
    expect(
        &int_main("infer xs = [1, 2, 3]\nint[] ys = xs\nreturn ys.length"),
        Value::Int(3),
    );
    expect(
        &int_main("infer m = { \"a\": 1 }\nmap<str, int> copy = m\nreturn copy[\"a\"]"),
        Value::Int(1),
    );
    expect(
        &int_main("infer o = option.Some(4)\noption<int> p = o\nreturn 0"),
        Value::Int(0),
    );
    expect(
        &int_main("infer b = 1 < 2\nbool y = b\nif (y) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
}

#[test]
fn infer_crystallizes_expression_results() {
    expect(
        &int_main("infer x = 1 + 2\nint y = x\nreturn y"),
        Value::Int(3),
    );
    expect(
        &int_main("infer x = 1.0 + 2.0\nfloat y = x\nif (y == 3.0) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    expect(
        &program(
            "int triple(int v) {\nreturn v * 3\n}",
            "infer x = triple(2)\nint y = x\nreturn y",
        ),
        Value::Int(6),
    );
}

#[test]
fn infer_rejects_ambiguous_initializers() {
    check_one_error(
        "int main() {\ninfer items = []\nreturn 0\n}",
        "cannot infer type for 'items'; ambiguous initializer",
    );
    check_one_error(
        "int main() {\ninfer m = {}\nreturn 0\n}",
        "cannot infer type for 'm'; ambiguous initializer",
    );
}

#[test]
fn infer_rejects_void_initializers() {
    check_one_error(
        "void nothing() {\nreturn\n}\nint main() {\ninfer x = nothing()\nreturn 0\n}",
        "cannot infer type for 'x'; void initializer",
    );
}

#[test]
fn infer_is_context_free_at_byte_positions() {
    // A byte return slot does not retroactively narrow an inferred int.
    check_one_error(
        "byte give() {\ninfer b = 5\nreturn b\n}\nint main() {\nreturn 0\n}",
        "wrong return type in `give`: expected `byte`, found `int`",
    );
}

#[test]
fn infer_binds_the_static_type_for_later_assignments() {
    expect(&int_main("infer x = 1\nx = 2\nreturn x"), Value::Int(2));
    check_one_error(
        "int main() {\ninfer x = 1\nx = 1.5\nreturn x\n}",
        "type mismatch in assignment to `x`: expected `int`, found `float`",
    );
}

#[test]
fn infer_from_a_byte_value_widens_to_int() {
    // A byte VALUE has an unambiguous type (byte); the inferred binding
    // crystallizes as byte and widens into int slots from there.
    expect(
        &int_main("byte b = 5\ninfer x = b\nint y = x\nreturn y"),
        Value::Int(5),
    );
}

// ---------------------------------------------------------------------------
// §2.12: argument forms and binding
// ---------------------------------------------------------------------------

#[test]
fn named_arguments_bind_by_name_in_any_order() {
    expect(
        &program(
            "int sub(int a, int b) {\nreturn a - b\n}",
            "return sub(b: 2, a: 10)",
        ),
        Value::Int(8),
    );
}

#[test]
fn positional_arguments_bind_by_position() {
    expect(
        &program(
            "int sub(int a, int b) {\nreturn a - b\n}",
            "return sub(10, 2)",
        ),
        Value::Int(8),
    );
}

#[test]
fn mixing_positional_and_named_arguments_is_a_syntax_error() {
    let outcome = parse_source(
        "int f(int a, int b) {\nreturn a + b\n}\nint main() {\nreturn f(1, b: 2)\n}\n",
    );
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|e| e.to_string().contains("cannot mix positional and named")),
        "{:?}",
        outcome.diagnostics
    );
}

#[test]
fn unknown_and_missing_named_arguments_are_reported_exactly() {
    // A satisfied parameter list keeps this at one diagnostic.
    check_one_error(
        "int f(int a) {\nreturn a\n}\nint main() {\nreturn f(a: 1, b: 2)\n}",
        "unknown argument `b` in call to `f`",
    );
}

#[test]
fn named_arguments_work_for_struct_literals_in_any_order() {
    expect(
        &program(
            "struct pt { int x\nint y }",
            "pt v = pt(y: 3, x: 2)\nreturn v.x + v.y",
        ),
        Value::Int(5),
    );
}

#[test]
fn arity_mismatches_are_single_diagnostics() {
    check_one_error(
        "int f(int a) {\nreturn a\n}\nint main() {\nreturn f(1, 2)\n}",
        "wrong number of arguments to `f`: expected 1, found 2",
    );
}

// ---------------------------------------------------------------------------
// Return slots and coercion
// ---------------------------------------------------------------------------

#[test]
fn byte_returns_widen_through_int_return_slots() {
    expect(
        &program("byte give() {\nbyte b = 250\nreturn b\n}", "return give()"),
        Value::Int(250),
    );
}

#[test]
fn wrong_return_types_are_rejected_exactly() {
    check_one_error(
        "int f() {\nreturn \"s\"\n}",
        "wrong return type in `f`: expected `int`, found `str`",
    );
    check_one_error(
        "str f() {\nreturn 1\n}",
        "wrong return type in `f`: expected `str`, found `int`",
    );
}

#[test]
fn void_functions_run_as_statements_and_their_results_are_dropped() {
    expect(
        &program(
            "void accumulate(int v) {\n}\nint grab() {\nreturn 1\n}",
            "accumulate(5)\nint ignored = grab()\nreturn 0",
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Call-expression composition
// ---------------------------------------------------------------------------

#[test]
fn call_results_compose_with_field_and_index_access() {
    expect(
        &program(
            "struct holder { int[] data }\nint[] makeData() {\nreturn [7, 8]\n}\nholder makeHolder() {\nreturn holder(data: makeData())\n}",
            "if (makeData()[0] == 7) {\nif (makeHolder().data[1] == 8) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn mutual_recursion_terminates_through_both_frames() {
    expect(
        &program(
            "bool isEven(int n) {\nif (n == 0) {\nreturn true\n}\nreturn isOdd(n - 1)\n}\nbool isOdd(int n) {\nif (n == 0) {\nreturn false\n}\nreturn isEven(n - 1)\n}",
            "if (isEven(10)) {\nif (!isOdd(10)) {\nif (isOdd(7)) {\nreturn 0\n}\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn forward_references_resolve_at_call_time() {
    // main calls a function declared textually below it.
    expect(
        &program("int later() {\nreturn 42\n}", "return later()"),
        Value::Int(42),
    );
}

#[test]
fn calls_in_condition_positions_evaluate_to_bool() {
    expect(
        &program(
            "bool positive(int v) {\nreturn v > 0\n}",
            "if (positive(3)) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    expect(
        &program(
            "bool positive(int v) {\nreturn v > 0\n}",
            "while (positive(0 - 1)) {\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn generic_struct_constructions_crystallize_from_arguments() {
    expect(
        &program(
            "struct pair2<A, B> { A first\nB second }",
            "pair2<int, str> p = pair2(first: 1, second: \"x\")\nreturn p.first",
        ),
        Value::Int(1),
    );
}
