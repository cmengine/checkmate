//! Layout suite (§A.8 and the file-level surface): newlines are significant
//! at statement boundaries and insignificant inside the innermost
//! parentheses; trailing operators continue expressions; leading binary
//! operators are errors; line endings and trailing content shape the file
//! surface. Driven end to end through `parse_source`.

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

fn parse_diagnostic_containing(source: &str, contains: &str) {
    let outcome = parse_source(source);
    assert!(
        !outcome.diagnostics.is_empty(),
        "expected a diagnostic, parsed clean: {source:?}"
    );
    if !contains.is_empty() {
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|error| error.to_string().contains(contains)),
            "diagnostics {:?} should mention {contains:?} (source: {source:?})",
            outcome
                .diagnostics
                .iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
        );
    }
}

fn int_main(body: &str) -> String {
    format!("int main() {{\n{body}\n}}\n")
}

fn program(decls: &str, body: &str) -> String {
    format!("{decls}\nint main() {{\n{body}\n}}\n")
}

// ---------------------------------------------------------------------------
// Trailing operators continue expressions (§A.8)
// ---------------------------------------------------------------------------

#[test]
fn trailing_operators_continue_across_newlines() {
    expect(&int_main("int total = 5 +\n3\nreturn total"), Value::Int(8));
    expect(
        &int_main("int a = 2\nint b = 3\nint d = a *\nb\nreturn d"),
        Value::Int(6),
    );
    expect(&int_main("return 1 +\n2 +\n3"), Value::Int(6));
}

#[test]
fn a_leading_unary_operator_is_allowed_after_a_trailing_operator() {
    // The §A.8 example: the leading `-` is unary, applied to b.
    expect(
        &int_main("int a = 5\nint b = 3\nint d = a +\n-b\nreturn d"),
        Value::Int(2),
    );
}

#[test]
fn leading_binary_operators_are_compile_errors() {
    parse_diagnostic_containing(
        &int_main("int a = 5\nint b = 3\nint d = a\n+b\nreturn d"),
        "",
    );
    // The next statement still parses: recovery keeps the boundary.
    let outcome = parse_source(&int_main("int a = 1\nint d = a\n+ 2\nreturn d"));
    assert!(!outcome.diagnostics.is_empty());
}

#[test]
fn newlines_inside_parentheses_are_insignificant() {
    expect(
        &program(
            "int add(int a, int b) {\nreturn a + b\n}",
            "return add(\n1,\n2\n)",
        ),
        Value::Int(3),
    );
    // An expression inside parens wraps across lines: the trailing
    // operator continues it, and the newline is not a boundary there.
    expect(
        &program(
            "int add(int a, int b) {\nreturn a + b\n}",
            "return add(\n1 +\n0,\n2)",
        ),
        Value::Int(3),
    );
}

#[test]
fn newlines_inside_brackets_stay_significant() {
    // Array literals treat newlines as element separators: both the
    // comma form and the bare-newline form parse.
    expect(&int_main("int[] a = [1,\n2]\nreturn a[1]"), Value::Int(2));
    expect(&int_main("int[] a = [1\n2]\nreturn a[1]"), Value::Int(2));
    // Map entries are newline-delimited inside braces (§2.6 style).
    expect(
        &int_main("map<str, int> m = {\n\"a\": 1\n\"b\": 2\n}\nreturn m[\"a\"] + m[\"b\"]"),
        Value::Int(3),
    );
}

#[test]
fn struct_literal_fields_wrap_freely_across_lines() {
    expect(
        &program(
            "struct pt { int x\nint y }",
            "pt v = pt(\nx: 1\ny: 2\n)\nreturn v.x + v.y",
        ),
        Value::Int(3),
    );
}

// ---------------------------------------------------------------------------
// Statement boundaries
// ---------------------------------------------------------------------------

#[test]
fn two_statements_on_one_line_are_rejected() {
    parse_diagnostic_containing(
        &int_main("int a = 1 int b = 2\nreturn a"),
        "end of statement",
    );
}

#[test]
fn return_stays_restricted_to_its_line() {
    // `return` followed by a newline returns nothing, even when an
    // expression could continue.
    parse_diagnostic_containing("int f() {\nreturn\n1\n}\nint main() {\nreturn 0\n}", "");
}

#[test]
fn blank_lines_and_trailing_whitespace_are_invisible() {
    expect(
        &int_main("\n\nint a = 1   \n\n\t\nint b = 2\n\nreturn a + b\n\n"),
        Value::Int(3),
    );
}

#[test]
fn comments_do_not_fuse_adjacent_statements() {
    expect(
        &int_main("int a = 1 // trailing comment\n// full line\nint b = 2\nreturn a + b"),
        Value::Int(3),
    );
}

// ---------------------------------------------------------------------------
// File-level surface
// ---------------------------------------------------------------------------

#[test]
fn crlf_line_endings_parse_and_run() {
    let source = "int main() {\r\nint x = 5\r\nreturn x\r\n}\r\n";
    expect(source, Value::Int(5));
}

#[test]
fn cr_only_line_endings_parse_and_run() {
    let source = "int main() {\rint x = 5\rreturn x\r}";
    expect(source, Value::Int(5));
}

#[test]
fn a_file_without_a_trailing_newline_is_complete() {
    expect("int main() {\nreturn 4\n}", Value::Int(4));
}

#[test]
fn an_empty_file_parses_to_nothing() {
    let outcome = parse_source("");
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    assert!(outcome.statements.is_empty());
}

#[test]
fn comments_only_files_parse_to_nothing() {
    let outcome = parse_source("// nothing here\n/* really nothing */\n");
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    assert!(outcome.statements.is_empty());
}
