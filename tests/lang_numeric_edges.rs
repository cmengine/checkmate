//! Numeric edge suite: §A.4 operand typing and §A.5 evaluation semantics —
//! truncating division and remainder signs, the checked-overflow boundary
//! at every i64 extreme, IEEE 754 float behavior including the canonical
//! display forms, and the precedence/associativity tree shapes of §A.2.
//! Complements `lang_runtime` by pinning the values that suite does not.

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

/// Builds `int main` whose body computes `expr` and returns it.
fn returns(expr: &str) -> String {
    format!("int main() {{\nreturn {expr}\n}}\n")
}

/// The most negative i64, spelled without a too-large literal.
const INT_MIN: &str = "0 - 9223372036854775807 - 1";

// ---------------------------------------------------------------------------
// Truncating division and remainder signs (§A.5)
// ---------------------------------------------------------------------------

#[test]
fn integer_division_truncates_toward_zero_in_every_sign_quadrant() {
    expect(&returns("7 / 2"), Value::Int(3));
    expect(&returns("(0 - 7) / 2"), Value::Int(-3));
    expect(&returns("7 / (0 - 2)"), Value::Int(-3));
    expect(&returns("(0 - 7) / (0 - 2)"), Value::Int(3));
}

#[test]
fn remainder_takes_the_sign_of_the_dividend() {
    expect(&returns("7 % 2"), Value::Int(1));
    expect(&returns("(0 - 7) % 2"), Value::Int(-1));
    expect(&returns("7 % (0 - 2)"), Value::Int(1));
    expect(&returns("(0 - 7) % (0 - 2)"), Value::Int(-1));
}

#[test]
fn division_identity_holds_for_truncation() {
    // For every sign combination, (a / b) * b + a % b == a.
    for a in ["7", "-7"] {
        for b in ["2", "-2"] {
            let negative = format!("({a}) / ({b}) * ({b}) + ({a}) % ({b})");
            expect(&returns(&negative), Value::Int(a.parse::<i64>().unwrap()));
        }
    }
}

#[test]
fn remainder_by_negative_zero_operand_message_is_exact() {
    expect_err(&returns("1 % 0"), "integer remainder by zero");
    expect_err(
        &int_main("int z = 0\nreturn 1 % z"),
        "integer remainder by zero",
    );
}

// ---------------------------------------------------------------------------
// The checked-overflow boundary at the i64 extremes (§2.4, §A.5)
// ---------------------------------------------------------------------------

#[test]
fn the_extremes_of_int_are_representable() {
    expect(&returns(INT_MIN), Value::Int(i64::MIN));
    expect(&returns("9223372036854775807"), Value::Int(i64::MAX));
}

#[test]
fn negating_the_most_negative_value_overflows() {
    // `-m` where m is i64::MIN is the one negation that overflows.
    expect_err(
        &int_main(&format!("int m = {INT_MIN}\nreturn -m")),
        "integer overflow in `-`",
    );
}

#[test]
fn arithmetic_at_the_extremes_is_exact_until_it_overflows() {
    expect(
        &int_main(&format!("int m = {INT_MIN}\nreturn m + 1")),
        Value::Int(i64::MIN + 1),
    );
    expect(
        &int_main(&format!("int m = {INT_MIN}\nreturn m - (0 - 1)")),
        Value::Int(i64::MIN + 1),
    );
    expect(
        &int_main("int m = 9223372036854775807\nreturn m - 1"),
        Value::Int(i64::MAX - 1),
    );
    expect_err(
        &int_main(&format!("int m = {INT_MIN}\nreturn m - 1")),
        "integer overflow in `-`",
    );
    expect_err(
        &int_main("int m = 9223372036854775807\nreturn m + 1"),
        "integer overflow in `+`",
    );
}

#[test]
fn the_one_overflowing_division_and_remainder_terminate() {
    // i64::MIN / -1 and i64::MIN % -1 are the only div/rem overflows.
    expect_err(
        &int_main(&format!("int m = {INT_MIN}\nreturn m / -1")),
        "integer overflow in `/`",
    );
    expect_err(
        &int_main(&format!("int m = {INT_MIN}\nreturn m % -1")),
        "integer overflow in `%`",
    );
}

#[test]
fn multiplication_overflow_reports_the_operator() {
    expect_err(
        &int_main("int a = 3037000500\nreturn a * a"),
        "integer overflow in `*`",
    );
    // Big factors, small product: no overflow (a * (a / 2) with
    // truncating division).
    expect(
        &int_main("int a = 3037000499\nreturn a * (a / 2)"),
        Value::Int(4_611_686_013_944_624_251),
    );
}

// ---------------------------------------------------------------------------
// Precedence and associativity tree shapes (§A.2)
// ---------------------------------------------------------------------------

#[test]
fn precedence_shapes_match_the_appendix_worked_examples() {
    // 1 + 2 * 3 == 7; 10 - 4 - 3 == 3 (left-associative); -x * y is
    // (-x) * y.
    expect(&returns("1 + 2 * 3"), Value::Int(7));
    expect(&returns("10 - 4 - 3"), Value::Int(3));
    expect(
        &int_main("int x = 3\nint y = 4\nreturn -x * y"),
        Value::Int(-12),
    );
}

#[test]
fn modulo_binds_like_multiplication() {
    // `*`, `/`, `%` share a level: 2 + 3 % 2 == 3, 10 / 2 / 5 == 1.
    expect(&returns("2 + 3 % 2"), Value::Int(3));
    expect(&returns("10 / 2 / 5"), Value::Int(1));
    expect(&returns("10 % 3 * 2"), Value::Int(2));
    expect(&returns("2 * 5 % 3"), Value::Int(1));
}

#[test]
fn double_negation_is_negation_twice() {
    // §A.2: since `--` is not a token, `--x` is identical to `-(-x)`.
    expect(&int_main("int x = 5\nreturn --x"), Value::Int(5));
    expect(&int_main("int x = 5\nreturn -(-(-x))"), Value::Int(-5));
    expect(&returns("-(-(3))"), Value::Int(3));
}

#[test]
fn unary_binds_tighter_than_multiplication() {
    expect(&int_main("int x = 3\nreturn -x * -x"), Value::Int(9));
    expect(&returns("-2 * -3"), Value::Int(6));
}

// ---------------------------------------------------------------------------
// Operand typing rejections not pinned elsewhere (§A.4)
// ---------------------------------------------------------------------------

#[test]
fn ordering_comparisons_reject_cross_type_float_int() {
    check_one_error(
        "int main() {\nfloat f = 1.0\nint i = 2\nif (f < i) {\nreturn 1\n}\nreturn 0\n}",
        "cannot apply `<` to `float` and `int`",
    );
    check_one_error(
        "int main() {\nfloat f = 1.0\nint i = 2\nif (i >= f) {\nreturn 1\n}\nreturn 0\n}",
        "cannot apply `>=` to `int` and `float`",
    );
}

#[test]
fn ordering_comparisons_reject_bool_and_str_operands() {
    check_one_error(
        "int main() {\nbool a = true\nbool b = false\nif (a < b) {\nreturn 1\n}\nreturn 0\n}",
        "cannot apply `<` to `bool` and `bool`",
    );
    check_one_error(
        "int main() {\nstr a = \"a\"\nstr b = \"b\"\nif (a <= b) {\nreturn 1\n}\nreturn 0\n}",
        "cannot apply `<=` to `str` and `str`",
    );
}

#[test]
fn arithmetic_rejects_bool_and_str_operands() {
    check_one_error(
        "int main() {\nreturn true + true\n}",
        "cannot apply `+` to `bool` and `bool`",
    );
    check_one_error(
        "int main() {\nstr a = \"a\"\nstr b = \"b\"\nreturn a - b\n}",
        "cannot apply `-` to `str` and `str`",
    );
    check_one_error(
        "int main() {\nstr a = \"a\"\nreturn a * 3\n}",
        "cannot apply `*` to `str` and `int`",
    );
}

// ---------------------------------------------------------------------------
// IEEE 754 float behavior (§A.4, §A.5)
// ---------------------------------------------------------------------------

#[test]
fn float_division_by_zero_is_an_infinity_not_an_error() {
    expect(
        &int_main(
            "float inf = 1.0 / 0.0\nfloat bigger = inf + 1000000.0\nif (bigger == inf) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    expect(
        &int_main("float ninf = (0.0 - 1.0) / 0.0\nif (ninf < 0.0) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
}

#[test]
fn infinity_minus_infinity_is_nan() {
    expect(
        &int_main(
            "float inf = 1.0 / 0.0\nfloat nan = inf - inf\nif (nan == nan) {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn nan_comparisons_are_all_false() {
    expect(
        &int_main(
            "float nan = 0.0 / 0.0\nif (nan < 1.0 || nan > 1.0 || nan <= 1.0 || nan >= 1.0) {\nreturn 1\n}\nreturn 0",
        ),
        Value::Int(0),
    );
}

#[test]
fn float_equality_follows_value_not_representation() {
    expect(
        &int_main("float a = 0.5\nfloat b = 0.25 + 0.25\nif (a == b) {\nreturn 0\n}\nreturn 1"),
        Value::Int(0),
    );
    // 0.1 + 0.2 != 0.3 in binary floating point.
    expect(
        &int_main("float a = 0.1 + 0.2\nfloat b = 0.3\nif (a == b) {\nreturn 1\n}\nreturn 0"),
        Value::Int(0),
    );
}

#[test]
fn float_overflow_becomes_infinity_not_an_error() {
    // A literal that would parse to infinity is a LEX error, so overflow
    // is reached by computation: doubling 1.0 about 1100 times saturates
    // at +inf. `x * 2.0 == x` distinguishes inf from any finite value.
    expect(
        &int_main(
            "float x = 1.0\nint i = 0\nwhile (i < 1100) {\nx *= 2.0\ni += 1\n}\nif (x * 2.0 == x) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
}

#[test]
fn float_and_int_stay_separate_domains() {
    // No implicit coercion in either direction — every mixing is a type
    // error (§2.4), including through assignment.
    check_one_error(
        "int main() {\nfloat f = 1.5\nint x = f\nreturn x\n}",
        "type mismatch in declaration of `x`: expected `int`, found `float`",
    );
    check_one_error(
        "int main() {\nint x = 1\nfloat f = x\nreturn 0\n}",
        "type mismatch in declaration of `f`: expected `float`, found `int`",
    );
    check_one_error(
        "int main() {\nint x = 1\nfloat f = 2.0\nif (x == f) {\nreturn 1\n}\nreturn 0\n}",
        "cannot apply `==` to `int` and `float`",
    );
}

#[test]
fn compound_assignment_keeps_float_domains() {
    expect(
        &int_main(
            "float f = 1.5\nf += 0.25\nf *= 2.0\nf /= 2.0\nf -= 0.25\nif (f == 1.5) {\nreturn 0\n}\nreturn 1",
        ),
        Value::Int(0),
    );
    check_one_error(
        "int main() {\nint x = 1\nx += 0.5\nreturn x\n}",
        "cannot apply `+=` to `int` and `float`",
    );
}
