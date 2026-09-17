//! Collection suite: arrays and maps at runtime (§11) — typing exactness,
//! indexing and lookup edges, iteration order and element typing, equality,
//! and the interaction of collections with value semantics. Complements
//! `lang_stress` (which drives scale) by pinning fine-grained behaviors.

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

fn expect_err(source: &str, message_contains: &str) {
    let error = run_main(source).expect_err("expected a runtime error");
    assert!(
        error.message.contains(message_contains),
        "error {:?} should mention {message_contains:?}",
        error.message
    );
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
// Arrays: shapes, lengths, and boundaries
// ---------------------------------------------------------------------------

#[test]
fn empty_arrays_are_typed_by_their_declaration() {
    expect(
        &int_main("int[] a = []\nif (a.length == 0) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    check_one_error(
        "int main() {\nstr[] s = []\nint[] a = s\nreturn 0\n}",
        "type mismatch in declaration of `a`",
    );
}

#[test]
fn array_length_reflects_the_literal_size() {
    expect(&int_main("return [1, 2, 3].length"), Value::Int(3));
    expect(&int_main("int[] a = [1]\nreturn a.length"), Value::Int(1));
}

#[test]
fn array_length_is_read_only() {
    check_one_error(
        "int main() {\nint[] a = []\na.length = 3\nreturn 0\n}",
        "cannot assign to `.length`",
    );
}

#[test]
fn index_boundaries_are_exact() {
    expect(
        &int_main("int[] a = [10, 20, 30]\nreturn a[2]"),
        Value::Int(30),
    );
    expect_err(
        &int_main("int[] a = [10, 20, 30]\nreturn a[3]"),
        "out of bounds",
    );
    expect_err(
        &int_main("int[] a = [10, 20, 30]\nreturn a[0 - 1]"),
        "out of bounds",
    );
}

#[test]
fn indexing_requires_an_int() {
    check_one_error(
        "int main() {\nint[] a = [1]\nreturn a[true]\n}",
        "array index must be `int`",
    );
    check_one_error(
        "int main() {\nint[] a = [1]\nreturn a[0.0]\n}",
        "array index must be `int`, found `float`",
    );
}

#[test]
fn array_elements_are_homogeneous() {
    check_one_error(
        "int main() {\nint[] a = [1, 2, 3.0]\nreturn 0\n}",
        "array elements must all have type",
    );
    // A byte element widens into an int array (lossless).
    expect(
        &int_main("byte b = 7\nint[] a = [1, b]\nreturn a[1]"),
        Value::Int(7),
    );
    // ...but an int element never narrows into a byte array.
    check_one_error(
        "int main() {\nint i = 7\nbyte[] bs = [1, i]\nreturn 0\n}",
        "array elements must all have type",
    );
}

#[test]
fn array_assignment_through_the_element_path_is_exact() {
    expect(
        &int_main("int[] a = [1, 2]\na[0] = 9\nreturn a[0]"),
        Value::Int(9),
    );
    check_one_error(
        "int main() {\nint[] a = [1]\na[0] = 2.0\nreturn 0\n}",
        "type mismatch in assignment",
    );
}

#[test]
fn arrays_hold_structs_and_iterate_elements_as_copies() {
    expect(
        &program(
            "struct item { int id }",
            "item[] xs = [item(id: 1), item(id: 2)]\nint total = 0\nfor (item it in xs) {\ntotal += it.id\n}\nif (total == 3) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn mutating_a_for_element_never_writes_back_into_the_array() {
    // §2.13: the element binds a copy.
    expect(
        &int_main(
            "int[] xs = [1, 2]\nfor (int v in xs) {\nv = v + 100\n}\nif (xs[0] == 1) {\nif (xs[1] == 2) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn array_equality_is_elementwise_and_order_sensitive() {
    expect(
        &int_main("if ([1, 2] == [1, 2]) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    expect(
        &int_main("if ([1, 2] == [2, 1]) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
    expect(
        &int_main("if ([1, 2] != [1, 2, 3]) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    check_one_error(
        "int main() {\nif ([1] == [1.0]) {\nreturn 1\n}\nreturn 0\n}",
        "cannot apply `==`",
    );
}

#[test]
fn arrays_of_collections_compose() {
    expect(
        &int_main(
            "map<str, int>[] registry = [{}]\nregistry[0][\"k\"] = 5\nreturn registry[0][\"k\"]",
        ),
        Value::Int(5),
    );
    expect(
        &int_main(
            "map<str, int[]> groups = {}\ngroups[\"a\"] = [1, 2]\ngroups[\"a\"][1] = 9\nreturn groups[\"a\"][1]",
        ),
        Value::Int(9),
    );
}

#[test]
fn arrays_are_fresh_values_from_function_returns() {
    expect(
        &program(
            "int[] fresh() {\nint[] xs = [1, 2]\nreturn xs\n}",
            "int[] a = fresh()\na[0] = 99\nint[] b = fresh()\nif (b[0] == 1) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Maps: keys, lookups, and iteration order
// ---------------------------------------------------------------------------

#[test]
fn map_insertion_order_is_the_iteration_order_across_key_types() {
    // Insert 3, 1, 2: iteration follows insertion, not sorted order.
    expect(
        &int_main(
            "map<int, str> m = {}\nm[3] = \"c\"\nm[1] = \"a\"\nm[2] = \"b\"\nstr out = \"\"\nfor (int k in m) {\nout += m[k]\n}\nif (out == \"cab\") {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    expect(
        &int_main(
            "map<bool, int> m = {}\nm[false] = 1\nm[true] = 2\nint total = 0\nfor (bool k in m) {\ntotal += m[k]\n}\nif (total == 3) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn updating_a_key_keeps_its_original_position() {
    expect(
        &int_main(
            "map<int, str> m = {}\nm[1] = \"first\"\nm[2] = \"second\"\nm[1] = \"FIRST\"\nstr out = \"\"\nfor (int k in m) {\nout += m[k]\n}\nif (out == \"FIRSTsecond\") {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn map_lookups_and_misses() {
    expect(
        &int_main("map<str, int> m = {}\nm[\"k\"] = 5\nreturn m[\"k\"]"),
        Value::Int(5),
    );
    expect_err(
        &int_main("map<str, int> m = {}\nreturn m[\"missing\"]"),
        "not found",
    );
}

#[test]
fn map_key_lookups_respect_the_declared_key_type() {
    check_one_error(
        "int main() {\nmap<str, int> m = {}\nreturn m[1]\n}",
        "map key must be `str`, found `int`",
    );
    // The failed lookup still yields the map's value type, so binding it
    // to a correctly-typed variable keeps the cascade at one diagnostic.
    check_one_error(
        "int main() {\nmap<int, str> m = {}\nstr x = m[\"k\"]\nreturn 0\n}",
        "map key must be `int`, found `str`",
    );
}

#[test]
fn map_literal_entry_typing_is_exact() {
    check_one_error(
        "int main() {\nmap<str, int> m = { \"a\": 1, \"b\": 2.0 }\nreturn 0\n}",
        "map values must all have type",
    );
    check_one_error(
        "int main() {\nmap<str, int> m = { \"a\": 1, 2: 3 }\nreturn 0\n}",
        "map keys must all have type",
    );
}

#[test]
fn maps_accept_generic_control_values() {
    // option values ride in maps like any other value type.
    expect(
        &int_main(
            "map<str, option<int>> m = {}\nm[\"a\"] = option.Some(5)\nint found = 0\nmatch (m[\"a\"]) {\nSome(int v) => {\nfound = v\n}\nNone() => {\n}\n}\nif (found == 5) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn maps_hold_enums_and_return_them() {
    expect(
        &program(
            "enum mode { On()\nOff(int since) }",
            "map<str, mode> flags = {}\nflags[\"turbo\"] = mode.Off(7)\nint since = 0\nmatch (flags[\"turbo\"]) {\nOn() => {\n}\nOff(int s) => {\nsince = s\n}\n}\nif (since == 7) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn maps_have_no_length_field() {
    check_one_error(
        "int main() {\nmap<str, int> m = {}\nreturn m.length\n}",
        "unknown field `length` on `map<str, int>`",
    );
}

#[test]
fn map_equality_ignores_insertion_order() {
    expect(
        &int_main(
            "map<str, int> a = {}\na[\"x\"] = 1\na[\"y\"] = 2\nmap<str, int> b = {}\nb[\"y\"] = 2\nb[\"x\"] = 1\nif (a == b) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    expect(
        &int_main(
            "map<str, int> a = { \"x\": 1 }\nmap<str, int> b = { \"x\": 2 }\nif (a == b) {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn iterating_an_empty_collection_runs_zero_times() {
    expect(
        &int_main(
            "int n = 0\nfor (int v in []) {\nn += 1\n}\nmap<str, int> m = {}\nfor (str k in m) {\nn += 1\n}\nif (n == 0) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn iterating_a_map_yields_keys_of_the_key_type() {
    // Iterating with the correct key type binds real keys.
    expect(
        &int_main(
            "map<str, int> m = {}\nm[\"a\"] = 1\nm[\"b\"] = 2\nint n = 0\nfor (str k in m) {\nn += m[k]\n}\nif (n == 3) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn collections_nest_deeply_and_assign_through_paths() {
    // Path assignment does not auto-vivify: intermediate maps must exist
    // before a deeper path is written.
    expect(
        &int_main(
            "map<str, map<str, map<str, int>>> deep = {}\ndeep[\"a\"] = {}\ndeep[\"a\"][\"b\"] = {}\ndeep[\"a\"][\"b\"][\"c\"] = 42\nreturn deep[\"a\"][\"b\"][\"c\"]",
        ),
        Value::Int(42),
    );
    expect(
        &int_main("int[][][] cube = [[[1]]]\ncube[0][0][0] = 8\nreturn cube[0][0][0]"),
        Value::Int(8),
    );
}

#[test]
fn byte_elements_widen_when_their_container_is_int_typed() {
    // A leading int literal anchors the array literal's element unification
    // at int, so a byte element widens losslessly.
    expect(
        &int_main("byte b = 200\nint[] a = [1, b]\nreturn a[1] + 1"),
        Value::Int(201),
    );
    // With ONLY a byte element the literal unifies to byte[], which never
    // assigns into int[].
    check_one_error(
        "int main() {\nbyte b = 200\nint[] a = [b]\nreturn 0\n}",
        "type mismatch in declaration of `a`: expected `int[]`, found `byte[]`",
    );
}
