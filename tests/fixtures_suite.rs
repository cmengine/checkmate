//! The fixture suite: every file under `tests/fixtures/` runs through the
//! full front-end pipeline (and the tree-walking interpreter where the file
//! is a healthy program), with per-directory contracts:
//!
//! - `programs/*.cm` parse and check CLEAN and every one is self-checking:
//!   `main()` returns 0 exactly when all of the fixture's internal pins
//!   hold, so the runner asserts `Ok(Value::Int(0))`.
//! - `recovery/*.cm` are damaged on purpose: the pipeline must report a
//!   NON-EMPTY diagnostic list, never panic, and every char-boundary prefix
//!   of every file must also survive the pipeline without panicking.
//! - `legacy/test.cm` (front-end fixture) parses with zero diagnostics;
//!   `legacy/mixed.cm` is a demo that must stay non-clean.

use std::path::Path;

use cme_compiler::check::check;
use cme_interp::{InterpError, Interpreter, Value};

const FIXTURES: &str = "tests/fixtures";

fn fixture_dir(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURES)
        .join(name)
}

fn cm_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("fixture dir {:?} readable: {error}", dir))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "cm"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "{:?} must contain at least one fixture",
        dir
    );
    files
}

/// Parses and checks `source`, returning the combined diagnostics.
fn diagnose(source: &str) -> Vec<String> {
    let outcome = cme_compiler::parse_source(source);
    let mut messages: Vec<String> = outcome
        .diagnostics
        .iter()
        .map(|error| error.message().to_string())
        .collect();
    messages.extend(check(&outcome.statements).iter().map(|e| e.to_string()));
    messages
}

/// Every healthy program fixture is self-checking: clean compile, `main`
/// returns 0.
#[test]
fn program_fixtures_run_and_self_verify() {
    for path in cm_files(&fixture_dir("programs")) {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{path:?} readable: {error}"));
        let messages = diagnose(&source);
        assert!(
            messages.is_empty(),
            "{path:?} must check clean: {messages:?}"
        );

        let outcome = cme_compiler::parse_source(&source);
        let interpreter = Interpreter::new(&outcome.statements);
        let result = interpreter
            .invoke("main", &[])
            .unwrap_or_else(|error| panic!("{path:?} must run: {error:?}"));
        assert_eq!(
            result,
            Value::Int(0),
            "{path:?} self-check pins failed (main returned {result:?})"
        );
    }
}

/// Every recovery fixture stays damaged: non-empty diagnostics, no panic.
#[test]
fn recovery_fixtures_report_and_survive() {
    for path in cm_files(&fixture_dir("recovery")) {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{path:?} readable: {error}"));
        let messages = diagnose(&source);
        assert!(
            !messages.is_empty(),
            "{path:?} is a recovery fixture: it must report damage"
        );
    }
}

/// The truncation property extends over the whole fixtures tree: every
/// char-boundary prefix of every fixture parses + checks without panicking,
/// and a clean prefix may run `main` (missing main / limits are normal
/// outcomes — only a panic fails).
#[test]
fn every_fixture_prefix_survives_the_pipeline() {
    for name in ["programs", "recovery", "legacy"] {
        for path in cm_files(&fixture_dir(name)) {
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{path:?} readable: {error}"));
            for end in 0..=source.len() {
                if !source.is_char_boundary(end) {
                    continue;
                }
                let prefix = &source[..end];
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
}

/// Legacy pins preserved across the move into the fixtures tree. test.cm is
/// a FRONT-END fixture (its statements predate functions and the checker's
/// top-level rule, so only lexer/parser/strip diagnostics are pinned);
/// mixed.cm is a check-demo that must stay non-clean.
#[test]
fn legacy_fixtures_keep_their_pinned_behavior() {
    let test_cm = std::fs::read_to_string(fixture_dir("legacy").join("test.cm"))
        .expect("legacy/test.cm readable");
    let outcome = cme_compiler::parse_source(&test_cm);
    assert!(
        outcome.diagnostics.is_empty(),
        "legacy/test.cm must parse clean: {:?}",
        outcome
            .diagnostics
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
    );

    let mixed_cm = std::fs::read_to_string(fixture_dir("legacy").join("mixed.cm"))
        .expect("legacy/mixed.cm readable");
    let messages = diagnose(&mixed_cm);
    assert!(
        !messages.is_empty(),
        "legacy/mixed.cm is a check-demo: it must stay non-clean"
    );
}

/// The runner rejects an accidental runtime error shape: if a program
/// fixture ever invokes a runtime failure, that is a fixture bug.
#[test]
fn program_fixture_runtime_errors_are_fixture_bugs() {
    for path in cm_files(&fixture_dir("programs")) {
        let source = std::fs::read_to_string(&path).expect("readable");
        let outcome = cme_compiler::parse_source(&source);
        let diagnostics = check(&outcome.statements);
        assert!(diagnostics.is_empty(), "{path:?}: {diagnostics:?}");
        let interpreter = Interpreter::new(&outcome.statements);
        let error: Option<InterpError> = match interpreter.invoke("main", &[]) {
            Ok(Value::Int(0)) => None,
            Ok(other) => panic!("{path:?}: main returned {other:?} — a fixture pin failed"),
            Err(error) => Some(error),
        };
        assert!(
            error.is_none(),
            "{path:?} hit a runtime error: {:?}",
            error.map(|e| e.message)
        );
    }
}
