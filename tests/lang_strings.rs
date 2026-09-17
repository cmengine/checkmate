//! String suite: §A.6 concatenation and canonical stringification, the
//! escape vocabulary at runtime, unicode content, and the `$"..."`/
//! interpolation surface. Value-level assertions pin the exact runtime
//! strings, including shapes a source-level comparison cannot express
//! (escapes decode to real control characters).

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

fn expect_str(body: &str, expected: &str) {
    let source = format!("str main() {{\n{body}\n}}\n");
    assert_eq!(
        run_main(&source),
        Ok(Value::Str(expected.to_string())),
        "body: {body:?}"
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

// ---------------------------------------------------------------------------
// §A.6: canonical stringification of every scalar
// ---------------------------------------------------------------------------

#[test]
fn int_stringification_is_decimal_with_a_minus_sign() {
    expect_str("return \"v\" + 0", "v0");
    expect_str("return \"v\" + 42", "v42");
    expect_str("return \"v\" + -123", "v-123");
    // The most negative int renders in full.
    expect_str(
        "return \"v\" + (0 - 9223372036854775807 - 1)",
        "v-9223372036854775808",
    );
}

#[test]
fn float_stringification_is_the_shortest_round_trip_form() {
    expect_str("return \"v\" + 0.5", "v0.5");
    expect_str("return \"v\" + 1.5", "v1.5");
    expect_str("return \"v\" + (0.1 + 0.2)", "v0.30000000000000004");
    expect_str("return \"v\" + 100000000000.0", "v100000000000");
    // A computed float uses the same canonical form.
    expect_str("return \"\" + (7.0 / 2.0)", "3.5");
    expect_str("return \"\" + (1.0 / 3.0)", "0.3333333333333333");
}

#[test]
fn non_finite_floats_stringify_as_named_values() {
    expect_str("return \"\" + (0.0 / 0.0)", "NaN");
    expect_str("return \"\" + (1.0 / 0.0)", "inf");
    expect_str("return \"\" + ((0.0 - 1.0) / 0.0)", "-inf");
    // Negative zero keeps its sign.
    expect_str("return \"\" + -0.0", "-0");
}

#[test]
fn bool_stringification_is_lowercase() {
    expect_str("return \"ok: \" + true", "ok: true");
    expect_str("return \"ok: \" + false", "ok: false");
}

#[test]
fn byte_stringification_matches_its_int_value() {
    expect_str("byte b = 200\nreturn \"v\" + b", "v200");
    expect_str("byte b = 0\nreturn \"v\" + b", "v0");
}

#[test]
fn str_operands_are_used_as_is() {
    expect_str("return \"a\" + \"b\"", "ab");
    expect_str("return \"\" + \"x\"", "x");
    expect_str("return \"x\" + \"\"", "x");
}

#[test]
fn concat_chains_are_left_associative_across_types() {
    // The §A.6 worked examples: the mixed forms are deterministic.
    expect_str("return \"a\" + 1 + 2", "a12");
    expect_str("return 1 + 2 + \"a\"", "3a");
    // Parenthesization changes the grouping and therefore the result.
    expect_str("return 1 + (2 + \"a\")", "12a");
    expect_str("return \"n=\" + (1 + 2)", "n=3");
}

// ---------------------------------------------------------------------------
// Escapes and literals at runtime
// ---------------------------------------------------------------------------

#[test]
fn escape_vocabulary_decodes_to_control_characters() {
    expect_str(r#"return "a\nb""#, "a\nb");
    expect_str(r#"return "a\tb""#, "a\tb");
    expect_str(r#"return "a\\b""#, "a\\b");
    expect_str(r#"return "a\"b""#, "a\"b");
    // Escapes compose inside longer content.
    expect_str(
        r#"return "line1\nline2\n\tindented""#,
        "line1\nline2\n\tindented",
    );
}

#[test]
fn unicode_content_round_trips_exactly() {
    expect_str(r#"return "日本語""#, "日本語");
    expect_str(r#"return "rocket: 🚀""#, "rocket: 🚀");
    expect_str(r#"return "ünïcödé" + " + " + "日本""#, "ünïcödé + 日本");
    expect_str(r#"return "α" + 1"#, "α1");
}

#[test]
fn string_equality_is_by_content() {
    expect_str(
        r#"if ("ab" == "a" + "b") {
    return "eq"
}
return "ne""#,
        "eq",
    );
    expect_str(
        r#"if ("a\nb" == "a" + "\n" + "b") {
    return "eq"
}
return "ne""#,
        "eq",
    );
}

#[test]
fn strings_build_in_loops() {
    expect_str(
        "str s = \"\"\nint i = 0\nwhile (i < 10) {\ns = s + (i % 10)\ni += 1\n}\nreturn s",
        "0123456789",
    );
    expect_str(
        "str s = \"\"\nfor (int v in [1, 2, 3]) {\ns += \"[\" + v + \"]\"\n}\nreturn s",
        "[1][2][3]",
    );
}

// ---------------------------------------------------------------------------
// Interpolation (§2.8-shaped islands)
// ---------------------------------------------------------------------------

#[test]
fn interpolation_renders_every_scalar_kind() {
    expect_str(r#"return $"{1}""#, "1");
    expect_str(r#"return $"{-5}""#, "-5");
    expect_str(r#"return $"{0 - 5}""#, "-5");
    expect_str(r#"return $"{2.5}""#, "2.5");
    expect_str(r#"return $"{true} {false}""#, "true false");
    expect_str(
        r#"str s = "x"
return $"{s}!""#,
        "x!",
    );
    expect_str(
        r#"byte b = 9
return $"{b}""#,
        "9",
    );
}

#[test]
fn interpolation_islands_evaluate_expressions() {
    expect_str(r#"return $"{1 + 2 * 3}""#, "7");
    expect_str(r#"return $"{(1 < 2)}""#, "true");
    expect_str(r#"return $"{1 == 2}""#, "false");
    expect_str(
        r#"int[] a = [7, 8]
return $"{a[0]} {a.length}""#,
        "7 2",
    );
    assert_eq!(
        run_main(
            "int add(int a, int b) {\n    return a + b\n}\nstr main() {\n    return $\"sum={add(2, 3)}\"\n}",
        ),
        Ok(Value::Str("sum=5".to_string())),
    );
}

#[test]
fn interpolation_supports_multiple_and_adjacent_islands() {
    expect_str(r#"return $"{1}-{2}-{3}""#, "1-2-3");
    expect_str(r#"return $"{1}{2}""#, "12");
    expect_str(r#"return $"a{$"b{$"c"}"}""#, "abc");
}

#[test]
fn interpolation_prefix_and_suffix_text_is_preserved() {
    expect_str(r#"return $"HP: {100} / {100}""#, "HP: 100 / 100");
    expect_str(r#"return $"x={1}!""#, "x=1!");
}

#[test]
fn interpolation_islands_must_be_scalars() {
    // Structs and enums do not stringify: an island carrying one is a
    // check error, not a runtime `[object]`-style surprise.
    check_one_error(
        "struct s2 { int v }\nstr main() {\ns2 x = s2(v: 1)\nreturn $\"{x}\"\n}",
        "cannot interpolate `s2`",
    );
    check_one_error(
        "enum e2 { A(int v) }\nstr main() {\ne2 x = e2.A(1)\nreturn $\"{x}\"\n}",
        "cannot interpolate `e2`",
    );
}

// ---------------------------------------------------------------------------
// Concatenation typing edges
// ---------------------------------------------------------------------------

#[test]
fn concat_rejects_non_stringifiable_operands() {
    // Arrays and maps never stringify (§A.6 lists scalars only).
    check_one_error(
        "str main() {\nint[] a = [1]\nreturn \"\" + a\n}",
        "cannot apply `+`",
    );
    check_one_error(
        "str main() {\nmap<str, int> m = {}\nreturn m + \"\"\n}",
        "cannot apply `+`",
    );
}

#[test]
fn concat_with_str_never_becomes_numeric() {
    // There is no conversion in the other direction: `+` with one str
    // operand is always concatenation, never addition.
    check_one_error(
        "int main() {\nstr s = \"1\"\nreturn s + 1\n}",
        "wrong return type in `main`: expected `int`, found `str`",
    );
}

#[test]
fn compound_assignment_stringifies_through_every_path() {
    expect_str(
        "str s = \"a\"\ns += \"b\"\ns += 1\ns += 2.5\ns += true\ns += \"\"\nreturn s",
        "ab12.5true",
    );
}
