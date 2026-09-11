//! §5.5 execution-limit behavior through the host API: fuel metering,
//! wall-clock deadlines, call-depth bounds, their defaults, their unset
//! conventions, and their interaction with one another.

use cme_api::{Engine, ErrorKind, ExecutionLimits, MAX_CALL_DEPTH, Value};

fn engine() -> Engine {
    Engine::new()
}

const SPIN: &str = "int spin() {\nint x = 0\nwhile (true) {\nx = x + 1\n}\nreturn x\n}\n";
const BOUNDED: &str = concat!(
    "int sum(int n) {\n",
    "int total = 0\n",
    "for (int i in [1, 2, 3, 4, 5, 6, 7, 8]) {\n",
    "total = total + i\n",
    "}\n",
    "return total\n",
    "}\n"
);

#[test]
fn default_limits_run_bounded_programs_unmetered() {
    let program = engine().load_source(BOUNDED).unwrap();
    let ctx = engine().create_context(&program, ExecutionLimits::default());
    assert_eq!(ctx.invoke("sum", &[Value::Int(8)]), Ok(Value::Int(36)));
    assert_eq!(ctx.limits(), &ExecutionLimits::default());
}

#[test]
fn fuel_bounds_a_runaway_loop_with_the_budget_kind() {
    let program = engine().load_source(SPIN).unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            fuel: Some(64),
            ..ExecutionLimits::default()
        },
    );
    let error = ctx.invoke("spin", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Budget);
    assert!(
        error.message.starts_with("fuel budget exhausted"),
        "{}",
        error.message
    );
    assert!(error.line >= 1, "the safepoint has a position");
}

#[test]
fn a_bounded_program_finishes_inside_a_generous_fuel_budget() {
    let program = engine().load_source(BOUNDED).unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            fuel: Some(1_000_000),
            ..ExecutionLimits::default()
        },
    );
    assert_eq!(ctx.invoke("sum", &[Value::Int(8)]), Ok(Value::Int(36)));
}

#[test]
fn zero_fuel_exhausts_at_the_first_safepoint() {
    // Rust semantics keep Option honesty: Some(0) really is zero.
    let program = engine().load_source("int f() {\nreturn 1\n}\n").unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            fuel: Some(0),
            ..ExecutionLimits::default()
        },
    );
    let error = ctx.invoke("f", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Budget);
}

#[test]
fn each_invocation_gets_a_fresh_fuel_allowance() {
    // The limit is per invocation: a context that exhausted once can
    // invoke again with a full allowance.
    let program = engine().load_source(SPIN).unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            fuel: Some(64),
            ..ExecutionLimits::default()
        },
    );
    assert_eq!(ctx.invoke("spin", &[]).unwrap_err().kind, ErrorKind::Budget);
    assert_eq!(ctx.invoke("spin", &[]).unwrap_err().kind, ErrorKind::Budget);
}

#[test]
fn an_expired_deadline_stops_immediately() {
    let program = engine().load_source(SPIN).unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            deadline_ms: Some(0),
            ..ExecutionLimits::default()
        },
    );
    let error = ctx.invoke("spin", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Deadline);
    assert!(
        error.message.starts_with("execution deadline exceeded"),
        "{}",
        error.message
    );
}

#[test]
fn a_wall_clock_deadline_stops_an_infinite_loop_in_time() {
    let program = engine().load_source(SPIN).unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            deadline_ms: Some(200),
            ..ExecutionLimits::default()
        },
    );
    let started = std::time::Instant::now();
    let error = ctx.invoke("spin", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Deadline);
    // The walker must stop at a safepoint — well within a test-friendly
    // multiple of the deadline, never hang.
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "deadline took {:?}",
        elapsed
    );
}

#[test]
fn a_deadline_never_interferes_with_bounded_work() {
    let program = engine().load_source(BOUNDED).unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            deadline_ms: Some(5_000),
            ..ExecutionLimits::default()
        },
    );
    assert_eq!(ctx.invoke("sum", &[Value::Int(8)]), Ok(Value::Int(36)));
}

#[test]
fn call_depth_bounds_recursion_and_reports_the_limit() {
    let program = engine()
        .load_source("int r(int n) {\nreturn r(n)\n}\n")
        .unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            max_call_depth: 10,
            ..ExecutionLimits::default()
        },
    );
    let error = ctx.invoke("r", &[Value::Int(0)]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::CallDepth);
    assert_eq!(error.message, "call depth limit of 10 exceeded");
}

#[test]
fn zero_call_depth_keeps_the_interpreter_default() {
    // 0 is the unset convention, not "no calls at all".
    let program = engine().load_source("int f() {\nreturn 7\n}\n").unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            max_call_depth: 0,
            ..ExecutionLimits::default()
        },
    );
    assert_eq!(ctx.invoke("f", &[]), Ok(Value::Int(7)));
}

#[test]
fn fuel_and_depth_and_deadline_compose() {
    // With all three set, whichever trips first wins: fuel here.
    let program = engine().load_source(SPIN).unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            fuel: Some(32),
            deadline_ms: Some(60_000),
            max_call_depth: 16,
        },
    );
    // spin has no calls: depth never trips; fuel trips long before the
    // 60-second deadline.
    assert_eq!(ctx.invoke("spin", &[]).unwrap_err().kind, ErrorKind::Budget);
}

#[test]
fn recursion_under_fuel_trips_the_budget_before_depth() {
    let program = engine()
        .load_source("int r(int n) {\nreturn r(n + 1)\n}\n")
        .unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            fuel: Some(50),
            deadline_ms: None,
            max_call_depth: 4_096,
        },
    );
    // Depth 4096 is far away; the fuel meter ends the run.
    assert_eq!(
        ctx.invoke("r", &[Value::Int(0)]).unwrap_err().kind,
        ErrorKind::Budget
    );
}

#[test]
fn limits_apply_to_member_invocations_too() {
    let source = concat!(
        "impl engine.gamemode {\n",
        "void Spin() {\nint x = 0\nwhile (true) {\nx = x + 1\n}\n}\n",
        "}\n"
    );
    let program = engine().load_source(source).unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            fuel: Some(64),
            ..ExecutionLimits::default()
        },
    );
    let error = ctx
        .invoke_member("engine.gamemode", "Spin", &[])
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Budget);
}

#[test]
fn the_default_depth_limit_is_the_interpreter_maximum() {
    assert_eq!(ExecutionLimits::default().max_call_depth, MAX_CALL_DEPTH);
    assert!(ExecutionLimits::default().fuel.is_none());
    assert!(ExecutionLimits::default().deadline_ms.is_none());
}

#[test]
fn limits_clone_and_compare() {
    let limits = ExecutionLimits {
        fuel: Some(10),
        deadline_ms: Some(20),
        max_call_depth: 30,
    };
    assert_eq!(limits, limits.clone());
    assert_ne!(limits, ExecutionLimits::default());
}
