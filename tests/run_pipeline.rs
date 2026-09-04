//! End-to-end integration tests for the root `cme` package: the full
//! front-end pipeline plus the tree-walking interpreter, exactly as the
//! CLI's `run` command drives it (parse, check, then invoke `main` with no
//! arguments — never running code that produced a diagnostic).

use cme_compiler::check::check;
use cme_interp::{InterpError, Interpreter, Value};

const BASIC_CM: &str = include_str!("../basic.cm");
const BOOM_CM: &str = include_str!("../boom.cm");

/// The pipeline the `run` command drives. Panics on any compile-stage
/// diagnostic so these tests only ever execute programs the gate accepts.
fn run_main(source: &str) -> Result<Value, InterpError> {
    let outcome = cme_compiler::parse_source(source);
    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(check(&outcome.statements));
    assert!(
        diagnostics.is_empty(),
        "the pipeline only runs clean programs: {diagnostics:#?}"
    );
    let interpreter = Interpreter::new(&outcome.statements);
    interpreter.invoke("main", &[])
}

#[test]
fn basic_cm_runs_end_to_end_and_returns_three() {
    // Hand-derived from basic.cm: fib(0..4) sums to 7 (0+1+1+2+3), x2 = 14,
    // -3 = 11, /3 truncates to 3, % 4 = 3; weight = 2.5 x 1.5 = 3.75 > 2.0
    // so status becomes "start checkmate heavy". main returns the total.
    assert_eq!(run_main(BASIC_CM), Ok(Value::Int(3)));
}

#[test]
fn prefix_truncation_never_panics_the_pipeline_or_the_interpreter() {
    // Extends the front-end truncation property through execution: for
    // every char-boundary prefix of every fixture, parse + check must
    // never panic, and the interpreter runs `main` whenever the prefix is
    // clean. Runtime errors (a prefix without `main`, a truncated
    // computation hitting a limit) are normal outcomes; only a panic
    // fails the property.
    for fixture in [BASIC_CM, BOOM_CM] {
        for end in 0..=fixture.len() {
            if !fixture.is_char_boundary(end) {
                continue;
            }
            let prefix = &fixture[..end];
            let outcome = cme_compiler::parse_source(prefix);
            let mut diagnostics = outcome.diagnostics;
            diagnostics.extend(check(&outcome.statements));
            if diagnostics.is_empty() {
                let interpreter = Interpreter::new(&outcome.statements);
                let _ = interpreter.invoke("main", &[]);
            }
        }
    }
}
