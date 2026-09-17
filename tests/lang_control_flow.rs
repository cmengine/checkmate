//! Control-flow suite: if/else chains, while and for-in loops, and the
//! match expression/statement surface (§2.14, §2.15) at runtime — scoping
//! per iteration, early returns from nested contexts, recursive user-defined
//! ADTs, and the exhaustiveness/wildcard shapes the checker admits.

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

fn int_main(body: &str) -> String {
    format!("int main() {{\n{body}\n}}\n")
}

fn program(prelude: &str, body: &str) -> String {
    format!("{prelude}\nint main() {{\n{body}\n}}\n")
}

// ---------------------------------------------------------------------------
// if / else-if / else
// ---------------------------------------------------------------------------

#[test]
fn else_if_chains_dispatch_in_order() {
    expect(
        &program(
            "int classify(int v) {\nif (v < 0) {\nreturn 0 - 1\n} else if (v == 0) {\nreturn 0\n} else {\nreturn 1\n}\n}",
            "if (classify(0 - 5) == 0 - 1) {\nif (classify(0) == 0) {\nif (classify(5) == 1) {\nreturn 0\n}\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn a_dangling_else_binds_to_the_nearest_if() {
    // The inner `if` takes the else, so `flag` stays false and the outer
    // branch falls through to the final return.
    expect(
        &int_main(
            "bool flag = false\nbool inner = true\nif (inner) {\nif (false) {\nflag = true\n} else {\nflag = false\n}\n}\nif (flag) {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn if_conditions_evaluate_once_and_branch_exactly() {
    expect(
        &int_main(
            "int n = 0\nif (true) {\nn += 1\n} else {\nn += 100\n}\nif (n == 1) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    expect(
        &int_main(
            "int n = 0\nif (false) {\nn += 1\n}\nn += 10\nif (n == 10) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn deeply_nested_ifs_compose() {
    let mut body = String::from("int depth = 0\n");
    for _ in 0..20 {
        body.push_str("if (true) {\ndepth += 1\n");
    }
    for _ in 0..20 {
        body.push_str("}\n");
    }
    body.push_str("return depth");
    expect(&int_main(&body), Value::Int(20));
}

// ---------------------------------------------------------------------------
// while
// ---------------------------------------------------------------------------

#[test]
fn while_loops_run_zero_or_many_times() {
    expect(
        &int_main("int n = 0\nwhile (false) {\nn += 1\n}\nreturn n"),
        Value::Int(0),
    );
    expect(
        &int_main("int n = 0\nint i = 0\nwhile (i < 5) {\nn += i\ni += 1\n}\nreturn n"),
        Value::Int(10),
    );
}

#[test]
fn while_conditions_may_call_functions() {
    expect(
        &program(
            "bool keepGoing(int n) {\nreturn n < 4\n}",
            "int n = 0\nwhile (keepGoing(n)) {\nn += 1\n}\nreturn n",
        ),
        Value::Int(4),
    );
}

#[test]
fn return_escapes_every_loop_nesting_level() {
    expect(
        &program(
            "int find(int[][] grid, int want) {\nfor (int[] row in grid) {\nfor (int v in row) {\nif (v == want) {\nreturn v\n}\n}\n}\nreturn 0 - 1\n}",
            "return find([[1, 2], [3, 4]], 3)",
        ),
        Value::Int(3),
    );
    expect(
        &program(
            "int find(int[][] grid, int want) {\nfor (int[] row in grid) {\nfor (int v in row) {\nif (v == want) {\nreturn v\n}\n}\n}\nreturn 0 - 1\n}",
            "return find([[1, 2], [3, 4]], 99)",
        ),
        Value::Int(-1),
    );
}

#[test]
fn while_bodies_get_fresh_scopes_per_iteration() {
    // A declaration inside the body rebinds each iteration; the loop
    // counter declared outside keeps accumulating.
    expect(
        &int_main(
            "int outer = 0\nint i = 0\nwhile (i < 3) {\nint fresh = i * 10\nouter += fresh\ni += 1\n}\nreturn outer",
        ),
        Value::Int(30),
    );
}

// ---------------------------------------------------------------------------
// for-in
// ---------------------------------------------------------------------------

#[test]
fn for_in_yields_elements_in_order() {
    expect(
        &int_main("int product = 1\nfor (int v in [2, 3, 4]) {\nproduct *= v\n}\nreturn product"),
        Value::Int(24),
    );
    expect(
        &int_main(
            "str joined = \"\"\nfor (str s in [\"a\", \"b\"]) {\njoined += s\n}\nif (joined == \"ab\") {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn for_over_structs_yields_field_values() {
    expect(
        &program(
            "struct pt { int x\nint y }",
            "int total = 0\nfor (pt p in [pt(x: 1, y: 2), pt(x: 3, y: 4)]) {\ntotal += p.x + p.y\n}\nreturn total",
        ),
        Value::Int(10),
    );
}

#[test]
fn early_return_inside_a_for_inside_an_if() {
    expect(
        &program(
            "int firstOver(int[] xs, int bar) {\nif (xs.length > 0) {\nfor (int v in xs) {\nif (v > bar) {\nreturn v\n}\n}\n}\nreturn 0 - 1\n}",
            "return firstOver([1, 7, 2], 5)",
        ),
        Value::Int(7),
    );
}

// ---------------------------------------------------------------------------
// match: expressions, statements, wildcards, ADTs
// ---------------------------------------------------------------------------

#[test]
fn match_expressions_dispatch_on_variant() {
    expect(
        &program(
            "enum e2 { A(int v)\nB(int tag)\nC() }\nint render(e2 x) {\nreturn match (x) {\nA(int v) => v\nB(int tag) => tag * 2\nC() => 0\n}\n}",
            "if (render(e2.A(7)) == 7) {\nif (render(e2.B(21)) == 42) {\nif (render(e2.C()) == 0) {\nreturn 0\n}\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn match_statement_arms_execute_side_effects_on_copies() {
    expect(
        &program(
            "enum op { Add(int amount)\nMul(int amount)\nNop() }\nint apply(int base, op o) {\nmatch (o) {\nAdd(int amount) => {\nbase = base + amount\n}\nMul(int amount) => {\nbase = base * amount\n}\nNop() => {\n}\n}\nreturn base\n}",
            "if (apply(10, op.Add(5)) == 15) {\nif (apply(10, op.Mul(5)) == 50) {\nif (apply(10, op.Nop()) == 10) {\nreturn 0\n}\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn wildcard_arms_catch_everything() {
    expect(
        &program(
            "enum e2 { A(int v)\nB()\nC() }\nint f(e2 x) {\nreturn match (x) {\nA(int v) => v\n_ => 100\n}\n}",
            "if (f(e2.A(3)) == 3) {\nif (f(e2.B()) == 100) {\nif (f(e2.C()) == 100) {\nreturn 0\n}\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn match_over_option_and_result_enums() {
    expect(
        &program(
            "str describe(option<int> o) {\nmatch (o) {\nSome(int v) => {\nreturn \"some\"\n}\nNone() => {\nreturn \"none\"\n}\n}\n}",
            "if (describe(option.Some(1)) == \"some\") {\nif (describe(option.None()) == \"none\") {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    expect(
        &program(
            "str render(result<int, str> r) {\nreturn match (r) {\nOk(int v) => \"\" + v\nErr(str e) => \"err:\" + e\n}\n}",
            "if (render(result.Ok(3)) == \"3\") {\nif (render(result.Err(\"boom\")) == \"err:boom\") {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn recursive_user_defined_adts_compute_recursively() {
    expect(
        &program(
            "enum tree {\nLeaf(int v)\nNode(tree l, tree r)\n}\nint sum(tree t) {\nmatch (t) {\nLeaf(int v) => {\nreturn v\n}\nNode(tree l, tree r) => {\nreturn sum(l) + sum(r)\n}\n}\n}",
            "tree t = tree.Node(tree.Leaf(1), tree.Node(tree.Leaf(2), tree.Leaf(3)))\nreturn sum(t)",
        ),
        Value::Int(6),
    );
    expect(
        &program(
            "enum tree {\nLeaf(int v)\nNode(tree l, tree r)\n}\nint depth(tree t) {\nmatch (t) {\nLeaf(int v) => {\nreturn 1\n}\nNode(tree l, tree r) => {\nint dl = depth(l)\nint dr = depth(r)\nint bigger = dl\nif (dr > bigger) {\nbigger = dr\n}\nreturn bigger + 1\n}\n}\n}",
            "tree t = tree.Node(tree.Leaf(1), tree.Node(tree.Leaf(2), tree.Leaf(3)))\nreturn depth(t)",
        ),
        Value::Int(3),
    );
}

#[test]
fn match_bindings_are_scoped_to_their_arm() {
    expect(
        &program(
            "enum e2 { A(int v)\nB() }\nint f(e2 x) {\nint v = 5\nmatch (x) {\nA(int w) => {\nv = w\n}\nB() => {\n}\n}\nreturn v\n}",
            "if (f(e2.A(9)) == 9) {\nif (f(e2.B()) == 5) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// The ? operator through deeper frames (§2.8)
// ---------------------------------------------------------------------------

#[test]
fn try_propagates_through_a_whole_call_chain() {
    expect(
        &program(
            "result<int, str> leaf(int v) {\nif (v == 0) {\nreturn Err(\"leaf\")\n}\nreturn Ok(v)\n}\nresult<int, str> mid(int v) {\nint x = leaf(v)?\nreturn Ok(x + 1)\n}\nresult<int, str> top(int v) {\nint y = mid(v)?\nreturn Ok(y + 1)\n}",
            "if (top(0) != result.Err(\"leaf\")) {\nreturn 1\n}\nif (top(1) != result.Ok(3)) {\nreturn 2\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn try_inside_loops_stops_the_whole_function() {
    expect(
        &program(
            "result<int, str> check(int v) {\nif (v == 3) {\nreturn Err(\"three\")\n}\nreturn Ok(v)\n}\nresult<int, str> scanAll() {\nfor (int i in [1, 2, 3, 4]) {\nint ok = check(i)?\n}\nreturn Ok(0)\n}",
            "if (scanAll() != result.Err(\"three\")) {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}
