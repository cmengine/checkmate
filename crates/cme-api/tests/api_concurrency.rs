//! Concurrency guarantees of the Rust host API (WHITEPAPER §1, §13):
//! programs and contexts are shareable, invocations race with nothing, and
//! fuel budgets never bleed between concurrent invocations.

use cme_api::{Engine, ExecutionLimits, Value};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn engine() -> Engine {
    Engine::new()
}

/// Leak a program for 'static borrows — bounded, test-only.
fn leak(source: &str) -> &'static cme_api::CompiledProgram {
    Box::leak(Box::new(engine().load_source(source).unwrap()))
}

/// Compile-time Send + Sync proof (fails to build if the guarantees break).
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Engine>();
    assert_send_sync::<cme_api::CompiledProgram>();
    assert_send_sync::<ExecutionLimits>();
};

#[test]
fn compiled_program_is_send_and_sync() {
    fn take<T: Send + Sync>(value: T) -> T {
        value
    }
    let program = take(engine().load_source("int f() {\nreturn 1\n}\n").unwrap());
    drop(program);
}

#[test]
fn one_context_many_threads_all_results_correct() {
    let program = leak("int add(int a, int b) {\nreturn a + b\n}\n");
    let ctx = Arc::new(engine().create_context(program, ExecutionLimits::default()));

    let mut handles = Vec::new();
    for thread in 0..8u64 {
        let ctx = Arc::clone(&ctx);
        handles.push(std::thread::spawn(move || {
            let mut seen = 0;
            for i in 0..50u64 {
                let a = (thread * 1000 + i) as i64;
                assert_eq!(
                    ctx.invoke("add", &[Value::Int(a), Value::Int(a)]),
                    Ok(Value::Int(a * 2))
                );
                seen += 1;
            }
            seen
        }));
    }
    for handle in handles {
        assert_eq!(handle.join().unwrap(), 50);
    }
}

#[test]
fn concurrent_fuel_budgets_are_independent() {
    // Every invocation gets its own fuel cell: N threads each running a
    // program that needs MORE than N× its own budget would fail if fuel
    // were shared, and succeed because it is not.
    let program =
        leak("int spin3() {\nint s = 0\nfor (int i in [1, 2, 3]) {\ns = s + i\n}\nreturn s\n}\n");
    let ctx = Arc::new(engine().create_context(
        program,
        ExecutionLimits {
            fuel: Some(100),
            ..ExecutionLimits::default()
        },
    ));

    let mut handles = Vec::new();
    for _ in 0..8 {
        let ctx = Arc::clone(&ctx);
        handles.push(std::thread::spawn(move || {
            for _ in 0..40 {
                assert_eq!(ctx.invoke("spin3", &[]), Ok(Value::Int(6)));
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn concurrent_failures_do_not_disturb_concurrent_successes() {
    // One thread hammers a failing entry point while others verify correct
    // results: neither side interferes with the other.
    let program = leak("int ok(int v) {\nreturn v + 1\n}\n");
    let ctx = Arc::new(engine().create_context(program, ExecutionLimits::default()));

    let fail_ctx = Arc::clone(&ctx);
    let failer = std::thread::spawn(move || {
        for _ in 0..200 {
            let _ = fail_ctx.invoke("missing", &[]);
        }
    });

    let successes = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for thread in 0..4 {
        let ctx = Arc::clone(&ctx);
        let successes = Arc::clone(&successes);
        handles.push(std::thread::spawn(move || {
            for i in 0..200 {
                assert_eq!(
                    ctx.invoke("ok", &[Value::Int(thread * 10_000 + i)]),
                    Ok(Value::Int(thread * 10_000 + i + 1))
                );
                successes.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    failer.join().unwrap();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(successes.load(Ordering::SeqCst), 800);
}

#[test]
fn concurrent_deadline_and_bounded_work_coexist() {
    // A thread spinning against a 100ms deadline alongside threads doing
    // bounded work: the spinner stops, the workers finish.
    let spin_program = leak(SPIN);
    let work_program = leak(WORK);

    let spin_ctx = engine().create_context(
        spin_program,
        ExecutionLimits {
            deadline_ms: Some(500),
            ..ExecutionLimits::default()
        },
    );
    let work_ctx = Arc::new(engine().create_context(work_program, ExecutionLimits::default()));

    let spinner = std::thread::spawn(move || spin_ctx.invoke("spin", &[]));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let work_ctx = Arc::clone(&work_ctx);
        workers.push(std::thread::spawn(move || {
            for _ in 0..100 {
                assert_eq!(work_ctx.invoke("work", &[]), Ok(Value::Int(45)));
            }
        }));
    }
    assert_eq!(
        spinner.join().unwrap().unwrap_err().message,
        "execution deadline exceeded (§5.5: the host's wall-clock deadline passed at a safepoint)"
    );
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn many_engines_and_programs_coexist() {
    // No global state anywhere: multiple engines, programs, and contexts
    // live at once and behave identically.
    let engines: Vec<_> = (0..4).map(|_| engine()).collect();
    let programs: Vec<&'static cme_api::CompiledProgram> = (0..4)
        .map(|_| leak("int five() {\nreturn 5\n}\n"))
        .collect();
    for engine in &engines {
        for program in &programs {
            let ctx = engine.create_context(program, ExecutionLimits::default());
            assert_eq!(ctx.invoke("five", &[]), Ok(Value::Int(5)));
        }
    }
}

const SPIN: &str = "int spin() {\nwhile (true) {\n}\nreturn 0\n}\n";
const WORK: &str = "int work() {\nint s = 0\nfor (int i in [1, 2, 3, 4, 5, 6, 7, 8, 9]) {\ns = s + i\n}\nreturn s\n}\n";
