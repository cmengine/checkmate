//! End-to-end integration tests for the root `cme` package: the full
//! front-end pipeline plus the tree-walking interpreter, exactly as the
//! CLI's `run` command drives it (parse, check, then invoke `main` with no
//! arguments — never running code that produced a diagnostic).

use cme_compiler::check::check;
use cme_interp::{InterpError, Interpreter, Value};

const BASIC_CM: &str = include_str!("../basic.cm");
const BOOM_CM: &str = include_str!("../boom.cm");
const SYNTAX_CM: &str = include_str!("../syntax.cm");

/// The pipeline the `run` command drives. Panics on any compile-stage
/// diagnostic so these tests only ever execute programs the gate accepts.
fn run_main(source: &str) -> Result<Value, InterpError> {
    let outcome = cme_compiler::parse_source(source);
    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(check(&outcome.statements));
    assert!(
        diagnostics.is_empty(),
        "the pipeline only runs clean programs: {diagnostics:?}"
    );
    let interpreter = Interpreter::new(&outcome.statements);
    interpreter.invoke("main", &[])
}

/// A `gameEvent.Spawn(kind, position)` value, for the describeEvent pin.
fn spawn_event(kind: &str, x: f64, y: f64) -> Value {
    Value::Enum {
        name: "gameEvent".into(),
        variant: "Spawn".into(),
        payload: vec![
            Value::Str(kind.into()),
            Value::Struct {
                name: "vec2".into(),
                fields: vec![("x".into(), Value::Float(x)), ("y".into(), Value::Float(y))],
            },
        ],
    }
}

/// A `result` value, for the chain / safeDiv pins.
fn result_value(variant: &str, payload: Value) -> Value {
    Value::Enum {
        name: "result".into(),
        variant: variant.into(),
        payload: vec![payload],
    }
}

#[test]
fn basic_cm_runs_end_to_end_and_returns_three() {
    // Hand-derived from basic.cm: fib(0..4) sums to 7 (0+1+1+2+3), x2 = 14,
    // -3 = 11, /3 truncates to 3, % 4 = 3; weight = 2.5 x 1.5 = 3.75 > 2.0
    // so status becomes "start checkmate heavy". main returns the total.
    assert_eq!(run_main(BASIC_CM), Ok(Value::Int(3)));
}

#[test]
fn syntax_cm_runs_end_to_end_and_every_check_passes() {
    // The full-language fixture: main runs every section check and returns
    // a report of the actual outputs the exercised functions produced; the
    // trailing `failures=0` line shows the entire surface behaves per the
    // whitepaper.
    let expected = "fib(10)=55\ngrade(95)=A grade(85)=B grade(42)=F\nsumDown(5)=15 sumAll([1,2,3])=6\nclamp(15,0,10)=10 clamp(value: 15, low: 0, high: 10)=10\ndescribeEvent(Spawn)=spawn:goblin@2,3\nclassifyEvent(Spawn)=3 isDamage(Damage(1))=true\noptionOrDefault([1,3],-1)=-1 optionOrDefault([3,9,14],-1)=14\nchain(64,4,2)=24\ndamage(hero,60) health=40 alive=true callerHealth=100\ndistanceSq((1,2),(3,4))=8\nloot gold=120 gems=3\nprobe=hp=100 pos=(3.5,-1.5) score=201 next=13 armed=true\nfailures=0\n";
    assert_eq!(run_main(SYNTAX_CM), Ok(Value::Str(expected.into())));
}

#[test]
fn syntax_cm_function_pins() {
    // Exact-value pins for the fixture's helpers, one per feature family.
    let outcome = cme_compiler::parse_source(SYNTAX_CM);
    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(check(&outcome.statements));
    assert!(
        diagnostics.is_empty(),
        "the pipeline only runs clean programs: {diagnostics:?}"
    );
    let interpreter = Interpreter::new(&outcome.statements);

    // Recursion and control flow.
    assert_eq!(
        interpreter.invoke("fib", &[Value::Int(10)]),
        Ok(Value::Int(55))
    );
    assert_eq!(
        interpreter.invoke("grade", &[Value::Int(95)]),
        Ok(Value::Str("A".into()))
    );
    assert_eq!(
        interpreter.invoke("sumDown", &[Value::Int(5)]),
        Ok(Value::Int(15))
    );

    // for-in over arrays.
    assert_eq!(
        interpreter.invoke(
            "sumAll",
            &[Value::Array(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3)
            ])]
        ),
        Ok(Value::Int(6))
    );

    // Positional and named arguments (§2.12).
    assert_eq!(
        interpreter.invoke("clamp", &[Value::Int(15), Value::Int(0), Value::Int(10)]),
        Ok(Value::Int(10))
    );

    // option: construction, match, and defaulting.
    assert_eq!(
        interpreter.invoke(
            "findEven",
            &[Value::Array(vec![
                Value::Int(3),
                Value::Int(9),
                Value::Int(14)
            ])]
        ),
        Ok(Value::Enum {
            name: "option".into(),
            variant: "Some".into(),
            payload: vec![Value::Int(14)]
        })
    );
    assert_eq!(
        interpreter.invoke(
            "optionOrDefault",
            &[
                Value::Array(vec![Value::Int(1), Value::Int(3)]),
                Value::Int(-1)
            ]
        ),
        Ok(Value::Int(-1))
    );

    // result and the ? operator through call boundaries (§2.8).
    assert_eq!(
        interpreter.invoke("chain", &[Value::Int(64), Value::Int(4), Value::Int(2)]),
        Ok(result_value("Ok", Value::Int(24)))
    );
    assert_eq!(
        interpreter.invoke("chain", &[Value::Int(64), Value::Int(0), Value::Int(2)]),
        Ok(result_value("Err", Value::Str("division by zero".into())))
    );

    // Enum construction, destructuring match, and interpolation (§2.7,
    // §2.15, §2.8).
    assert_eq!(
        interpreter.invoke("describeEvent", &[spawn_event("goblin", 2.0, 3.0)]),
        Ok(Value::Str("spawn:goblin@2,3".into()))
    );

    // The pure interpolation probe.
    assert_eq!(
        interpreter.invoke("probeInterpolation", &[]),
        Ok(Value::Str(
            "hp=100 pos=(3.5,-1.5) score=201 next=13 armed=true".into()
        ))
    );
}

/// A `counter` value, for the impl-block pins.
fn counter_value(value: i64) -> Value {
    Value::Struct {
        name: "counter".into(),
        fields: vec![("value".into(), Value::Int(value))],
    }
}

#[test]
fn syntax_cm_impl_pins() {
    // Exact-value pins for the §10.4 impl-block surface: associated members
    // on a struct (unioned across two blocks), on an enum (feeding match),
    // and through a host-style dotted path target.
    let outcome = cme_compiler::parse_source(SYNTAX_CM);
    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(check(&outcome.statements));
    assert!(
        diagnostics.is_empty(),
        "the pipeline only runs clean programs: {diagnostics:?}"
    );
    let interpreter = Interpreter::new(&outcome.statements);

    // Struct members: plain read and a forward reference across blocks.
    assert_eq!(
        interpreter.invoke_member("counter", "peek", &[counter_value(41)]),
        Ok(Value::Int(41))
    );
    assert_eq!(
        interpreter.invoke_member("counter", "peekTwice", &[counter_value(41)]),
        Ok(Value::Int(82))
    );

    // Value semantics: bump mutates its own clone (§2.13).
    assert_eq!(
        interpreter.invoke_member("counter", "bump", &[counter_value(41)]),
        Ok(counter_value(42))
    );

    // Enum member.
    assert_eq!(
        interpreter.invoke_member(
            "suit",
            "label",
            &[Value::Enum {
                name: "suit".into(),
                variant: "Hearts".into(),
                payload: vec![]
            }]
        ),
        Ok(Value::Str("hearts".into()))
    );

    // Host-style dotted path target: engine.gamemode.InitGame(config).
    assert_eq!(
        interpreter.invoke_member(
            "engine.gamemode",
            "InitGame",
            &[Value::Struct {
                name: "GameConfig".into(),
                fields: vec![
                    ("startingScore".into(), Value::Int(100)),
                    ("active".into(), Value::Bool(true)),
                ],
            }]
        ),
        Ok(Value::Struct {
            name: "GameState".into(),
            fields: vec![
                ("score".into(), Value::Int(100)),
                ("active".into(), Value::Bool(true)),
            ],
        })
    );
}

#[test]
fn prefix_truncation_never_panics_the_pipeline_or_the_interpreter() {
    // Extends the front-end truncation property through execution: for
    // every char-boundary prefix of every fixture, parse + check must
    // never panic, and the interpreter runs `main` whenever the prefix is
    // clean. Runtime errors (a prefix without `main`, a truncated
    // computation hitting a limit) are normal outcomes; only a panic
    // fails the property.
    for fixture in [BASIC_CM, BOOM_CM, SYNTAX_CM] {
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
