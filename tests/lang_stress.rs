//! Adversarial language stress: the compiler's own budgets (nesting depth,
//! operator and type-node complexity), the owner-ruled shadowing ban on
//! every binding form, and runtime shapes near the edges of what the tree
//! walker must handle — deep collections, wide maps, long call pipelines,
//! and composed loops.

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

/// Runs `run_main` on a generous stack — debug-build interpreter frames are
/// wide, and deep user-function chains multiply them (see the same idiom in
/// `lang_runtime`).
fn expect_on_big_stack(source: &str, value: Value) {
    let owned = source.to_string();
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_main(&owned))
        .expect("spawn");
    assert_eq!(
        handle.join().expect("clean run"),
        Ok(value),
        "source: {source:?}"
    );
}

fn expect_err(source: &str, message_contains: &str) {
    let error = run_main(source).expect_err("expected a runtime error");
    assert!(
        error.message.contains(message_contains),
        "error {:?} should mention {message_contains:?}",
        error.message
    );
}

/// The source must produce exactly one diagnostic mentioning `contains`.
fn one_diagnostic(source: &str, contains: &str) {
    let outcome = cme_compiler::parse_source(source);
    let mut messages: Vec<String> = outcome
        .diagnostics
        .iter()
        .map(|error| error.message().to_string())
        .collect();
    messages.extend(check(&outcome.statements).iter().map(|e| e.to_string()));
    assert!(
        messages.len() == 1 && messages[0].contains(contains),
        "expected one diagnostic containing {contains:?}, got: {messages:?}"
    );
}

fn main_of(body: &str) -> String {
    format!("int main() {{\n{body}\n}}\n")
}

// ---------------------------------------------------------------------------
// The compiler's own budgets (plan-mandated depth/complexity guards)
// ---------------------------------------------------------------------------

#[test]
fn nesting_budget_is_clean_at_the_edges() {
    // Deep-but-legal nesting parses without complaint.
    let ok = main_of(&format!(
        "int x = {}1{}\nreturn x",
        "(".repeat(50),
        ")".repeat(50)
    ));
    let outcome = cme_compiler::parse_source(&ok);
    assert!(
        outcome.diagnostics.is_empty(),
        "50-deep parens must parse: {:?}",
        outcome.diagnostics
    );

    // Past MAX_NESTING_DEPTH there is exactly ONE clean diagnostic —
    // never a stack overflow, never a diagnostic cascade.
    let too_deep = main_of(&format!(
        "int x = {}1{}\nreturn x",
        "(".repeat(200),
        ")".repeat(200)
    ));
    one_diagnostic(&too_deep, "expression nesting is too deep");

    // Blocks are budgeted the same way: 100-deep is fine.
    let mut blocks = String::from("int main() {\nint n = 0\n");
    for _ in 0..100 {
        blocks.push_str("if (true) {\n");
    }
    blocks.push_str("n += 1\n");
    for _ in 0..100 {
        blocks.push_str("}\n");
    }
    blocks.push_str("return n\n}\n");
    let outcome = cme_compiler::parse_source(&blocks);
    assert!(
        outcome.diagnostics.is_empty(),
        "100-deep blocks must parse: {:?}",
        outcome.diagnostics
    );
}

#[test]
fn operator_budget_is_enforced_with_a_clean_diagnostic() {
    // The last operator inside the budget parses...
    let ok = main_of(&format!(
        "int x = 1{}\nreturn x",
        " + 1".repeat(cme_compiler::parser::MAX_EXPR_OPERATORS)
    ));
    let outcome = cme_compiler::parse_source(&ok);
    assert!(
        outcome.diagnostics.is_empty(),
        "the operator budget edge must parse: {:?}",
        outcome.diagnostics
    );

    // ...one more is a single clean diagnostic.
    let too_wide = main_of(&format!(
        "int x = 1{}\nreturn x",
        " + 1".repeat(cme_compiler::parser::MAX_EXPR_OPERATORS + 1)
    ));
    one_diagnostic(&too_wide, "expression is too complex");
}

// ---------------------------------------------------------------------------
// The owner-ruled shadowing ban across every binding form
// ---------------------------------------------------------------------------

#[test]
fn for_loop_bindings_honor_the_shadowing_ban() {
    // A for element may not shadow an enclosing declaration...
    one_diagnostic(
        "int main() {\nint v = 1\nfor (int v in [1, 2]) {\n}\nreturn v\n}",
        "shadows a declaration",
    );
    // ...including when a nested block sits between the declarations.
    one_diagnostic(
        "int main() {\nint v = 1\nif (true) {\nfor (int v in [1, 2]) {\n}\n}\nreturn v\n}",
        "shadows a declaration",
    );
    // Distinct names are fine and the element binds per iteration.
    expect(
        &main_of("int total = 0\nfor (int v in [1, 2, 3]) {\ntotal += v\n}\nreturn total"),
        Value::Int(6),
    );
}

#[test]
fn match_pattern_bindings_honor_the_shadowing_ban() {
    one_diagnostic(
        "enum e3 {\nA(int v)\nB()\n}\nint main() {\nint v = 1\ne3 e = e3.A(1)\nint r = 0\nmatch (e) {\nA(int v) => {\nr = v\n}\nB() => {\n}\n}\nreturn r\n}",
        "shadows a declaration",
    );
    // Distinct names bind the payload.
    expect(
        "enum e3 {\nA(int v)\nB()\n}\nint main() {\ne3 e = e3.A(7)\nmatch (e) {\nA(int payload) => {\nreturn payload\n}\nB() => {\nreturn 0\n}\n}\n}",
        Value::Int(7),
    );
}

#[test]
fn the_shadowing_ban_reaches_nested_scopes_of_every_kind() {
    // while bodies...
    one_diagnostic(
        "int main() {\nint i = 1\nwhile (i < 2) {\nint i = 2\ni += 1\n}\nreturn i\n}",
        "shadows a declaration",
    );
    // ...and nested function bodies re-declaring an outer LOCAL is fine
    // (functions have their own scopes), but two locals in one scope are not.
    one_diagnostic(
        "int main() {\nint a = 1\nint a = 2\nreturn a\n}",
        "duplicate declaration of `a`",
    );
}

// ---------------------------------------------------------------------------
// Runtime shapes near the edges
// ---------------------------------------------------------------------------

#[test]
fn deeply_nested_collections_run() {
    expect(
        &main_of("int[][][] cube = [[[1, 2], [3, 4]], [[5, 6], [7, 8]]]\nreturn cube[1][0][1]"),
        Value::Int(6),
    );
    expect(
        &main_of(
            "map<str, map<str, int>> nested = {}\nnested[\"a\"] = {}\nnested[\"a\"][\"b\"] = 42\nreturn nested[\"a\"][\"b\"]",
        ),
        Value::Int(42),
    );
    // Mutation through the nested path is visible in place.
    expect(
        &main_of("int[][] grid = [[1], [2]]\ngrid[0][0] = 9\nreturn grid[0][0] + grid[1][0]"),
        Value::Int(11),
    );
}

#[test]
fn wide_maps_build_iterate_and_sum() {
    let mut source = main_of_head();
    source.push_str("map<int, int> m = {}\n");
    for i in 0..50 {
        source.push_str(&format!("m[{i}] = {i} * 2\n"));
    }
    source.push_str("int total = 0\nfor (int k in m) {\ntotal += m[k]\n}\nreturn total\n}\n");
    expect(&source, Value::Int(50 * 49));
}

fn main_of_head() -> String {
    "int main() {\n".to_string()
}

#[test]
fn long_call_pipelines_run() {
    // A 60-function chain: each adds one.
    let mut source = String::from("int f0() {\nreturn 0\n}\n");
    for i in 1..60 {
        source.push_str(&format!("int f{i}() {{\nreturn f{}() + 1\n}}\n", i - 1));
    }
    source.push_str("int main() {\nreturn f59()\n}\n");
    expect_on_big_stack(&source, Value::Int(59));
}

#[test]
fn while_loops_compose_and_terminate() {
    // Σ_{i=0..9} Σ_{j=0..i-1} j = Σ i(i-1)/2 = 120.
    expect(
        &main_of(
            "int i = 0\nint total = 0\nwhile (i < 10) {\nint j = 0\nwhile (j < i) {\ntotal += j\nj += 1\n}\ni += 1\n}\nreturn total",
        ),
        Value::Int(120),
    );
}

#[test]
fn runtime_errors_identify_the_operation() {
    expect_err(&main_of("int[] a = [1]\nreturn a[5]"), "out of bounds");
    expect_err(
        &main_of("map<str, int> m = {}\nreturn m[\"missing\"]"),
        "missing",
    );
    expect_err(&main_of("return 1 / 0"), "division by zero");
}

// ---------------------------------------------------------------------------
// The interpreter's own depth guard
// ---------------------------------------------------------------------------

#[test]
fn recursion_budget_sits_at_the_documented_limit() {
    // Deep-but-legal recursion terminates with the right answer...
    expect_on_big_stack(
        "int down(int n) {\nif (n == 0) {\nreturn 0\n}\nreturn down(n - 1)\n}\nint main() {\nreturn down(500)\n}\n",
        Value::Int(0),
    );
    // ...and runaway recursion is a clean error, not a crash. The big
    // stack lets the interpreter's own 1024 depth limit (not thread
    // exhaustion) do the terminating.
    let owned =
        "int loop(int n) {\nreturn loop(n + 1)\n}\nint main() {\nreturn loop(0)\n}".to_string();
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_main(&owned).expect_err("depth limit").message)
        .expect("spawn");
    let message = handle.join().expect("clean error, no panic");
    assert!(
        message.contains("depth") || message.contains("call depth"),
        "got: {message:?}"
    );
}

#[test]
fn trailing_commas_are_rejected_in_call_arguments() {
    // Newlines are free inside parens (§A.8), but a trailing comma before
    // the closer is a clean parse error at the right place.
    one_diagnostic(
        "int f(int a, int b) {\nreturn a + b\n}\nint main() {\nreturn f(\n1,\n2,\n)\n}\n",
        "expected an expression",
    );
    // The same call without the trailing comma parses and runs.
    expect(
        "int f(int a, int b) {\nreturn a + b\n}\nint main() {\nreturn f(\n1,\n2\n)\n}\n",
        Value::Int(3),
    );
}
