//! Byte crystallization suite (§2.4): the checker/walker agreement
//! matrix for a direct integer literal on the other side of a byte
//! operand. The checker crystallizes such a literal — in equality,
//! comparison, and arithmetic, in parenthesized position, and at every
//! typed slot — so the tree walker must evaluate it in the byte domain:
//! same-domain overflow termination, Byte-shaped results, and strict
//! compile-time range checks everywhere.
//!
//! This suite pins the seam that `lang_byte_semantics.rs` deliberately
//! left unpinned while the walker still compared `Byte` against `Int`
//! structurally. Every runtime expectation below goes through the full
//! front end + checker + walker pipeline, so a checker/walker divergence
//! fails the pin either way.

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

/// The source must check with exactly one diagnostic mentioning `contains`.
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

fn byte_main(body: &str) -> String {
    format!("byte main() {{\n{body}\n}}\n")
}

fn program(prelude: &str, body: &str) -> String {
    format!("{prelude}\nint main() {{\n{body}\n}}\n")
}

/// Runs `run_main` on a generous stack — debug-build interpreter frames are
/// wide, and deep user-function chains multiply them (the same idiom as
/// `lang_stress` and `lang_runtime`).
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

/// A prelude plus a `byte main` entry: the return keeps its Byte shape, so
/// slot-shape pins can distinguish Byte(N) from an Int that merely widened.
fn byte_program(prelude: &str, body: &str) -> String {
    format!("{prelude}\nbyte main() {{\n{body}\n}}\n")
}

// ---------------------------------------------------------------------------
// Equality against a crystallized literal (the reported `b == 5` bug)
// ---------------------------------------------------------------------------

#[test]
fn byte_equals_literal_is_true_when_values_match() {
    expect(
        &int_main("byte b = 5\nif (b == 5) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
}

#[test]
fn literal_equals_byte_is_true_when_values_match() {
    // The mirror form: the crystallization is order-independent.
    expect(
        &int_main("byte b = 5\nif (5 == b) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
}

#[test]
fn byte_equals_literal_is_false_when_values_differ() {
    expect(
        &int_main("byte b = 5\nif (b == 6) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
    expect(
        &int_main("byte b = 5\nif (6 == b) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
}

#[test]
fn byte_not_equals_literal_covers_both_directions() {
    expect(
        &int_main("byte b = 5\nif (b != 6) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 5\nif (6 != b) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 5\nif (b != 5) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
}

#[test]
fn byte_equality_against_literals_at_the_domain_boundaries() {
    // 0 and 255 are the byte extremes: equality must hold exactly there.
    expect(
        &int_main("byte b = 0\nif (b == 0) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 255\nif (b == 255) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 255\nif (b == 254) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
    expect(
        &int_main("byte b = 0\nif (b == 255) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
}

#[test]
fn byte_equality_against_literal_after_reassignment() {
    // The slot must keep its byte shape across a plain assignment for the
    // crystallized comparison to stay in the byte domain.
    expect(
        &int_main("byte b = 5\nb = 6\nif (b == 6) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 5\nb = 200\nb = 201\nif (b == 201) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
}

#[test]
fn byte_equality_against_literal_inside_loops() {
    // A countdown whose termination condition is a crystallized literal:
    // if the walker compared shapes instead of values, the loop would run
    // away or never terminate.
    expect(
        &int_main(
            "byte b = 5\nint steps = 0\nwhile (b != 0) {\nb -= 1\nsteps += 1\n}\nreturn steps",
        ),
        Value::Int(5),
    );
    expect(
        &int_main(
            "byte b = 0\nint steps = 0\nwhile (b != 3) {\nb += 1\nsteps += 1\n}\nreturn steps",
        ),
        Value::Int(3),
    );
}

#[test]
fn infer_equality_against_literal_crystallizes_bool() {
    // `infer x = b == 5` crystallizes bool; the walker must produce the
    // same truth the checker's byte == byte typing implies.
    expect(
        &int_main("byte b = 5\ninfer same = b == 5\nif (same) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 5\ninfer same = b == 6\nif (same) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
}

#[test]
fn equality_between_byte_variables_stays_structural() {
    // No literal involved: byte-vs-byte equality through variables.
    expect(
        &int_main("byte a = 5\nbyte b = 5\nif (a == b) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
}

#[test]
fn byte_and_int_variable_equality_stays_a_type_error() {
    // Crystallization is a LITERAL rule: a genuine int variable never
    // compares against a byte (§2.4, §A.4).
    check_one_error(
        "int main() {\nbyte b = 5\nint i = 5\nif (b == i) {\nreturn 0\n}\nreturn 1\n}",
        "cannot apply `==` to `byte` and `int`",
    );
}

// ---------------------------------------------------------------------------
// Arithmetic against a crystallized literal (the reported `b + 100` bug)
// ---------------------------------------------------------------------------

#[test]
fn byte_plus_literal_that_fits_stays_byte_shaped() {
    // 200 + 55 fits in the byte domain: the result is Byte-shaped, not an
    // int that the declaration slot coerced back.
    expect(
        &byte_main("byte b = 200\nbyte c = b + 55\nreturn c"),
        Value::Byte(255),
    );
    expect(
        &byte_main("byte b = 200\nbyte c = 55 + b\nreturn c"),
        Value::Byte(255),
    );
    expect(
        &byte_main("byte b = 0\nbyte c = b + 255\nreturn c"),
        Value::Byte(255),
    );
}

#[test]
fn byte_plus_literal_past_the_domain_overflows() {
    // The reported bug: 200 + 100 must terminate, not silently widen.
    expect_err(
        &int_main("byte b = 200\nbyte c = b + 100\nreturn c"),
        "integer overflow in `+`",
    );
    expect_err(
        &int_main("byte b = 200\nbyte c = 100 + b\nreturn c"),
        "integer overflow in `+`",
    );
    expect_err(
        &byte_main("byte b = 255\nbyte c = b + 1\nreturn c"),
        "integer overflow in `+`",
    );
    // Overflowing even when the result feeds a widening int slot: the
    // byte domain is a property of the EXPRESSION, not the destination.
    expect_err(
        &int_main("byte b = 200\nint x = b + 100\nreturn x"),
        "integer overflow in `+`",
    );
}

#[test]
fn byte_minus_literal_underflow_and_exactness() {
    expect_err(
        &int_main("byte b = 200\nbyte c = b - 201\nreturn c"),
        "integer overflow in `-`",
    );
    expect(
        &byte_main("byte b = 200\nbyte c = b - 200\nreturn c"),
        Value::Byte(0),
    );
    // Mirror form: the literal is the left operand.
    expect_err(
        &int_main("byte b = 200\nbyte c = 199 - b\nreturn c"),
        "integer overflow in `-`",
    );
    expect(
        &byte_main("byte b = 200\nbyte c = 200 - b\nreturn c"),
        Value::Byte(0),
    );
}

#[test]
fn byte_times_literal_overflows_in_the_byte_domain() {
    expect_err(
        &int_main("byte b = 200\nbyte c = b * 2\nreturn c"),
        "integer overflow in `*`",
    );
    expect_err(
        &int_main("byte b = 200\nbyte c = 2 * b\nreturn c"),
        "integer overflow in `*`",
    );
    // 255 * 1 and 0 * anything stay exact.
    expect(
        &byte_main("byte b = 255\nbyte c = b * 1\nreturn c"),
        Value::Byte(255),
    );
    expect(
        &byte_main("byte b = 255\nbyte c = b * 0\nreturn c"),
        Value::Byte(0),
    );
}

#[test]
fn byte_division_and_remainder_against_literals() {
    expect(
        &byte_main("byte b = 200\nbyte c = b / 3\nreturn c"),
        Value::Byte(66),
    );
    expect(
        &byte_main("byte b = 200\nbyte c = b % 3\nreturn c"),
        Value::Byte(2),
    );
    // Mirror: a crystallized literal dividend.
    expect(
        &byte_main("byte b = 200\nbyte c = 255 / b\nreturn c"),
        Value::Byte(1),
    );
    expect(
        &byte_main("byte b = 200\nbyte c = 255 % b\nreturn c"),
        Value::Byte(55),
    );
    expect_err(
        &int_main("byte b = 200\nbyte c = b / 0\nreturn c"),
        "integer division by zero",
    );
    expect_err(
        &int_main("byte b = 200\nbyte c = b % 0\nreturn c"),
        "integer remainder by zero",
    );
}

#[test]
fn byte_literal_chains_stay_in_the_byte_domain() {
    // ((b + 1) + 2): each step's left operand is byte-typed, so every
    // literal crystallizes and the whole chain computes in bytes.
    expect(
        &byte_main("byte b = 200\nbyte c = b + 1 + 2\nreturn c"),
        Value::Byte(203),
    );
    expect_err(
        &int_main("byte b = 200\nbyte c = b + 1 + 100\nreturn c"),
        "integer overflow in `+`",
    );
    // A literal-led chain: 1 + b is byte, then + 2 crystallizes too.
    expect(
        &byte_main("byte b = 200\nbyte c = 1 + b + 2\nreturn c"),
        Value::Byte(203),
    );
}

#[test]
fn parenthesized_literals_do_not_crystallize_from_binary_position() {
    // The checker's Binary arm crystallizes a DIRECT IntLit operand only:
    // `(100)` is a Paren node, so `b + (100)` types as int. A byte slot
    // rejects it, and an int slot computes in int arithmetic (no overflow
    // at 300). The walker mirrors the node-kind rule exactly.
    check_one_error(
        "int main() {\nbyte b = 200\nbyte c = b + (100)\nreturn 0\n}",
        "type mismatch in declaration of `c`: expected `byte`, found `int`",
    );
    check_one_error(
        "int main() {\nbyte b = 200\nbyte c = (100) + b\nreturn 0\n}",
        "type mismatch in declaration of `c`: expected `byte`, found `int`",
    );
    // Deeply parenthesized: same rule.
    check_one_error(
        "int main() {\nbyte b = 200\nbyte c = b + ((100))\nreturn 0\n}",
        "type mismatch in declaration of `c`: expected `byte`, found `int`",
    );
    // In int slots the values are the plain int sums.
    expect(
        &int_main("byte b = 200\nint x = b + (100)\nreturn x"),
        Value::Int(300),
    );
    expect(
        &int_main("byte b = 200\nint x = (100) + b\nreturn x"),
        Value::Int(300),
    );
    expect(
        &int_main("byte b = 200\nint x = b + ((100))\nreturn x"),
        Value::Int(300),
    );
}

#[test]
fn parenthesized_sum_on_the_right_is_genuine_int_arithmetic() {
    // `b + (1 + 2)`: the parens wrap a BINARY, which ignores the expected
    // type — the sum is int, so the whole expression is int.
    check_one_error(
        "int main() {\nbyte b = 200\nbyte c = b + (1 + 2)\nreturn 0\n}",
        "type mismatch in declaration of `c`: expected `byte`, found `int`",
    );
    expect(
        &int_main("byte b = 200\nint x = b + (1 + 2)\nreturn x"),
        Value::Int(203),
    );
}

#[test]
fn negated_literal_does_not_crystallize() {
    // `-1` is an expression, not a literal: byte + int widens to int.
    check_one_error(
        "int main() {\nbyte b = 200\nbyte c = b + -1\nreturn 0\n}",
        "type mismatch in declaration of `c`: expected `byte`, found `int`",
    );
    expect(
        &int_main("byte b = 200\nint x = b + -1\nreturn x"),
        Value::Int(199),
    );
}

#[test]
fn byte_minus_literal_that_would_go_negative_in_int_stays_unsigned() {
    // 0 - 1 in the byte domain underflows; the same expression with a
    // byte VARIABLE left operand is the pinned underflow case.
    expect_err(
        &int_main("byte b = 0\nbyte c = b - 1\nreturn c"),
        "integer overflow in `-`",
    );
}

#[test]
fn mixed_byte_variable_plus_int_variable_still_widens() {
    // Regression pin for §A.4: with a genuine int operand there is no
    // crystallization and no byte-domain overflow — the values are int.
    expect(
        &int_main("byte b = 200\nint i = 100\nreturn b + i"),
        Value::Int(300),
    );
    expect(
        &int_main("byte b = 200\nint i = 100\nreturn i + b"),
        Value::Int(300),
    );
    // The same expression through a byte slot is a type error: int never
    // narrows back.
    check_one_error(
        "int main() {\nbyte b = 200\nint i = 100\nbyte c = b + i\nreturn 0\n}",
        "type mismatch in declaration of `c`: expected `byte`, found `int`",
    );
}

#[test]
fn string_concatenation_of_a_crystallized_expression_renders_digits() {
    // The inner `b + 1` computes in the byte domain; concatenation then
    // renders the byte exactly like its int value (§A.6).
    expect(
        &int_main(
            "byte b = 200\nstr s = \"v\" + (b + 1)\nif (s == \"v201\") {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Ordering comparisons against crystallized literals
// ---------------------------------------------------------------------------

#[test]
fn byte_comparisons_against_literals_are_exact() {
    expect(
        &int_main("byte b = 5\nif (b < 6) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 5\nif (6 > b) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 5\nif (b <= 5) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 5\nif (b >= 5) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 5\nif (b > 5) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
    expect(
        &int_main("byte b = 5\nif (b < 5) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
}

#[test]
fn byte_comparisons_against_boundary_literals() {
    // 255 and 0 are the extremes: comparisons against them must hold in
    // the byte domain, where nothing exceeds either bound.
    expect(
        &int_main("byte b = 255\nif (b <= 255) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 255\nif (b < 255) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
    expect(
        &int_main("byte b = 0\nif (b >= 0) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("byte b = 0\nif (b > 0) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
}

#[test]
fn byte_comparison_loop_guard_against_a_literal() {
    // A grow-toward-bound loop: the guard compares in the byte domain on
    // every iteration.
    expect(
        &int_main(
            "byte b = 250\nint steps = 0\nwhile (b < 255) {\nb += 1\nsteps += 1\n}\nreturn steps",
        ),
        Value::Int(5),
    );
}

#[test]
fn byte_comparison_against_an_out_of_range_literal_is_a_type_error() {
    // Crystallization range-checks literals in comparison position too:
    // 300 can never be a byte, so the comparison is rejected outright.
    check_one_error(
        "int main() {\nbyte b = 5\nif (b == 256) {\nreturn 0\n}\nreturn 1\n}",
        "byte literal out of range: `256` does not fit in 0..=255",
    );
    check_one_error(
        "int main() {\nbyte b = 5\nif (b < 300) {\nreturn 0\n}\nreturn 1\n}",
        "byte literal out of range",
    );
    check_one_error(
        "int main() {\nbyte b = 5\nif (300 >= b) {\nreturn 0\n}\nreturn 1\n}",
        "byte literal out of range",
    );
}

// ---------------------------------------------------------------------------
// Crystallization at typed slots: runtime shapes
// ---------------------------------------------------------------------------

#[test]
fn byte_param_slot_crystallizes_the_literal() {
    // The argument literal crystallizes at the parameter slot: inside the
    // callee the value is Byte-shaped, which a `byte main` entry reveals.
    expect(
        &byte_program("byte echo(byte v) {\nreturn v\n}", "return echo(7)"),
        Value::Byte(7),
    );
}

#[test]
fn byte_param_arithmetic_inside_the_callee_is_byte_domain() {
    // A crystallized literal argument makes the callee's arithmetic run in
    // the byte domain: 200 + 100 overflows even though the callee only
    // sees a plain `byte` parameter, and even when the result feeds a
    // widening int slot — the domain follows the EXPRESSION, not the
    // destination.
    expect_err(
        &program(
            "byte shift(byte v) {\nbyte bumped = v + 100\nreturn bumped\n}",
            "int x = shift(200)\nreturn x",
        ),
        "integer overflow in `+`",
    );
    expect_err(
        &program(
            "int widen(byte v) {\nint widened = v + 100\nreturn widened\n}",
            "return widen(200)",
        ),
        "integer overflow in `+`",
    );
    // In-range arithmetic through the same callee is exact.
    expect(
        &program(
            "int shift(byte v) {\nint widened = v + 55\nreturn widened\n}",
            "return shift(200)",
        ),
        Value::Int(255),
    );
}

#[test]
fn byte_return_slot_crystallizes_the_literal() {
    expect(
        &byte_program("byte give() {\nreturn 7\n}", "return give()"),
        Value::Byte(7),
    );
}

#[test]
fn byte_struct_field_slot_crystallizes_the_literal() {
    expect(
        &byte_program(
            "struct pixel {\nbyte r\nbyte g\n}",
            "pixel p = pixel(r: 7, g: 9)\nreturn p.r",
        ),
        Value::Byte(7),
    );
    expect(
        &byte_program(
            "struct pixel {\nbyte r\nbyte g\n}",
            "pixel p = pixel(r: 7, g: 9)\nreturn p.g",
        ),
        Value::Byte(9),
    );
}

#[test]
fn byte_struct_field_arithmetic_is_byte_domain() {
    // Field reads keep the Byte shape, so `p.r + 100` computes in bytes.
    expect_err(
        &program(
            "struct pixel {\nbyte r\n}",
            "pixel p = pixel(r: 200)\nint x = p.r + 100\nreturn x",
        ),
        "integer overflow in `+`",
    );
    expect(
        &byte_program(
            "struct pixel {\nbyte r\n}",
            "pixel p = pixel(r: 200)\nbyte r = p.r + 55\nreturn r",
        ),
        Value::Byte(255),
    );
}

#[test]
fn byte_enum_payload_slot_crystallizes_the_literal() {
    expect(
        &byte_program(
            "enum level {\nHigh(byte v)\nLow()\n}",
            "level l = level.High(7)\nmatch (l) {\nHigh(byte v) => {\nreturn v\n}\nLow() => {\nreturn 0\n}\n}",
        ),
        Value::Byte(7),
    );
}

#[test]
fn byte_enum_payload_arithmetic_is_byte_domain() {
    expect_err(
        &program(
            "enum level {\nHigh(byte v)\n}",
            "level l = level.High(200)\nint x = 0\nmatch (l) {\nHigh(byte v) => {\nx = v + 100\n}\n}\nreturn x",
        ),
        "integer overflow in `+`",
    );
}

#[test]
fn byte_array_element_slots_crystallize_the_literals() {
    expect(
        &byte_main("byte[] xs = [7, 9]\nreturn xs[0]"),
        Value::Byte(7),
    );
    expect(
        &byte_main("byte[] xs = [7, 9]\nreturn xs[1]"),
        Value::Byte(9),
    );
}

#[test]
fn byte_array_element_arithmetic_is_byte_domain() {
    // Element reads keep the Byte shape, so arithmetic against a literal
    // stays in the byte domain.
    expect_err(
        &int_main("byte[] xs = [200]\nint x = xs[0] + 100\nreturn x"),
        "integer overflow in `+`",
    );
    expect(
        &byte_main("byte[] xs = [200]\nbyte y = xs[0] + 55\nreturn y"),
        Value::Byte(255),
    );
}

#[test]
fn byte_map_literal_values_crystallize() {
    expect(
        &byte_main("map<str, byte> m = {\"a\": 7}\nreturn m[\"a\"]"),
        Value::Byte(7),
    );
}

#[test]
fn byte_map_literal_value_arithmetic_is_byte_domain() {
    // Values built from a literal map keep the Byte shape, so the
    // arithmetic is byte-domain and the overflow is caught in `+`.
    expect_err(
        &int_main("map<str, byte> m = {\"a\": 200}\nint x = m[\"a\"] + 100\nreturn x"),
        "integer overflow in `+`",
    );
    expect(
        &byte_main("map<str, byte> m = {\"a\": 200}\nbyte v = m[\"a\"] + 55\nreturn v"),
        Value::Byte(255),
    );
}

#[test]
fn byte_option_payload_slot_crystallizes_the_literal() {
    expect(
        &byte_main(
            "option<byte> o = Some(7)\nmatch (o) {\nSome(byte v) => {\nreturn v\n}\nNone() => {\nreturn 0\n}\n}",
        ),
        Value::Byte(7),
    );
}

#[test]
fn byte_result_payload_slots_crystallize_the_literals() {
    expect(
        &byte_main(
            "result<byte, str> r = Ok(7)\nmatch (r) {\nOk(byte v) => {\nreturn v\n}\nErr(str e) => {\nreturn 0\n}\n}",
        ),
        Value::Byte(7),
    );
}

#[test]
fn byte_foreach_element_over_a_byte_array_keeps_its_shape() {
    // The element type comes from the array (byte[]), so the binding is
    // Byte-shaped and the accumulation stays byte-domain.
    expect(
        &byte_main(
            "byte[] xs = [1, 2, 3]\nbyte total = 0\nfor (byte v in xs) {\ntotal += v\n}\nreturn total",
        ),
        Value::Byte(6),
    );
}

#[test]
fn for_loop_over_a_fresh_int_array_rejects_a_byte_binding() {
    // A fresh `[1, 2, 3]` is an int array: the element type does NOT
    // crystallize from the loop's byte binding.
    check_one_error(
        "int main() {\nfor (byte v in [1, 2, 3]) {\nreturn 0\n}\nreturn 1\n}",
        "wrong element type in for loop: expected `byte`, found `int`",
    );
}

#[test]
fn byte_slot_write_crystallizes_in_range_literals() {
    // Plain assignment into a byte slot keeps the byte shape.
    expect(&byte_main("byte b = 0\nb = 7\nreturn b"), Value::Byte(7));
    expect(
        &byte_main("byte[] xs = [0]\nxs[0] = 7\nreturn xs[0]"),
        Value::Byte(7),
    );
    expect(
        &byte_program(
            "struct pixel {\nbyte r\n}",
            "pixel p = pixel(r: 0)\np.r = 7\nreturn p.r",
        ),
        Value::Byte(7),
    );
}

#[test]
fn byte_slot_domain_survives_write_then_arithmetic() {
    // Write an in-range literal, then arithmetic against a literal: the
    // whole chain must stay in the byte domain (the reported bug via the
    // reassignment path).
    expect_err(
        &int_main("byte b = 5\nb = 200\nbyte c = b + 100\nreturn c"),
        "integer overflow in `+`",
    );
    expect(
        &byte_main("byte b = 5\nb = 200\nbyte c = b + 55\nreturn c"),
        Value::Byte(255),
    );
}

#[test]
fn compound_assignment_against_a_crystallized_literal_overflows() {
    // `b += 10` already overflowed correctly; the pin keeps the
    // crystallized-literal path honest for every compound operator.
    expect_err(
        &int_main("byte b = 250\nb += 10\nreturn b"),
        "integer overflow in `+`",
    );
    expect_err(
        &int_main("byte b = 5\nb -= 10\nreturn b"),
        "integer overflow in `-`",
    );
    expect_err(
        &int_main("byte b = 20\nb *= 30\nreturn b"),
        "integer overflow in `*`",
    );
    expect(
        &byte_main("byte b = 250\nb += 5\nreturn b"),
        Value::Byte(255),
    );
    expect(&byte_main("byte b = 10\nb -= 10\nreturn b"), Value::Byte(0));
}

// ---------------------------------------------------------------------------
// Range enforcement at runtime slots
// ---------------------------------------------------------------------------

#[test]
fn map_fresh_key_writes_domain_check_at_the_next_byte_slot() {
    // A fresh-key write into a byte-valued map stores the value unshaped
    // (the walker is type-agnostic about element types), so the arithmetic
    // computes as int and the next byte SLOT terminates the invocation
    // instead of storing an out-of-domain value. The checker passes the
    // program — the runtime domain check is the honest backstop.
    let source = int_main(
        "map<str, byte> m = {}\nm[\"a\"] = 250\nm[\"a\"] = m[\"a\"] + 10\nbyte c = m[\"a\"]\nreturn c",
    );
    let outcome = parse_source(&source);
    assert!(outcome.diagnostics.is_empty(), "front end must be clean");
    assert!(
        check(&outcome.statements).is_empty(),
        "checker must be clean"
    );
    let error = Interpreter::new(&outcome.statements)
        .invoke("main", &[])
        .expect_err("the out-of-domain value must terminate");
    assert!(
        error
            .message
            .contains("byte value out of range: `260` does not fit in 0..=255"),
        "error {:?} should name the byte domain violation",
        error.message
    );
}

#[test]
fn map_fresh_key_writes_in_range_still_domain_check() {
    // In-range values flow through the same seam and land as bytes.
    expect(
        &byte_main("map<str, byte> m = {}\nm[\"a\"] = 250\nbyte c = m[\"a\"]\nreturn c"),
        Value::Byte(250),
    );
}

#[test]
fn option_payload_slot_rejects_out_of_domain_values_at_compile_time() {
    check_one_error(
        "int main() {\noption<byte> o = Some(300)\nreturn 0\n}",
        "byte literal out of range: `300` does not fit in 0..=255",
    );
}

#[test]
fn result_payload_slot_rejects_out_of_domain_values_at_compile_time() {
    check_one_error(
        "int main() {\nresult<byte, str> r = Ok(300)\nreturn 0\n}",
        "byte literal out of range",
    );
}

// ---------------------------------------------------------------------------
// Compile-time range pins in operator position
// ---------------------------------------------------------------------------

#[test]
fn arithmetic_against_an_out_of_range_literal_is_rejected() {
    check_one_error(
        "int main() {\nbyte b = 200\nbyte c = b + 300\nreturn 0\n}",
        "byte literal out of range: `300` does not fit in 0..=255",
    );
    check_one_error(
        "int main() {\nbyte b = 200\nbyte c = 300 + b\nreturn 0\n}",
        "byte literal out of range",
    );
    check_one_error(
        "int main() {\nbyte b = 200\nbyte c = b * 300\nreturn 0\n}",
        "byte literal out of range",
    );
}

#[test]
fn assignment_of_an_out_of_range_literal_is_rejected() {
    check_one_error(
        "int main() {\nbyte b = 0\nb = 256\nreturn 0\n}",
        "byte literal out of range: `256` does not fit in 0..=255",
    );
    check_one_error(
        "int main() {\nbyte b = 0\nb += 300\nreturn 0\n}",
        "byte literal out of range",
    );
}

#[test]
fn argument_and_return_positions_reject_out_of_range_literals() {
    check_one_error(
        "int take(byte b) {\nreturn b\n}\nint main() {\nreturn take(300)\n}",
        "byte literal out of range",
    );
    check_one_error(
        "byte give() {\nreturn 300\n}\nint main() {\nreturn 0\n}",
        "byte literal out of range",
    );
}

#[test]
fn collection_slots_reject_out_of_range_literals() {
    check_one_error(
        "int main() {\nbyte[] xs = [300]\nreturn 0\n}",
        "byte literal out of range",
    );
    check_one_error(
        "int main() {\nmap<str, byte> m = {\"a\": 300}\nreturn 0\n}",
        "byte literal out of range",
    );
    check_one_error(
        "struct pixel {\nbyte r\n}\nint main() {\npixel p = pixel(r: 300)\nreturn 0\n}",
        "byte literal out of range",
    );
    check_one_error(
        "enum level {\nHigh(byte v)\n}\nint main() {\nlevel l = level.High(300)\nreturn 0\n}",
        "byte literal out of range",
    );
    check_one_error(
        "int main() {\nbyte[] xs = [1]\nxs[0] = 300\nreturn 0\n}",
        "byte literal out of range",
    );
    check_one_error(
        "int main() {\nmap<str, byte> m = {}\nm[\"a\"] = 300\nreturn 0\n}",
        "byte literal out of range",
    );
}

// ---------------------------------------------------------------------------
// `infer` crystallizes the byte-typed initializer
// ---------------------------------------------------------------------------

#[test]
fn infer_of_a_crystallized_expression_is_byte_typed() {
    // `b + 1` is byte (the literal crystallizes), so `infer` crystallizes
    // byte; widening into an int slot is lossless.
    expect(
        &int_main("byte b = 200\ninfer x = b + 1\nint y = x\nreturn y"),
        Value::Int(201),
    );
    expect(
        &byte_main("byte b = 200\ninfer x = b + 1\nreturn x"),
        Value::Byte(201),
    );
}

#[test]
fn infer_of_an_overflowing_crystallized_expression_terminates() {
    expect_err(
        &int_main("byte b = 200\ninfer x = b + 100\nreturn 0"),
        "integer overflow in `+`",
    );
}

#[test]
fn infer_of_a_byte_comparison_crystallizes_bool() {
    expect(
        &int_main("byte b = 5\ninfer flag = b >= 5\nif (flag) {\nreturn 1\n}\nreturn 0"),
        Value::Int(1),
    );
}

// ---------------------------------------------------------------------------
// Recursion and call-depth interactions with the byte domain
// ---------------------------------------------------------------------------

#[test]
fn byte_recursion_with_a_crystallized_literal_counts_down_exactly() {
    // A byte countdown through recursion: each `n - 1` computes in the
    // byte domain; the recursion bottoms out at the crystallized 0. Deep
    // chains multiply debug-build frames, so both run on the big stack.
    expect_on_big_stack(
        &program(
            "int count(byte n) {\nif (n == 0) {\nreturn 0\n}\nreturn 1 + count(n - 1)\n}",
            "return count(50)",
        ),
        Value::Int(50),
    );
    expect_on_big_stack(
        &program(
            "int count(byte n) {\nif (n == 0) {\nreturn 0\n}\nreturn 1 + count(n - 1)\n}",
            "return count(255)",
        ),
        Value::Int(255),
    );
}

#[test]
fn byte_underflow_terminates_mid_recursion() {
    // An unguarded countdown underflows the byte domain at 0 - 1 and
    // terminates the whole invocation cleanly.
    expect_err(
        &program(
            "int drain(byte n) {\nreturn 1 + drain(n - 1)\n}",
            "return drain(3)",
        ),
        "integer overflow in `-`",
    );
}

#[test]
fn byte_overflow_terminates_through_nested_calls() {
    // The overflow deep inside nested calls terminates the whole
    // invocation with the operator named.
    expect_err(
        &program(
            "int inner(byte b) {\nreturn b + 100\n}\nint outer(byte b) {\nreturn inner(b)\n}",
            "return outer(200)",
        ),
        "integer overflow in `+`",
    );
}
