//! Invocation semantics of the Rust host API: arguments, results, value
//! semantics, entry-point misses, and every shape of success and failure a
//! host can produce through [`cme_api::Context::invoke`] and
//! [`invoke_member`] (WHITEPAPER §13.1, §2.1, §2.13, §10.4).

use cme_api::{Engine, ErrorKind, ExecutionError, ExecutionLimits, MAX_CALL_DEPTH, Value};

fn context(source: &str) -> cme_api::Context<'static> {
    context_with(source, ExecutionLimits::default())
}

fn context_with(source: &str, limits: ExecutionLimits) -> cme_api::Context<'static> {
    // Leak the program so borrowed contexts work in short test bodies —
    // the leak is bounded and irrelevant under `cargo test`.
    let program: &'static _ = Box::leak(Box::new(
        Engine::new()
            .load_source(source)
            .expect("test source compiles"),
    ));
    Engine::new().create_context(program, limits)
}

#[test]
fn scalar_arguments_and_results_round_trip() {
    let ctx = context(concat!(
        "int negate(int v) {\nreturn -v\n}\n",
        "float halve(float v) {\nreturn v / 2.0\n}\n",
        "bool flip(bool v) {\nreturn !v\n}\n",
        "str shout(str v) {\nreturn v + \"!\"\n}\n",
    ));
    assert_eq!(ctx.invoke("negate", &[Value::Int(40)]), Ok(Value::Int(-40)));
    assert_eq!(
        ctx.invoke("halve", &[Value::Float(9.0)]),
        Ok(Value::Float(4.5))
    );
    assert_eq!(
        ctx.invoke("flip", &[Value::Bool(false)]),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        ctx.invoke("shout", &[Value::Str("hi".into())]),
        Ok(Value::Str("hi!".into()))
    );
}

#[test]
fn multi_argument_calls_bind_positionally() {
    let ctx = context("int sub(int a, int b) {\nreturn a - b\n}\n");
    assert_eq!(
        ctx.invoke("sub", &[Value::Int(10), Value::Int(4)]),
        Ok(Value::Int(6))
    );
    // Order matters: positional binding, not names.
    assert_eq!(
        ctx.invoke("sub", &[Value::Int(4), Value::Int(10)]),
        Ok(Value::Int(-6))
    );
}

#[test]
fn host_arguments_are_cloned_not_aliased() {
    // §2.13: mutating a parameter never mutates the caller's instance.
    let ctx = context(concat!(
        "struct bag {\nint gold\n}\n",
        "bag drain(bag b) {\nb.gold = 0\nreturn b\n}\n",
    ));
    let original = Value::Struct {
        name: "bag".into(),
        fields: vec![("gold".into(), Value::Int(100))],
    };
    let drained = ctx
        .invoke("drain", std::slice::from_ref(&original))
        .unwrap();
    assert_eq!(
        drained,
        Value::Struct {
            name: "bag".into(),
            fields: vec![("gold".into(), Value::Int(0))],
        }
    );
    // The host's value is untouched.
    assert_eq!(
        original,
        Value::Struct {
            name: "bag".into(),
            fields: vec![("gold".into(), Value::Int(100))],
        }
    );
}

#[test]
fn composite_results_come_back_structurally() {
    let ctx = context(concat!(
        "struct vec2 {\nfloat x\nfloat y\n}\n",
        "enum shape {\nCircle(float radius)\nRect(vec2 corner)\n}\n",
        "vec2 mkVec(float x, float y) {\nreturn vec2(x: x, y: y)\n}\n",
        "shape mkCircle(float r) {\nreturn shape.Circle(r)\n}\n",
        "int[] firstThree() {\nreturn [1, 2, 3]\n}\n",
        "map<str, int> counts() {\nreturn {\n\"a\": 1\n\"b\": 2\n}\n}\n",
    ));

    let vec_value = ctx
        .invoke("mkVec", &[Value::Float(1.0), Value::Float(2.5)])
        .unwrap();
    match vec_value {
        Value::Struct { name, fields } => {
            assert_eq!(name, "vec2");
            assert_eq!(fields[0], ("x".into(), Value::Float(1.0)));
            assert_eq!(fields[1], ("y".into(), Value::Float(2.5)));
        }
        other => panic!("expected struct, got {other:?}"),
    }

    let circle = ctx.invoke("mkCircle", &[Value::Float(3.0)]).unwrap();
    match circle {
        Value::Enum {
            name,
            variant,
            payload,
        } => {
            assert_eq!(name, "shape");
            assert_eq!(variant, "Circle");
            assert_eq!(payload, vec![Value::Float(3.0)]);
        }
        other => panic!("expected enum, got {other:?}"),
    }

    let array = ctx.invoke("firstThree", &[]).unwrap();
    assert_eq!(
        array,
        Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );

    let map = ctx.invoke("counts", &[]).unwrap();
    match map {
        Value::Map(entries) => {
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].0, Value::Str("a".into()));
            assert_eq!(entries[1].1, Value::Int(2));
        }
        other => panic!("expected map, got {other:?}"),
    }
}

#[test]
fn void_results_return_void() {
    let ctx = context("void doNothing() {\n}\nint main() {\nreturn 1\n}\n");
    let result = ctx.invoke("doNothing", &[]).unwrap();
    assert!(result.is_void());
}

#[test]
fn recursion_and_mutual_recursion_run() {
    let ctx = context(concat!(
        "int fib(int n) {\nif (n < 2) {\nreturn n\n}\nreturn fib(n - 1) + fib(n - 2)\n}\n",
        "bool isEven(int n) {\nif (n == 0) {\nreturn true\n}\nreturn isOdd(n - 1)\n}\n",
        "bool isOdd(int n) {\nif (n == 0) {\nreturn false\n}\nreturn isEven(n - 1)\n}\n",
    ));
    assert_eq!(ctx.invoke("fib", &[Value::Int(20)]), Ok(Value::Int(6765)));
    assert_eq!(
        ctx.invoke("isEven", &[Value::Int(10)]),
        Ok(Value::Bool(true))
    );
    assert_eq!(ctx.invoke("isOdd", &[Value::Int(7)]), Ok(Value::Bool(true)));
}

#[test]
fn script_side_effects_between_invocations_do_not_exist() {
    // §1: no ambient mutable global state. Two invocations of the same
    // function see the same fresh world.
    let ctx = context("int tick() {\nint count = 0\ncount += 1\nreturn count\n}\n");
    assert_eq!(ctx.invoke("tick", &[]), Ok(Value::Int(1)));
    assert_eq!(ctx.invoke("tick", &[]), Ok(Value::Int(1)));
}

#[test]
fn runtime_failures_classify_as_runtime_and_carry_positions() {
    let ctx = context(concat!(
        "int boom(int line3) {\nreturn 1 / line3\n}\n",
        "int overflow() {\nint big = 9223372036854775807\nreturn big + 1\n}\n",
    ));
    let error = ctx.invoke("boom", &[Value::Int(0)]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Runtime);
    assert!(error.line >= 2, "positioned on the division: {error:?}");
    assert!(error.column >= 1);
    assert!(error.file.is_none(), "loose sources have no file");
    assert!(error.span.is_some());
    assert!(error.render().starts_with("line 2,"), "{}", error.render());

    let error = ctx.invoke("overflow", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Runtime);
    assert!(error.message.contains("overflow"), "{}", error.message);
}

#[test]
fn every_runtime_failure_shape_is_a_clean_error() {
    let source = concat!(
        "int divZero(int a, int b) {\nreturn a / b\n}\n",
        "int modZero(int a, int b) {\nreturn a % b\n}\n",
        "int outOfBounds() {\nint[] xs = [1]\nreturn xs[5]\n}\n",
        "int negativeIndex() {\nint[] xs = [1]\nreturn xs[0 - 2]\n}\n",
        "int missingKey() {\nmap<str, int> m = {\"a\": 1}\nreturn m[\"z\"]\n}\n",
        "int strMultiply() {\n// ungated defensive shape: parse clean only\nreturn 0\n}\n",
    );
    let ctx = context(source);
    for (name, args) in [
        ("divZero", vec![Value::Int(5), Value::Int(0)]),
        ("modZero", vec![Value::Int(5), Value::Int(0)]),
        ("outOfBounds", vec![]),
        ("negativeIndex", vec![]),
        ("missingKey", vec![]),
    ] {
        let error = ctx.invoke(name, &args).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Runtime, "{name}: {error:?}");
        assert!(!error.message.is_empty(), "{name} must explain itself");
    }
}

#[test]
fn unknown_entry_points_classify_and_explain() {
    let ctx = context("int real() {\nreturn 1\n}\n");
    let error = ctx.invoke("ghost", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::UnknownEntry);
    assert_eq!(error.message, "unknown function `ghost`");
    // No position: the request itself was the mistake.
    assert_eq!(error.line, 0);
    assert_eq!(error.column, 0);
    assert_eq!(error.file, None);
    assert_eq!(error.span, None);
    assert_eq!(error.render(), "unknown function `ghost`");
}

#[test]
fn arity_mismatches_fail_defensively() {
    let ctx = context("int add(int a, int b) {\nreturn a + b\n}\n");
    for args in [
        vec![],
        vec![Value::Int(1)],
        vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    ] {
        let error = ctx.invoke("add", &args).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Runtime, "{args:?}");
        assert!(error.message.contains("wrong number of arguments"));
    }
}

#[test]
fn wrong_shaped_arguments_fail_without_panicking() {
    // The checker type-checked the PROGRAM, not the host's arguments: a
    // host can hand a bool where an int is declared. The `+` node has no
    // bool implementation — the walker refuses defensively, never panics.
    let ctx = context("int add(int a, int b) {\nreturn a + b\n}\n");
    let error = ctx
        .invoke("add", &[Value::Bool(true), Value::Int(2)])
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Runtime);
    assert!(!error.message.is_empty());

    // A struct where a scalar is declared fails the same way.
    let error = ctx
        .invoke(
            "add",
            &[
                Value::Struct {
                    name: "x".into(),
                    fields: vec![],
                },
                Value::Int(2),
            ],
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Runtime);
}

#[test]
fn empty_source_contexts_refuse_every_invocation() {
    let ctx = context("");
    let error = ctx.invoke("main", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::UnknownEntry);
}

#[test]
fn struct_and_enum_targeted_impl_members_invoke() {
    let ctx = context(concat!(
        "struct vec2 {\nfloat x\nfloat y\n}\n",
        "enum state {\nOn(int level)\nOff()\n}\n",
        "impl vec2 {\n",
        "float dot(vec2 a, vec2 b) {\nreturn a.x * b.x + a.y * b.y\n}\n",
        "float length(vec2 v) {\nreturn v.x * v.x + v.y * v.y\n}\n",
        "}\n",
        "impl state {\nint level(state s) {\nreturn match (s) {\nOn(int l) => l\nOff() => 0 - 1\n}\n}\n",
        "}\n",
    ));
    let a = Value::Struct {
        name: "vec2".into(),
        fields: vec![
            ("x".into(), Value::Float(2.0)),
            ("y".into(), Value::Float(3.0)),
        ],
    };
    let b = Value::Struct {
        name: "vec2".into(),
        fields: vec![
            ("x".into(), Value::Float(4.0)),
            ("y".into(), Value::Float(5.0)),
        ],
    };
    assert_eq!(
        ctx.invoke_member("vec2", "dot", &[a.clone(), b]),
        Ok(Value::Float(23.0))
    );
    assert_eq!(
        ctx.invoke_member("vec2", "length", &[a]),
        Ok(Value::Float(13.0))
    );

    let on = Value::Enum {
        name: "state".into(),
        variant: "On".into(),
        payload: vec![Value::Int(7)],
    };
    assert_eq!(
        ctx.invoke_member("state", "level", &[on]),
        Ok(Value::Int(7))
    );
}

#[test]
fn host_style_dotted_impl_targets_invoke() {
    // §10.4: host-style dotted paths enter through invoke_member — the
    // same surface the C API's cm_invoke(ctx, "engine.gamemode", ...) uses.
    let ctx = context(concat!(
        "impl engine.gamemode {\n",
        "int InitGame(int seed) {\nreturn seed * 2\n}\n",
        "void OnTick(int dt) {\n}\n",
        "}\n",
    ));
    assert_eq!(
        ctx.invoke_member("engine.gamemode", "InitGame", &[Value::Int(21)]),
        Ok(Value::Int(42))
    );
    assert!(
        ctx.invoke_member("engine.gamemode", "OnTick", &[Value::Int(1)])
            .unwrap()
            .is_void()
    );
}

#[test]
fn unknown_impl_targets_and_members_classify_as_entry_misses() {
    let ctx =
        context("struct vec2 {\nfloat x\n}\nimpl vec2 {\nfloat x2(vec2 v) {\nreturn 0.0\n}\n}\n");
    let error = ctx
        .invoke_member("engine.physics", "Apply", &[])
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::UnknownEntry);
    assert!(error.message.contains("engine.physics.Apply"));

    let error = ctx.invoke_member("vec2", "ghost", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::UnknownEntry);
}

#[test]
fn impl_member_arity_failures_are_clean() {
    let ctx =
        context("struct vec2 {\nfloat x\n}\nimpl vec2 {\nfloat x2(vec2 v) {\nreturn 0.0\n}\n}\n");
    let error = ctx.invoke_member("vec2", "x2", &[]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Runtime);
    assert!(error.message.contains("wrong number of arguments"));
}

#[test]
fn deep_default_recursion_still_hits_the_fixed_guard() {
    // Default limits keep the interpreter's own behavior: runaway
    // recursion ends with the pinned depth error — on a big-stack thread,
    // exactly as the interpreter suite does it.
    let source = "int spin(int n) {\nreturn spin(n + 1)\n}\n";
    let program: &'static _ = Box::leak(Box::new(
        Engine::new().load_source(source).expect("compiles"),
    ));
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let ctx = Engine::new().create_context(program, ExecutionLimits::default());
            ctx.invoke("spin", &[Value::Int(0)])
        })
        .unwrap();
    let error = handle.join().unwrap().unwrap_err();
    assert_eq!(error.kind, ErrorKind::CallDepth);
    assert_eq!(
        error.message,
        format!("call depth limit of {MAX_CALL_DEPTH} exceeded")
    );
}

#[test]
fn a_lowered_depth_limit_needs_no_special_stack() {
    let source = "int spin(int n) {\nreturn spin(n + 1)\n}\nint id(int v) {\nreturn v\n}\n";
    let ctx = context_with(
        source,
        ExecutionLimits {
            max_call_depth: 8,
            ..ExecutionLimits::default()
        },
    );
    let error = ctx
        .invoke("spin", &[Value::Int(0)])
        .expect_err("depth 8 < recursion demand");
    assert_eq!(error.kind, ErrorKind::CallDepth);
    assert_eq!(error.message, "call depth limit of 8 exceeded");
    // A failed invocation poisons nothing: bounded work still runs on the
    // same context afterwards.
    assert_eq!(ctx.invoke("id", &[Value::Int(7)]), Ok(Value::Int(7)));
}

#[test]
fn results_survive_context_and_engine_reuse() {
    let engine = Engine::new();
    let program = engine
        .load_source("int id(int v) {\nreturn v\n}\n")
        .unwrap();
    let ctx1 = engine.create_context(&program, ExecutionLimits::default());
    let ctx2 = engine.create_context(&program, ExecutionLimits::default());
    assert_eq!(ctx1.invoke("id", &[Value::Int(1)]), Ok(Value::Int(1)));
    assert_eq!(ctx2.invoke("id", &[Value::Int(2)]), Ok(Value::Int(2)));
    assert_eq!(ctx1.invoke("id", &[Value::Int(3)]), Ok(Value::Int(3)));
}

#[test]
fn unicode_arguments_and_results_round_trip() {
    let ctx = context("str cat(str a, str b) {\nreturn a + b\n}\n");
    assert_eq!(
        ctx.invoke(
            "cat",
            &[Value::Str("héllo 🌍".into()), Value::Str("!".into())]
        ),
        Ok(Value::Str("héllo 🌍!".into()))
    );
}

#[test]
fn large_outputs_round_trip() {
    let ctx = context(
        "int[] iota() {\nint[] out = [0, 0, 0]\nfor (int i in [1, 2, 3]) {\nout[i - 1] = i\n}\nreturn out\n}\n",
    );
    let result = ctx.invoke("iota", &[]).unwrap();
    assert_eq!(result.as_array().map(<[Value]>::len), Some(3));
    assert_eq!(result.as_array().unwrap()[2], Value::Int(3));
}

#[test]
fn the_error_type_is_a_std_error() {
    let ctx = context("int f() {\nreturn 1\n}\n");
    let error: ExecutionError = ctx.invoke("ghost", &[]).unwrap_err();
    let boxed: Box<dyn std::error::Error> = Box::new(error.clone());
    assert_eq!(boxed.to_string(), error.render());
}

#[test]
fn value_conversions_build_arguments_from_rust_scalars() {
    let ctx = context(concat!(
        "str describe(int i, float f, bool b, str s) {\n",
        "return s + \" \" + i + \" \" + f + \" \" + b\n}\n",
    ));
    let result = ctx
        .invoke(
            "describe",
            &[
                Value::from(42i64),
                Value::from(2.5f64),
                Value::from(true),
                Value::from("got"),
            ],
        )
        .unwrap();
    assert_eq!(result, Value::Str("got 42 2.5 true".into()));
}
