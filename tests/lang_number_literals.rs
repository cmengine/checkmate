//! Number-literal suite: the §2 numeric grammar end to end — radix
//! prefixes (hex `0x`, octal `0o`, binary `0b`), `_` digit separators,
//! float exponents, and the collision rules that keep every non-number
//! digit-led word an identifier. Every value flows through the full
//! pipeline (`parse_source` + `check` + the tree walker), and every
//! diagnostic is checked for exact count and message.

use cme_compiler::check::check;
use cme_compiler::lexer::{LexError, lex, lex_with_errors};
use cme_compiler::parse_source;
use cme_core::Span;
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

fn expect_float(source: &str, value: f64) {
    match run_main(source) {
        Ok(Value::Float(actual)) => assert!(
            (actual - value).abs() <= f64::EPSILON.max(value.abs() * 1e-12),
            "expected {value}, got {actual}; source: {source:?}"
        ),
        other => panic!("expected float {value}, got {other:?}; source: {source:?}"),
    }
}

fn expect_lex_error(source: &str, contains: &str) {
    let outcome = parse_source(source);
    assert!(
        !outcome.diagnostics.is_empty(),
        "expected a front-end diagnostic; source: {source:?}"
    );
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| d.to_string().contains(contains)),
        "diagnostics {:?} should mention {contains:?}",
        outcome.diagnostics
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

fn str_main(body: &str) -> String {
    format!("str main() {{\n{body}\n}}\n")
}

// ---------------------------------------------------------------------------
// Radix prefixes: values through the full pipeline
// ---------------------------------------------------------------------------

#[test]
fn hex_octal_and_binary_evaluate_to_decimal_values() {
    expect(&int_main("return 0xFF"), Value::Int(255));
    expect(&int_main("return 0x10"), Value::Int(16));
    expect(&int_main("return 0o777"), Value::Int(511));
    expect(&int_main("return 0b1010"), Value::Int(10));
}

#[test]
fn uppercase_prefixes_and_digits_are_accepted() {
    expect(&int_main("return 0XFF"), Value::Int(255));
    expect(&int_main("return 0Xff"), Value::Int(255));
    expect(&int_main("return 0B1010"), Value::Int(10));
    expect(&int_main("return 0O17"), Value::Int(15));
    expect(&int_main("return 0xAbCdEf"), Value::Int(11259375));
}

#[test]
fn i64_max_in_every_radix_runs() {
    expect(
        &int_main("return 9223372036854775807"),
        Value::Int(i64::MAX),
    );
    expect(&int_main("return 0x7FFFFFFFFFFFFFFF"), Value::Int(i64::MAX));
    expect(
        &int_main("return 0o777777777777777777777"),
        Value::Int(i64::MAX),
    );
    expect(
        &int_main("return 0b111111111111111111111111111111111111111111111111111111111111111"),
        Value::Int(i64::MAX),
    );
}

#[test]
fn radix_literals_participate_in_arithmetic() {
    expect(&int_main("return 0xFF + 0b1"), Value::Int(256));
    expect(&int_main("return 0o10 * 0x2"), Value::Int(16));
    expect(&int_main("return 0xFF / 0xF"), Value::Int(17));
    expect(&int_main("return 0xFF % 0o10"), Value::Int(7));
    expect(&int_main("return -0xFF"), Value::Int(-255));
    expect(&int_main("return -0b100 - -0b1"), Value::Int(-3));
}

#[test]
fn radix_literals_in_comparisons_and_match() {
    expect(
        &int_main("if (0xFF == 255) { return 1 }\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("int x = 0b110\nif (x < 0o10) { return 1 }\nreturn 0"),
        Value::Int(1),
    );
}

#[test]
fn radix_literals_in_collections_and_maps() {
    expect(
        &int_main("int[] a = [0x1, 0b10, 0o3]\nreturn a[1] + a[0] + a[2]"),
        Value::Int(6),
    );
    expect(
        &int_main("map<int, int> m = { 0xFF: 0b1 }\nreturn m[255]"),
        Value::Int(1),
    );
}

#[test]
fn radix_literals_drive_loops_and_structs() {
    expect(
        &int_main("int total = 0\nfor (int i in [0x1, 0x2, 0x3]) {\ntotal += i\n}\nreturn total"),
        Value::Int(6),
    );
    expect(
        "struct pair {\nint a\nint b\n}\nint main() {\npair p = pair(a: 0x10, b: 0b1)\nreturn p.a + p.b\n}",
        Value::Int(17),
    );
}

#[test]
fn separators_group_digits_in_every_form() {
    expect(&int_main("return 1_000_000"), Value::Int(1000000));
    expect(&int_main("return 0xFF_FF"), Value::Int(65535));
    expect(&int_main("return 0b1_0_1"), Value::Int(5));
    expect(&int_main("return 0o7_7"), Value::Int(63));
    expect(&int_main("return 12_345_678 + 1_0"), Value::Int(12345688));
    expect(
        &int_main("return 0x7F_FF_FF_FF_FF_FF_FF_FF"),
        Value::Int(i64::MAX),
    );
}

#[test]
fn separator_values_are_checked_against_i64() {
    expect(
        &int_main("return 9_223_372_036_854_775_807"),
        Value::Int(i64::MAX),
    );
}

#[test]
fn all_radix_spellings_denote_the_same_value() {
    expect(
        &int_main(
            "if (0b1010 == 0o12) { return 1 }
return 0",
        ),
        Value::Int(1),
    );
    expect(
        &int_main(
            "if (0xFF == 255) { return 1 }
return 0",
        ),
        Value::Int(1),
    );
    expect(
        &int_main(
            "if (0b1010 != 0o13) { return 1 }
return 0",
        ),
        Value::Int(1),
    );
}

#[test]
fn stringification_renders_the_value_not_the_spelling() {
    expect(
        &str_main(r#"return "mask: " + 0xFF"#),
        Value::Str("mask: 255".to_string()),
    );
    expect(
        &str_main(r#"return "bits: " + 0b101"#),
        Value::Str("bits: 5".to_string()),
    );
    expect(
        &str_main(r#"return "grouped: " + 1_000"#),
        Value::Str("grouped: 1000".to_string()),
    );
}

// ---------------------------------------------------------------------------
// Byte crystallization through radix literals
// ---------------------------------------------------------------------------

#[test]
fn byte_crystallizes_from_radix_literals_in_range() {
    // byte + byte stays byte (overflow-checked), so the sum of three
    // maxed bytes must widen one term at a time through an int
    // accumulator to escape the byte ceiling.
    expect(
        "int main() {\nbyte b = 0xFF\nbyte o = 0o377\nbyte x = 0b11111111\nint total = 0\ntotal += b\ntotal += o\ntotal += x\nreturn total\n}",
        Value::Int(765),
    );
}

#[test]
fn byte_out_of_range_radix_literal_is_a_compile_error() {
    check_one_error(
        "int main() {\nbyte b = 0x100\nreturn 0\n}",
        "byte literal out of range",
    );
    check_one_error(
        "int main() {\nbyte b = 0b111111111\nreturn 0\n}",
        "byte literal out of range",
    );
    check_one_error(
        "int main() {\nbyte b = 0o400\nreturn 0\n}",
        "byte literal out of range",
    );
}

#[test]
fn byte_widening_with_radix_literals() {
    // A direct literal beside a byte crystallizes to byte (checked add):
    // `b + 0xFF` overflows the byte ceiling at runtime.
    expect(
        &int_main("byte b = 0b10\nbyte c = 0b11\nreturn b + c"),
        Value::Int(5),
    );
    // A byte widens against a genuine int (§2.4): the variable form adds
    // as int, so the same ceiling disappears.
    expect(
        &int_main("byte b = 0b10\nint i = 0xFF\nreturn b + i"),
        Value::Int(257),
    );
}

#[test]
fn byte_from_separator_literal_crystallizes() {
    expect(&int_main("byte b = 2_5_5\nreturn b + 0"), Value::Int(255));
}

// ---------------------------------------------------------------------------
// Float exponents end to end
// ---------------------------------------------------------------------------

fn float_main(body: &str) -> String {
    format!("float main() {{\n{body}\n}}\n")
}

#[test]
fn exponent_forms_evaluate() {
    expect_float(&float_main("return 1e3"), 1000.0);
    expect_float(&float_main("return 1E3"), 1000.0);
    expect_float(&float_main("return 1.5e3"), 1500.0);
    expect_float(&float_main("return 2.5e-1"), 0.25);
    expect_float(&float_main("return 1e-3"), 0.001);
    expect_float(&float_main("return 1e+3"), 1000.0);
    expect_float(&float_main("return 3.25E+2"), 325.0);
}

#[test]
fn exponent_floats_in_arithmetic_and_comparisons() {
    expect_float(&float_main("return 1e2 + 2e2"), 300.0);
    expect_float(&float_main("return 1.5e3 / 1e2"), 15.0);
    expect(
        &int_main("if (1e3 == 1000.0) { return 1 }\nreturn 0"),
        Value::Int(1),
    );
    expect(
        &int_main("if (1e2 > 99.0 && 1e-1 < 0.2) { return 1 }\nreturn 0"),
        Value::Int(1),
    );
}

#[test]
fn exponent_floats_take_separators() {
    expect_float(&float_main("return 1_000.5e0"), 1000.5);
    expect_float(&float_main("return 1_0e1_0"), 1.0e11);
}

#[test]
fn infer_crystallizes_exponent_literals_to_float() {
    expect(
        "float main() {\ninfer x = 1e3\nreturn x\n}",
        Value::Float(1000.0),
    );
}

#[test]
fn infer_crystallizes_radix_integers_to_int() {
    expect(&int_main("infer x = 0xFF\nreturn x"), Value::Int(255));
    expect(&int_main("infer x = 1_000\nreturn x"), Value::Int(1000));
}

#[test]
fn an_exponent_literal_never_fills_a_byte_or_int() {
    check_one_error("int main() {\nbyte b = 1e2\nreturn 0\n}", "mismatch");
    check_one_error("int main() {\nint x = 1.5e2\nreturn 0\n}", "mismatch");
}

// ---------------------------------------------------------------------------
// Identifier fallbacks: the no-collision guarantees
// ---------------------------------------------------------------------------

#[test]
fn broken_numeric_words_stay_identifiers() {
    // `0b12` is one digit-led identifier (longest match), so the
    // declaration is an unknown-name error, never a partial literal.
    check_one_error(
        "int main() {\nint x = 0b12\nreturn x\n}",
        "unknown name `0b12`",
    );
    check_one_error(
        "int main() {\nint x = 0xZZ\nreturn x\n}",
        "unknown name `0xZZ`",
    );
    check_one_error(
        "int main() {\nint x = 0o8\nreturn x\n}",
        "unknown name `0o8`",
    );
    check_one_error(
        "int main() {\nint x = 1__0\nreturn x\n}",
        "unknown name `1__0`",
    );
    check_one_error("int main() {\nint x = 1_\nreturn x\n}", "unknown name `1_`");
}

#[test]
fn whitepaper_digit_led_idents_are_still_names() {
    expect(&int_main("int 3Vector = 7\nreturn 3Vector"), Value::Int(7));
    expect(&int_main("int 2D = 3\nreturn 2D + 0x1"), Value::Int(4));
    expect(&int_main("int 2_D = 1\nreturn 2_D"), Value::Int(1));
    expect(&int_main("int 123abc = 5\nreturn 123abc"), Value::Int(5));
}

#[test]
fn numeric_prefix_words_and_literals_coexist() {
    // `0xFFg` is a name, `0xFF` is a value: both can sit in one program
    // without colliding.
    expect(
        &int_main("int 0xFFg = 0xFF\nreturn 0xFFg + 0xFF"),
        Value::Int(510),
    );
    expect(
        &int_main("int 1e10x = 1\ninfer y = 1e10\nreturn 1e10x"),
        Value::Int(1),
    );
}

#[test]
fn a_broken_word_never_splits_across_a_dot() {
    // `1.length` stays member access on a literal: the checker reports
    // the field, not a parse error.
    check_one_error(
        "int main() {\nreturn 1.length\n}",
        "unknown field `length` on `int`",
    );
    check_one_error(
        "int main() {\nreturn 0xFF.length\n}",
        "unknown field `length` on `int`",
    );
}

#[test]
fn incomplete_exponents_degrade_to_float_then_ident() {
    // `1.5e` lexes as the float `1.5` followed by the identifier `e` —
    // the parser then reports the stray name, exactly as it would for any
    // two expressions on one line.
    let outcome = parse_source("int main() {\nreturn 1.5e.length\n}");
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| d.to_string().contains("identifier `e`")),
        "{:?}",
        outcome.diagnostics
    );
    // `1e` is a single digit-led identifier — never a broken literal.
    check_one_error("int main() {\nint x = 1e\nreturn x\n}", "unknown name `1e`");
}

// ---------------------------------------------------------------------------
// Compile-time literal overflow
// ---------------------------------------------------------------------------

#[test]
fn radix_overflow_is_a_lex_error_not_a_crash() {
    expect_lex_error(
        &int_main("return 0xFFFFFFFFFFFFFFFF"),
        "integer literal is too large",
    );
    expect_lex_error(
        &int_main("return 0b1000000000000000000000000000000000000000000000000000000000000000"),
        "integer literal is too large",
    );
    expect_lex_error(
        &int_main("return 0o2000000000000000000000"),
        "integer literal is too large",
    );
}

#[test]
fn decimal_and_separator_overflow_is_a_lex_error() {
    expect_lex_error(
        &int_main("return 99999999999999999999"),
        "integer literal is too large",
    );
    expect_lex_error(
        &int_main("return 9_999_999_999_999_999_999"),
        "integer literal is too large",
    );
}

#[test]
fn float_exponent_overflow_is_a_lex_error() {
    expect_lex_error(&int_main("return 1e999"), "float literal is too large");
    expect_lex_error(&int_main("return 1_0e999_9"), "float literal is too large");
}

#[test]
fn one_overflow_reports_exactly_one_diagnostic() {
    let outcome = parse_source(&int_main("return 0x8000000000000000"));
    assert_eq!(outcome.diagnostics.len(), 1, "{:?}", outcome.diagnostics);
    assert!(
        outcome.diagnostics[0]
            .to_string()
            .contains("integer literal is too large"),
        "{:?}",
        outcome.diagnostics[0]
    );
}

#[test]
fn lexer_reports_the_canonical_error_kinds() {
    let (_, errors) = lex_with_errors("0x8000000000000000");
    assert_eq!(
        errors,
        vec![LexError::IntegerOverflow {
            span: Span::new(0, 18)
        }]
    );
    let (_, errors) = lex_with_errors("1e999");
    assert_eq!(
        errors,
        vec![LexError::FloatOverflow {
            span: Span::new(0, 5)
        }]
    );
    let tokens = lex("0xFF").expect("valid hex lexes");
    assert!(matches!(
        tokens[0].token,
        cme_compiler::lexer::Token::IntLit(255)
    ));
}

// ---------------------------------------------------------------------------
// Mixed programs: the literal forms coexist with everything else
// ---------------------------------------------------------------------------

#[test]
fn a_realistic_mixed_program_runs() {
    expect(
        "struct pixel {\nbyte r\nbyte g\nbyte b\n}\nint main() {\n\
         pixel p = pixel(r: 0xFF, g: 0b10000000, b: 0o200)\n\
         int alpha = 0xFF\n\
         int total = alpha\n\
         total += p.r\n\
         total += p.g\n\
         total += p.b\n\
         return total\n}",
        Value::Int(766),
    );
}

#[test]
fn compound_assignment_accepts_every_form() {
    expect(
        &int_main("int x = 0b1\nx += 0xF\nx *= 2_0\nx -= 0b1\nx /= 0o17\nreturn x"),
        Value::Int(21),
    );
}

#[test]
fn hex_literals_survive_interpolation_islands() {
    expect(
        &str_main(
            r#"int hp = 0x10
return $"hp={hp + 0x1}""#,
        ),
        Value::Str("hp=17".to_string()),
    );
}

#[test]
fn enums_and_match_arms_take_radix_payloads() {
    expect(
        "enum level {\nLow(int v)\nHigh(int v)\n}\nint main() {\n\
         level l = level.High(0xFF)\n\
         int out = 0\n\
         match (l) {\nLow(int v) => { out = v }\nHigh(int v) => { out = v + 0b1 }\n}\n\
         return out\n}",
        Value::Int(256),
    );
}

#[test]
fn while_loop_counts_down_in_hex() {
    expect(
        &int_main(
            "int n = 0x8\nint total = 0\nwhile (n > 0x0) {\ntotal += n\nn -= 0b10\n}\nreturn total",
        ),
        Value::Int(20),
    );
}

#[test]
fn function_parameters_and_returns_take_radix_literals() {
    expect(
        "int addMask(int base) {\nreturn base + 0xFF\n}\nint main() {\nreturn addMask(0b1)\n}",
        Value::Int(256),
    );
}
