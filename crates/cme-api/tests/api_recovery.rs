//! Invocation guards recover after nested calls, errors, and host unwinding.

use cme_api::{Engine, ErrorKind, ExecutionLimits, Value};

#[test]
fn indirect_reentry_is_rejected_without_disturbing_either_context() {
    use cme_api::{CapabilityProvider, Context};
    use std::sync::{Arc, Mutex};
    struct Relay {
        target: Mutex<Option<&'static Context<'static>>>,
        entry: &'static str,
        observed: Mutex<Option<ErrorKind>>,
    }
    impl CapabilityProvider for Relay {
        fn call(&self, _: &str, _: &[Value]) -> Result<Value, String> {
            let target = self.target.lock().unwrap().unwrap();
            match target.invoke(self.entry, &[]) {
                Ok(value) => Ok(value),
                Err(error) => {
                    *self.observed.lock().unwrap() = Some(error.kind);
                    Ok(Value::Int(17))
                }
            }
        }
    }
    fn context(relay: Arc<Relay>) -> &'static Context<'static> {
        let mut engine = Engine::new();
        engine
            .load_schema_text(
                "schema relay 1.0.0\nsince 1.0.0 capability bridge {\nint Call()\n}\n",
                "relay.cm",
            )
            .unwrap();
        engine.register_capability("relay.bridge", relay).unwrap();
        let program = Box::leak(Box::new(engine.load_source("import relay.bridge\nint entry() {\nreturn relay.bridge.Call()\n}\nint helper() {\nreturn 42\n}\n").unwrap()));
        Box::leak(Box::new(
            engine.create_context(program, ExecutionLimits::default()),
        ))
    }
    let ab = Arc::new(Relay {
        target: Mutex::new(None),
        entry: "entry",
        observed: Mutex::new(None),
    });
    let ba = Arc::new(Relay {
        target: Mutex::new(None),
        entry: "helper",
        observed: Mutex::new(None),
    });
    let a = context(ab.clone());
    let b = context(ba.clone());
    *ab.target.lock().unwrap() = Some(b);
    *ba.target.lock().unwrap() = Some(Box::leak(Box::new(a.clone())));
    assert_eq!(a.invoke("entry", &[]).unwrap(), Value::Int(17));
    assert_eq!(*ba.observed.lock().unwrap(), Some(ErrorKind::Reentrant));
    assert_eq!(*ab.observed.lock().unwrap(), None);
    for context in [a, b] {
        assert_eq!(context.invoke("helper", &[]).unwrap(), Value::Int(42));
    }
    // Release provider references to the other context after the test.
    *ab.target.lock().unwrap() = None;
    *ba.target.lock().unwrap() = None;
}

#[test]
fn extreme_deadline_does_not_panic() {
    let engine = Engine::new();
    let program = engine.load_source("int main() { return 42 }").unwrap();
    let context = engine.create_context(
        &program,
        ExecutionLimits {
            deadline_ms: Some(u64::MAX),
            ..Default::default()
        },
    );
    assert_eq!(context.invoke("main", &[]).unwrap(), Value::Int(42));
}

#[test]
fn failed_invocations_do_not_poison_the_context() {
    let engine = Engine::new();
    let program = engine
        .load_source("int fail() { return 1 / 0 }\nint ok() { return 42 }")
        .unwrap();
    let context = engine.create_context(&program, ExecutionLimits::default());
    for _ in 0..10 {
        assert_eq!(
            context.invoke("fail", &[]).unwrap_err().kind,
            ErrorKind::Runtime
        );
        assert_eq!(
            context.invoke("missing", &[]).unwrap_err().kind,
            ErrorKind::UnknownEntry
        );
        assert_eq!(
            context.invoke("ok", &[Value::Int(1)]).unwrap_err().kind,
            ErrorKind::Runtime
        );
        assert_eq!(context.invoke("ok", &[]).unwrap(), Value::Int(42));
    }
}

#[test]
fn panicking_provider_releases_the_context_guard() {
    use cme_api::CapabilityProvider;
    use std::sync::Arc;
    struct Panics;
    impl CapabilityProvider for Panics {
        fn call(&self, _: &str, _: &[Value]) -> Result<Value, String> {
            panic!("host panic")
        }
    }
    let mut engine = Engine::new();
    engine
        .load_schema_text(
            "schema panicHost 1.0.0\nsince 1.0.0 capability bridge {\nint Call()\n}\n",
            "panic.cm",
        )
        .unwrap();
    engine
        .register_capability("panicHost.bridge", Arc::new(Panics))
        .unwrap();
    let program = engine.load_source("import panicHost.bridge\nint entry() { return panicHost.bridge.Call() }\nint helper() { return 42 }").unwrap();
    let context = engine.create_context(&program, ExecutionLimits::default());
    for _ in 0..3 {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| context.invoke("entry", &[])))
                .is_err()
        );
        assert_eq!(context.invoke("helper", &[]).unwrap(), Value::Int(42));
    }
}
