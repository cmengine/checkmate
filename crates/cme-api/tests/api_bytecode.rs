//! The host pipeline compiles once and meters fresh instruction budgets.

use cme_api::{Engine, ErrorKind, ExecutionLimits, Value};

#[test]
fn programs_reuse_verified_bytecode_and_each_call_has_fresh_fuel() {
    let engine = Engine::new();
    let program = engine.load_source("int main() {\nreturn 42\n}\n").unwrap();
    assert_eq!(program.bytecode().function_count(), 1);
    assert_eq!(program.bytecode().instruction_count(), 2);
    let context = engine.create_context(
        &program,
        ExecutionLimits {
            fuel: Some(2),
            ..Default::default()
        },
    );
    for _ in 0..3 {
        assert_eq!(context.invoke("main", &[]).unwrap(), Value::Int(42));
    }
    let context = engine.create_context(
        &program,
        ExecutionLimits {
            fuel: Some(1),
            ..Default::default()
        },
    );
    for _ in 0..3 {
        let error = context.invoke("main", &[]).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Budget);
        assert_eq!(error.line, 2);
        assert!(error.message.contains("instruction count"));
    }
}

#[test]
fn host_recursion_needs_no_large_native_stack() {
    let engine = Engine::new();
    let program = engine
        .load_source("int count(int n) {\nif (n == 0) { return 0 }\nreturn 1 + count(n - 1)\n}\n")
        .unwrap();
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || {
            let context = engine.create_context(&program, ExecutionLimits::default());
            assert_eq!(
                context.invoke("count", &[Value::Int(1000)]).unwrap(),
                Value::Int(1000)
            );
            assert_eq!(
                context
                    .invoke("count", &[Value::Int(1500)])
                    .unwrap_err()
                    .kind,
                ErrorKind::CallDepth
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
