//! Byte semantics suite (§2.4): the unsigned 8-bit scalar is the one
//! primitive with its own typing story — literal crystallization with a
//! compile-time range check, lossless widening to `int`, byte-domain
//! overflow-checked arithmetic, and strict equality. Every rule below is
//! pinned through the full front end + tree-walker pipeline.
//!
//! Shape notes: a `byte` value returned through an `int main` widens to an
//! `Int`-shaped value, so byte-shape pins use a `byte main` entry. The
//! checker/walker crystallization seam (a byte operand vs a direct integer
//! literal) is now closed and pinned in `lang_byte_crystallization.rs`;
//! this suite keeps byte-vs-byte equality and the mixed variable cases.

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

// ---------------------------------------------------------------------------
// Literal crystallization (§2.4)
// ---------------------------------------------------------------------------

#[test]
fn byte_literal_crystallizes_to_the_byte_runtime_shape() {
    // The declaration slot crystallizes the literal, and a `byte main`
    // return keeps the slot byte-typed: the value stays Byte-shaped.
    expect(&byte_main("byte b = 200\nreturn b"), Value::Byte(200));
    expect(&byte_main("byte b = 0\nreturn b"), Value::Byte(0));
    expect(&byte_main("byte b = 255\nreturn b"), Value::Byte(255));
}

#[test]
fn byte_returning_through_int_main_widens() {
    // The mirror shape: a byte value leaving through an int return slot
    // widens losslessly to an Int-shaped value.
    expect(&int_main("byte b = 200\nreturn b"), Value::Int(200));
    expect(&int_main("byte b = 255\nreturn b"), Value::Int(255));
}

#[test]
fn byte_literal_range_is_checked_at_compile_time() {
    check_one_error(
        "int main() {\nbyte b = 256\nreturn 0\n}",
        "byte literal out of range: `256` does not fit in 0..=255",
    );
    check_one_error(
        "int main() {\nbyte b = 300\nreturn 0\n}",
        "byte literal out of range",
    );
}

#[test]
fn a_non_literal_int_never_converts_to_byte() {
    // §2.4: a non-literal `int` never converts, even when the value is
    // statically known to fit.
    check_one_error(
        "int main() {\nint i = 5\nbyte b = i\nreturn 0\n}",
        "type mismatch in declaration of `b`: expected `byte`, found `int`",
    );
    // A negated literal is an expression, not a literal: no conversion.
    check_one_error(
        "int main() {\nbyte b = -1\nreturn 0\n}",
        "type mismatch in declaration of `b`: expected `byte`, found `int`",
    );
    // The same rule guards parameters...
    check_one_error(
        "int take(byte b) {\nreturn b\n}\nint main() {\nint i = 7\nreturn take(i)\n}",
        "wrong argument type in call to `take`: expected `byte`, found `int`",
    );
    // ...assignments...
    check_one_error(
        "int main() {\nbyte b = 0\nint i = 1\nb = i\nreturn 0\n}",
        "type mismatch in assignment to `b`: expected `byte`, found `int`",
    );
    // ...and returns.
    check_one_error(
        "byte give() {\nint i = 1\nreturn i\n}\nint main() {\nreturn 0\n}",
        "wrong return type in `give`: expected `byte`, found `int`",
    );
}

// ---------------------------------------------------------------------------
// Lossless widening (§2.4)
// ---------------------------------------------------------------------------

#[test]
fn byte_widens_losslessly_to_int_in_declarations() {
    expect(
        &int_main("byte b = 250\nint x = b\nreturn x"),
        Value::Int(250),
    );
}

#[test]
fn byte_widens_losslessly_at_param_slots() {
    expect(
        &program(
            "int grow(int x) {\nreturn x + 1\n}",
            "byte b = 5\nreturn grow(b)",
        ),
        Value::Int(6),
    );
}

#[test]
fn byte_widens_losslessly_at_return_slots() {
    // `give` returns a byte; main's declared `int` return widens it.
    expect(
        &program("byte give() {\nbyte b = 250\nreturn b\n}", "return give()"),
        Value::Int(250),
    );
}

#[test]
fn byte_and_int_arithmetic_yields_int() {
    // §2.4: mixed `byte op int` widens the byte and computes as int. With
    // variable operands both the checker and the walker agree, so the
    // result is int-shaped even past the byte ceiling.
    expect(
        &int_main("byte b = 200\nint i = 100\nreturn b + i"),
        Value::Int(300),
    );
    expect(
        &int_main("byte b = 200\nint i = 100\nreturn i + b"),
        Value::Int(300),
    );
    expect(
        &int_main("byte b = 5\nint i = 2\nreturn b * i"),
        Value::Int(10),
    );
    expect(
        &int_main("byte b = 5\nint i = 2\nreturn b / i"),
        Value::Int(2),
    );
    expect(
        &int_main("byte b = 5\nint i = 2\nreturn b % i"),
        Value::Int(1),
    );
}

// ---------------------------------------------------------------------------
// Byte-domain arithmetic is overflow-checked (§2.4)
// ---------------------------------------------------------------------------

#[test]
fn byte_plus_byte_stays_byte_and_overflows() {
    // Two byte VARIABLES compute in the 0..=255 domain with a checked add.
    expect_err(
        &int_main("byte a = 200\nbyte b = 100\nbyte c = a + b\nreturn c"),
        "integer overflow in `+`",
    );
}

#[test]
fn byte_addition_that_fits_stays_byte() {
    // 200 + 55 fits: the checker crystallizes the literal, so the walker
    // computes the addition in the byte domain and the result is
    // Byte-shaped directly.
    expect(
        &byte_main("byte b = 200\nbyte c = b + 55\nreturn c"),
        Value::Byte(255),
    );
}

#[test]
fn byte_subtraction_underflow_terminates() {
    expect_err(
        &int_main("byte a = 0\nbyte b = 1\nbyte c = a - b\nreturn c"),
        "integer overflow in `-`",
    );
}

#[test]
fn byte_multiplication_overflow_terminates() {
    expect_err(
        &int_main("byte a = 20\nbyte b = 30\nbyte c = a * b\nreturn c"),
        "integer overflow in `*`",
    );
}

#[test]
fn byte_compound_assignment_is_overflow_checked() {
    // `b += 10` on a byte slot: the literal right operand crystallizes
    // against the slot's runtime kind, so 250 + 10 overflows the byte
    // domain exactly like `b = b + 10` would.
    expect_err(
        &int_main("byte b = 250\nb += 10\nreturn b"),
        "integer overflow in `+`",
    );
    expect_err(
        &int_main("byte b = 0\nb -= 1\nreturn b"),
        "integer overflow in `-`",
    );
    // In-range compound updates keep the byte value in the 0..=255 domain.
    expect(
        &byte_main("byte b = 250\nb += 5\nreturn b"),
        Value::Byte(255),
    );
    expect(&byte_main("byte b = 10\nb -= 10\nreturn b"), Value::Byte(0));
    expect(&byte_main("byte b = 6\nb *= 7\nreturn b"), Value::Byte(42));
}

#[test]
fn byte_division_truncates_and_remainder_stays_exact() {
    // Byte division truncates like int division; returned through int
    // main each result widens to its Int shape.
    expect(
        &int_main("byte a = 7\nbyte b = 2\nreturn a / b"),
        Value::Int(3),
    );
    expect(
        &int_main("byte a = 7\nbyte b = 2\nreturn a % b"),
        Value::Int(1),
    );
    expect(
        &int_main("byte a = 255\nbyte b = 16\nreturn a % b"),
        Value::Int(15),
    );
    // Byte-typed slots keep the Byte shape.
    expect(
        &byte_main("byte a = 7\nbyte b = 2\nbyte q = a / b\nreturn q"),
        Value::Byte(3),
    );
}

#[test]
fn byte_division_and_remainder_by_zero_terminate() {
    expect_err(
        &int_main("byte a = 5\nbyte z = 0\nreturn a / z"),
        "division by zero",
    );
    expect_err(
        &int_main("byte a = 5\nbyte z = 0\nreturn a % z"),
        "remainder by zero",
    );
}

// ---------------------------------------------------------------------------
// Equality and comparison (§2.4, §A.4)
// ---------------------------------------------------------------------------

#[test]
fn byte_equality_is_strict_same_type() {
    expect(
        &int_main("byte a = 5\nbyte b = 5\nif (a == b) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    expect(
        &int_main("byte a = 5\nbyte b = 6\nif (a != b) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
}

#[test]
fn byte_and_int_equality_is_a_type_error() {
    // §2.4: equality remains strict — widening is an arithmetic rule, not
    // an equality mixing.
    check_one_error(
        "int main() {\nbyte b = 5\nint i = 5\nif (b == i) {\nreturn 0\n}\nreturn 1\n}",
        "cannot apply `==` to `byte` and `int`",
    );
    check_one_error(
        "int main() {\nbyte b = 5\nint i = 5\nif (b != i) {\nreturn 0\n}\nreturn 1\n}",
        "cannot apply `!=` to `byte` and `int`",
    );
}

#[test]
fn byte_comparisons_against_int_widen() {
    // Unlike equality, ordering comparisons accept byte/int mixings
    // (§A.4): the byte side widens.
    expect(
        &int_main("byte b = 5\nint i = 200\nif (b < i) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    expect(
        &int_main("byte b = 5\nint i = 200\nif (i > b) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    expect(
        &int_main(
            "byte b = 200\nint i = 200\nif (b <= i) {\nif (i >= b) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn byte_comparisons_stay_exact_within_the_byte_domain() {
    expect(
        &int_main("byte a = 1\nbyte b = 2\nif (a < b) {\nif (b >= a) {\nreturn 0\n}\n}\nreturn 1"),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Unary operators and indexing reject byte (§A.4)
// ---------------------------------------------------------------------------

#[test]
fn unary_minus_rejects_byte() {
    check_one_error(
        "int main() {\nbyte b = 5\nreturn -b\n}",
        "cannot apply `-` to `byte`",
    );
}

#[test]
fn logical_not_rejects_byte() {
    check_one_error(
        "int main() {\nbyte b = 5\nreturn !b\n}",
        "cannot apply `!` to `byte`",
    );
}

#[test]
fn array_indexing_rejects_byte() {
    // Widening is lossless for arithmetic slots, but an index must be a
    // real `int` — a byte does not index.
    check_one_error(
        "int main() {\nint[] a = [10, 20]\nbyte i = 1\nreturn a[i]\n}",
        "array index must be `int`, found `byte`",
    );
}

// ---------------------------------------------------------------------------
// Stringification (§A.6): a byte renders exactly like its int value
// ---------------------------------------------------------------------------

#[test]
fn byte_stringifies_as_decimal_digits() {
    expect(
        &int_main("byte b = 200\nstr s = \"v\" + b\nif (s == \"v200\") {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    expect(
        &int_main("byte b = 7\nstr s = b + \"v\"\nif (s == \"7v\") {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    expect(
        &int_main("byte b = 7\nstr s = $\"{b}\"\nif (s == \"7\") {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// Byte in composites: arrays, maps, structs, enums
// ---------------------------------------------------------------------------

#[test]
fn byte_arrays_iterate_with_widened_totals() {
    expect(
        &int_main(
            "byte[] bs = [1, 2, 250]\nint total = 0\nfor (byte v in bs) {\ntotal += v\n}\nif (total == 253) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn byte_array_element_assignment_crystallizes_in_range_literals() {
    // Reading the element back through an int slot widens, so the
    // comparison below is int-vs-int.
    expect(
        &int_main(
            "byte[] bs = [1]\nbs[0] = 5\nint v = bs[0]\nif (v == 5) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    check_one_error(
        "int main() {\nbyte[] bs = [1]\nbs[0] = 300\nreturn 0\n}",
        "byte literal out of range: `300` does not fit in 0..=255",
    );
}

#[test]
fn byte_reassignment_crystallizes_in_range_literals() {
    expect(
        &byte_main("byte b = 0\nb = 255\nreturn b"),
        Value::Byte(255),
    );
    check_one_error(
        "int main() {\nbyte b = 200\nb = 300\nreturn 0\n}",
        "byte literal out of range: `300` does not fit in 0..=255",
    );
}

#[test]
fn byte_map_values_crystallize_from_literals() {
    expect(
        &int_main(
            "map<str, byte> m = {}\nm[\"a\"] = 250\nint v = m[\"a\"]\nif (v == 250) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn byte_map_keys_require_a_real_byte() {
    // A literal map key does not crystallize in key position: the key slot
    // demands a genuine byte value.
    check_one_error(
        "int main() {\nmap<byte, str> m = {}\nm[1] = \"a\"\nreturn 0\n}",
        "map key must be `byte`, found `int`",
    );
    // With a byte variable the shape is exact.
    expect(
        &int_main(
            "map<byte, str> m = {}\nbyte k = 1\nm[k] = \"a\"\nif (m[k] == \"a\") {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn byte_struct_fields_crystallize_and_round_trip() {
    expect(
        &program(
            "struct pixel {\nbyte r\nbyte g\nbyte b\n}",
            "pixel p = pixel(r: 200, g: 44, b: 0)\nint total = p.r + p.g\nint blue = p.b\nif (blue == 0) {\nif (total == 244) {\nreturn 0\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn byte_struct_field_assignment_is_range_checked() {
    check_one_error(
        "struct pixel {\nbyte r\n}\nint main() {\npixel p = pixel(r: 0)\np.r = 999\nreturn 0\n}",
        "byte literal out of range",
    );
}

#[test]
fn byte_enum_payloads_crystallize_and_widen() {
    expect(
        &program(
            "enum level {\nHigh(byte v)\nLow()\n}",
            "level l = level.High(200)\nmatch (l) {\nHigh(byte v) => {\nint widened = v\nif (widened == 200) {\nreturn 0\n}\n}\nLow() => {\n}\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn byte_struct_equality_compares_byte_fields() {
    expect(
        &program(
            "struct pixel {\nbyte r\nbyte g\n}",
            "pixel a = pixel(r: 1, g: 2)\npixel b = pixel(r: 1, g: 2)\nif ((a == b) != true) {\nreturn 1\n}\npixel c = pixel(r: 1, g: 3)\nif ((a == c) != false) {\nreturn 2\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

// ---------------------------------------------------------------------------
// `infer` never crystallizes into byte (§2.16 interaction)
// ---------------------------------------------------------------------------

#[test]
fn infer_stays_context_free_for_byte_positions() {
    // `infer b = 5` crystallizes an int; a byte return slot does not
    // retroactively narrow it.
    check_one_error(
        "byte give() {\ninfer b = 5\nreturn b\n}\nint main() {\nreturn 0\n}",
        "wrong return type in `give`: expected `byte`, found `int`",
    );
    // But a byte VALUE widening into an inferred int binding is fine.
    expect(
        &int_main("byte b = 5\ninfer x = b\nint y = x\nreturn y"),
        Value::Int(5),
    );
}
