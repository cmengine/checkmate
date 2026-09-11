//! The root-facade view of the API: exactly the WHITEPAPER §13.1 shape,
//! compiled through the `cme` umbrella package rather than this crate.

use cme::{Engine, ExecutionLimits, Value};

#[test]
fn use_cme_engine_and_execution_limits_works() {
    let engine = Engine::new();
    let program = engine
        .load_source("int main() {\nreturn 40 + 2\n}\n")
        .expect("clean source compiles");

    let limits = ExecutionLimits {
        fuel: Some(1_000_000),
        deadline_ms: Some(50),
        max_call_depth: 64,
    };
    let context = engine.create_context(&program, limits);

    let answer = context.invoke("main", &[]).expect("bounded work completes");
    assert_eq!(answer, Value::Int(42));
}

#[test]
fn limits_enforced_through_the_facade() {
    let engine = Engine::new();
    let program = engine
        .load_source("int spin() {\nwhile (true) {\n}\nreturn 0\n}\n")
        .unwrap();
    let context = engine.create_context(
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
