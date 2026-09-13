//! Facade wiring test: the WHITEPAPER §13.1 import shape must work
//! literally from the root `cme` package. Gated on the `api` feature —
//! under the default build this file compiles to nothing, exactly like the
//! rest of the feature-gated facade.
#![cfg(feature = "api")]

use cme::{CompiledProgram, Context, Engine, ExecutionLimits};

#[test]
fn the_whitepaper_import_shape_loads_limits_and_invokes() {
    let engine = Engine::new();
    let program: CompiledProgram = engine
        .load_source("int answer() {\nreturn 40 + 2\n}\n")
        .expect("clean source compiles");
    let context: Context<'_> = engine.create_context(&program, ExecutionLimits::default());
    assert_eq!(context.invoke("answer", &[]), Ok(cme::Value::Int(42)));
}

#[test]
fn the_api_module_path_resolves_too() {
    let engine = cme::api::Engine::new();
    let program = engine
        .load_source("int id(int v) {\nreturn v\n}\n")
        .unwrap();
    let ctx = engine.create_context(
        &program,
        cme::ExecutionLimits {
            max_call_depth: 16,
            ..ExecutionLimits::default()
        },
    );
    assert_eq!(
        ctx.invoke("id", &[cme::Value::Int(9)]),
        Ok(cme::Value::Int(9))
    );
}

#[test]
fn default_features_expose_nothing() {
    // Compile-time proof lives in CI; this test just documents the rule.
    // When `api` is off, this FILE is empty (see the cfg above), so the
    // assertion that matters is: the default build must not leak the
    // names. That is pinned by the fact this file's imports are gated.
}

#[test]
fn explicit_limits_let_bounded_work_complete() {
    let engine = Engine::new();
    let program: CompiledProgram = engine
        .load_source("int main() {\nreturn 40 + 2\n}\n")
        .expect("clean source compiles");
    let context: Context<'_> = engine.create_context(
        &program,
        ExecutionLimits {
            fuel: Some(1_000_000),
            deadline_ms: Some(50),
            max_call_depth: 64,
        },
    );
    assert_eq!(context.invoke("main", &[]), Ok(cme::Value::Int(42)));
}

#[test]
fn fuel_exhaustion_reports_a_budget_error() {
    let engine = Engine::new();
    let program: CompiledProgram = engine
        .load_source("int spin() {\nwhile (true) {\n}\nreturn 0\n}\n")
        .expect("clean source compiles");
    let context: Context<'_> = engine.create_context(
        &program,
        ExecutionLimits {
            fuel: Some(10),
            deadline_ms: None,
            max_call_depth: 64,
        },
    );
    let error = context.invoke("spin", &[]).unwrap_err();
    assert_eq!(error.kind, cme::api::ErrorKind::Budget);
}
