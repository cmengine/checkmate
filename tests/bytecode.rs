//! Differential coverage: independent control-flow/call engines over the same
//! checked program, plus bytecode-specific budgets, frames and verification.

use std::cell::Cell;
use std::path::Path;
use std::time::{Duration, Instant};

#[path = "support/fixtures.rs"]
mod fixtures;

use cme_compiler::check::{CheckedProgram, prepare_typed};
use cme_core::ast::{PrimitiveType, Type};
use cme_interp::bytecode::{Bytecode, Vm};
use cme_interp::{InterpError, InterpErrorKind, Interpreter, Value};

fn compile(source: &str) -> (CheckedProgram, Bytecode) {
    let expanded = if cme_compiler::mega::expand::mentions_megaprogram(source) {
        cme_compiler::mega::expand::expand_source(source)
            .expect("expansion")
            .expanded
    } else {
        source.to_string()
    };
    let parsed = cme_compiler::parse_source(&expanded);
    assert!(
        parsed.diagnostics.is_empty(),
        "parse: {:?}",
        parsed.diagnostics
    );
    let checked = prepare_typed(&parsed.statements, &[], None);
    assert!(
        checked.diagnostics().is_empty(),
        "check: {:?}",
        checked.diagnostics()
    );
    let ir = cme_compiler::lower::lower(&checked, &[]).expect("lowering");
    let code = Bytecode::compile(&ir).expect("codegen");
    (checked, code)
}

fn same_result(walker: Result<Value, InterpError>, vm: Result<Value, InterpError>) {
    match (walker, vm) {
        (Ok(expected), Ok(actual)) => assert_eq!(actual, expected),
        (Err(expected), Err(actual)) => {
            assert_eq!(actual.kind(), expected.kind());
            assert_eq!(actual.message, expected.message);
            assert_eq!(actual.span, expected.span);
        }
        (expected, actual) => panic!("engines disagree: walker={expected:?}, VM={actual:?}"),
    }
}

#[test]
fn every_program_fixture_matches_the_reference_walker() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/programs");
    let mut paths: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    let mut count = 0;
    for path in paths {
        if path.extension().is_none_or(|ext| ext != "cm") {
            continue;
        }
        let source = fixtures::program_source(&path);
        let (checked, code) = compile(&source);
        let expected = Interpreter::new(checked.statements()).invoke("main", &[]);
        let actual = Vm::new(&code).invoke("main", &[]);
        assert_eq!(
            actual,
            Ok(fixtures::expected_result(&path)),
            "fixture: {path:?}"
        );
        same_result(expected, actual);
        count += 1;
    }
    assert!(count >= 10);
}

#[test]
fn runtime_errors_keep_kind_message_and_expression_span() {
    for body in [
        "return 1 / 0",
        "return 1 % 0",
        "return 9223372036854775807 + 1",
        "int[] xs = [1]\nreturn xs[-1]",
        "map<str, int> m = {}\nreturn m[\"missing\"]",
        "byte b = 255\nreturn b + 1",
        "byte b = 1\nb -= 2\nreturn b",
        "return (0 - 9223372036854775807 - 1) / -1",
        "int[] xs = [1]\nxs[2] += 1\nreturn 0",
    ] {
        let source = format!("int main() {{\n{body}\n}}\n");
        let (checked, code) = compile(&source);
        same_result(
            Interpreter::new(checked.statements()).invoke("main", &[]),
            Vm::new(&code).invoke("main", &[]),
        );
    }
}

#[test]
fn fuel_is_exact_instruction_count_and_charged_on_returns() {
    let (_, code) = compile("int main() {\nreturn 42\n}\n");
    // One constant and one return, no implicit AST-operation charges.
    assert_eq!(code.instruction_count(), 2);
    for budget in [0, 1] {
        let fuel = Cell::new(budget);
        let error = Vm::new(&code)
            .with_fuel(&fuel)
            .invoke("main", &[])
            .unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::Budget);
        assert!(error.message.contains("instruction count"));
        assert_eq!(fuel.get(), 0);
    }
    let fuel = Cell::new(2);
    assert_eq!(
        Vm::new(&code).with_fuel(&fuel).invoke("main", &[]),
        Ok(Value::Int(42))
    );
    assert_eq!(fuel.get(), 0);
}

#[test]
fn empty_loops_are_metered_and_deadlines_observed() {
    let (_, code) = compile("void main() {\nwhile (true) {}\n}\n");
    let fuel = Cell::new(23);
    assert_eq!(
        Vm::new(&code)
            .with_fuel(&fuel)
            .invoke("main", &[])
            .unwrap_err()
            .kind(),
        InterpErrorKind::Budget
    );
    assert_eq!(fuel.get(), 0);
    assert_eq!(
        Vm::new(&code)
            .with_deadline(Instant::now() - Duration::from_millis(1))
            .invoke("main", &[])
            .unwrap_err()
            .kind(),
        InterpErrorKind::Deadline
    );
}

#[test]
fn script_recursion_uses_explicit_frames_on_a_small_native_stack() {
    let (_, code) =
        compile("int down(int n) {\nif (n == 0) { return 0 }\nreturn down(n - 1) + 1\n}\n");
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || {
            assert_eq!(
                Vm::new(&code).invoke("down", &[Value::Int(1000)]),
                Ok(Value::Int(1000))
            );
            let error = Vm::new(&code)
                .with_call_depth_limit(12)
                .invoke("down", &[Value::Int(1000)])
                .unwrap_err();
            assert_eq!(error.kind(), InterpErrorKind::CallDepth);
            assert!(error.message.contains("12"));
            assert_eq!(
                Vm::new(&code)
                    .invoke("down", &[Value::Int(1500)])
                    .unwrap_err()
                    .kind(),
                InterpErrorKind::CallDepth
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn inferred_locals_and_call_targets_are_resolved_before_execution() {
    let (checked, _) = compile(
        "int plus(int x) {\nreturn x + 1\n}\nint main() {\ninfer x = 7\nreturn plus(x)\n}\n",
    );
    let ir = cme_compiler::lower::lower(&checked, &[]).unwrap();
    assert!(
        ir.functions
            .iter()
            .all(|function| function.registers.iter().all(|ty| *ty != Type::Infer))
    );
    assert!(
        ir.functions[1]
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(i.op, cme_core::ir::Op::Call { function: 0, .. }))
    );
    assert!(
        ir.functions[1]
            .registers
            .contains(&Type::Prim(PrimitiveType::Int))
    );
}

#[test]
fn missing_entries_and_host_arity_match_the_walker() {
    let (checked, code) = compile("int f(byte x) {\nreturn x\n}\n");
    for args in [vec![], vec![Value::Int(300)], vec![Value::Int(12)]] {
        same_result(
            Interpreter::new(checked.statements()).invoke("f", &args),
            Vm::new(&code).invoke("f", &args),
        );
    }
    same_result(
        Interpreter::new(checked.statements()).invoke("missing", &[]),
        Vm::new(&code).invoke("missing", &[]),
    );
    same_result(
        Interpreter::new(checked.statements()).invoke_member("missing", "F", &[]),
        Vm::new(&code).invoke_member("missing", "F", &[]),
    );
}

#[test]
fn diagnostic_programs_never_lower() {
    let parsed = cme_compiler::parse_source("int main() {\nreturn unknown\n}\n");
    let checked = prepare_typed(&parsed.statements, &[], None);
    assert!(cme_compiler::lower::lower(&checked, &[]).is_err());
}

#[test]
fn malformed_public_ir_is_rejected_before_execution() {
    let (checked, _) = compile("int main() {\nreturn 1\n}\n");
    let ir = cme_compiler::lower::lower(&checked, &[]).unwrap();
    let mut malformed = ir.clone();
    malformed.functions[0].blocks[0].exit.kind = cme_core::ir::ExitKind::Jump(99);
    assert!(Bytecode::compile(&malformed).is_err());
    let mut malformed = ir.clone();
    malformed.functions[0].registers.clear();
    assert!(Bytecode::compile(&malformed).is_err());
    let mut malformed = ir.clone();
    malformed.functions[0].blocks.clear();
    assert!(Bytecode::compile(&malformed).is_err());
    let mut malformed = ir;
    malformed.functions[0].blocks[0].instructions[0].op = cme_core::ir::Op::Call {
        dst: 0,
        function: 77,
        args: vec![],
    };
    assert!(Bytecode::compile(&malformed).is_err());
}

const TRACE_SCHEMA: &str = "
schema trace 1.0.0
struct Pair {
    int first
    int second
}
since 1.0.0 capability events {
    int Mark(int value)
    bool Flag(bool value)
    int Boom()
    result<int, str> Result()
}
since 1.0.0 interface entry {
    int Run()
}
";

struct TraceHost {
    log: std::cell::RefCell<Vec<i64>>,
    wrong_bool: bool,
}

impl cme_interp::CapabilityHost for TraceHost {
    fn call(&self, path: &[&str], member: &str, args: &[Value]) -> Result<Value, String> {
        assert_eq!(path, &["trace", "events"]);
        match member {
            "Mark" => {
                let Value::Int(value) = args[0] else {
                    panic!("Mark int argument")
                };
                self.log.borrow_mut().push(value);
                Ok(Value::Int(value))
            }
            "Flag" if self.wrong_bool => Ok(Value::Int(42)),
            "Flag" => Ok(args[0].clone()),
            "Boom" => Err("trace failed".to_string()),
            "Result" => Ok(Value::Enum {
                name: "result".to_string(),
                variant: "Malformed".to_string(),
                payload: vec![Value::Int(42)],
            }),
            _ => panic!("unexpected trace call"),
        }
    }
}

fn schema_compile(source: &str) -> (CheckedProgram, Vec<cme_core::ast::Stmt>, Bytecode) {
    let parsed_schema = cme_compiler::schema::parse_schema_file(TRACE_SCHEMA);
    assert!(parsed_schema.diagnostics.is_empty());
    let set = cme_compiler::schema::SchemaSet::build(vec![parsed_schema.file.unwrap()]).unwrap();
    let schema = cme_compiler::schema::SchemaContext::grant_all(set);
    let parsed = cme_compiler::parse_source(source);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let checked = prepare_typed(&parsed.statements, &[], Some(&schema));
    let declarations = cme_compiler::schema::declaration_statements(&schema);
    let code = cme_compiler::lower::compile(&checked, &declarations).unwrap();
    (checked, declarations, code)
}

#[test]
fn schema_calls_preserve_evaluation_order_and_impl_entry_dispatch() {
    let source = "import trace.events
int combine(int first, int second) { return first * 10 + second }
impl trace.entry {
    int Run() {
        infer pair = Pair(second: trace.events.Mark(2), first: trace.events.Mark(1))
        int args = combine(second: trace.events.Mark(4), first: trace.events.Mark(3))
        bool skipped = false && trace.events.Flag(true)
        int[][] xs = [[10, 20], [30, 40]]
        xs[trace.events.Mark(0)][trace.events.Mark(1)] += trace.events.Mark(5)
        xs[trace.events.Mark(0)][trace.events.Mark(1)] = trace.events.Mark(6)
        return pair.first * 1000 + pair.second * 100 + args + xs[0][1]
    }
}
";
    let (checked, declarations, code) = schema_compile(source);
    let host = TraceHost {
        log: Default::default(),
        wrong_bool: false,
    };
    let walker = Interpreter::new(checked.statements())
        .with_declarations(&declarations)
        .with_capabilities(&host);
    let expected = walker.invoke_member("trace.entry", "Run", &[]);
    let log = host.log.take();
    let actual = Vm::new(&code)
        .with_capabilities(&host)
        .invoke_member("trace.entry", "Run", &[]);
    assert_eq!(actual, Ok(Value::Int(1240)));
    assert_eq!(*host.log.borrow(), log);
    assert_eq!(log, vec![1, 2, 4, 3, 1, 0, 5, 6, 1, 0]);
    same_result(expected, actual);
}

#[test]
fn host_failures_and_boolean_shape_errors_match_the_reference() {
    for body in [
        "return trace.events.Boom()",
        "if (trace.events.Flag(true)) { return 1 }\nreturn 0",
        "while (trace.events.Flag(true)) {}\nreturn 0",
        "bool value = true && trace.events.Flag(true)\nreturn 0",
        "bool value = false || trace.events.Flag(true)\nreturn 0",
        "bool value = trace.events.Flag(true) && true\nreturn 0",
        "bool value = trace.events.Flag(true) || false\nreturn 0",
    ] {
        let source = format!("import trace.events\nint main() {{\n{body}\n}}\n");
        let (checked, declarations, code) = schema_compile(&source);
        let host = TraceHost {
            log: Default::default(),
            wrong_bool: true,
        };
        same_result(
            Interpreter::new(checked.statements())
                .with_declarations(&declarations)
                .with_capabilities(&host)
                .invoke("main", &[]),
            Vm::new(&code).with_capabilities(&host).invoke("main", &[]),
        );
    }
}

#[test]
fn inferred_and_generic_assignment_slots_retain_checker_types() {
    let (checked, _) = compile(
        "struct Box<T> { T value }\nint main() {\ninfer x = Box(value: [1, 2])\nx.value[0] += 3\nreturn x.value[0]\n}\n",
    );
    let ir = cme_compiler::lower::lower(&checked, &[]).unwrap();
    assert!(
        ir.functions[0]
            .registers
            .iter()
            .all(|ty| *ty != Type::Infer)
    );
}

#[test]
fn reused_spans_do_not_merge_distinct_expression_types() {
    use cme_core::ast::{Span, StmtKind};
    let mut parsed =
        cme_compiler::parse_source("int main() {\ninfer a = 1\ninfer b = \"text\"\nreturn a\n}\n");
    if let StmtKind::FuncDecl { body, .. } = &mut parsed.statements[0].kind {
        for stmt in &mut body.stmts {
            if let StmtKind::VarDecl { expr, .. } = &mut stmt.kind {
                expr.span = Span::new(0, 0);
            }
        }
    }
    let checked = prepare_typed(&parsed.statements, &[], None);
    let StmtKind::FuncDecl { body, .. } = &checked.statements()[0].kind else {
        panic!()
    };
    for (stmt, primitive) in body
        .stmts
        .iter()
        .zip([PrimitiveType::Int, PrimitiveType::Str])
    {
        let StmtKind::VarDecl { expr, .. } = &stmt.kind else {
            panic!()
        };
        assert_eq!(checked.expression_type(expr), Some(&Type::Prim(primitive)));
    }
    let code = cme_compiler::lower::compile(&checked, &[]).unwrap();
    assert_eq!(Vm::new(&code).invoke("main", &[]), Ok(Value::Int(1)));
}

#[test]
fn malformed_host_results_keep_try_operator_errors() {
    let source =
        "import trace.events\nresult<int, str> main() {\nreturn Ok(trace.events.Result()?)\n}\n";
    let (checked, declarations, code) = schema_compile(source);
    let host = TraceHost {
        log: Default::default(),
        wrong_bool: false,
    };
    same_result(
        Interpreter::new(checked.statements())
            .with_declarations(&declarations)
            .with_capabilities(&host)
            .invoke("main", &[]),
        Vm::new(&code).with_capabilities(&host).invoke("main", &[]),
    );
}

#[test]
fn mixed_int_byte_overflow_is_checked_in_both_operand_orders() {
    for operation in [
        "high + one",
        "one + high",
        "low - one",
        "one - low",
        "high * two",
        "two * high",
    ] {
        let source = format!(
            "int main() {{\nint high = 9223372036854775807\nint low = 0 - 9223372036854775807 - 1\nbyte one = 1\nbyte two = 2\nreturn {operation}\n}}\n"
        );
        let (checked, code) = compile(&source);
        let walker = Interpreter::new(checked.statements()).invoke("main", &[]);
        let vm = Vm::new(&code).invoke("main", &[]);
        assert!(
            walker
                .as_ref()
                .unwrap_err()
                .message
                .contains("integer overflow")
        );
        assert!(
            vm.as_ref()
                .unwrap_err()
                .message
                .contains("integer overflow")
        );
        same_result(walker, vm);
    }
    for (operation, expected) in [
        ("high - one", i64::MAX - 1),
        ("low + one", i64::MIN + 1),
        ("one * high", i64::MAX),
    ] {
        let source = format!(
            "int main() {{\nint high = 9223372036854775807\nint low = 0 - 9223372036854775807 - 1\nbyte one = 1\nreturn {operation}\n}}\n"
        );
        let (checked, code) = compile(&source);
        assert_eq!(Vm::new(&code).invoke("main", &[]), Ok(Value::Int(expected)));
        same_result(
            Interpreter::new(checked.statements()).invoke("main", &[]),
            Vm::new(&code).invoke("main", &[]),
        );
    }
}

// Boundary matrices exercise runtime operands, rather than constant folding.
#[test]
fn integer_boundary_matrix_matches_checked_rust_arithmetic() {
    let boundaries = [
        i64::MIN,
        i64::MIN + 1,
        -256,
        -1,
        0,
        1,
        255,
        256,
        i64::MAX - 1,
        i64::MAX,
    ];
    for (operator, calculate) in [
        ("+", i64::checked_add as fn(i64, i64) -> Option<i64>),
        ("-", i64::checked_sub),
        ("*", i64::checked_mul),
        ("/", i64::checked_div),
        ("%", i64::checked_rem),
    ] {
        let (checked, code) = compile(&format!(
            "int f(int a, int b) {{\nreturn a {operator} b\n}}\n"
        ));
        let walker = Interpreter::new(checked.statements());
        let vm = Vm::new(&code);
        for a in boundaries {
            for b in boundaries {
                let args = [Value::Int(a), Value::Int(b)];
                let actual = vm.invoke("f", &args);
                match calculate(a, b) {
                    Some(value) => assert_eq!(actual, Ok(Value::Int(value)), "{a} {operator} {b}"),
                    None => assert_eq!(
                        actual.as_ref().unwrap_err().kind(),
                        InterpErrorKind::Runtime
                    ),
                }
                same_result(walker.invoke("f", &args), actual);
            }
        }
    }
}

#[test]
fn byte_boundary_matrix_preserves_byte_domain() {
    for (operator, calculate) in [
        ("+", u8::checked_add as fn(u8, u8) -> Option<u8>),
        ("-", u8::checked_sub),
        ("*", u8::checked_mul),
        ("/", u8::checked_div),
        ("%", u8::checked_rem),
    ] {
        let (checked, code) = compile(&format!(
            "byte f(byte a, byte b) {{\nreturn a {operator} b\n}}\n"
        ));
        for a in [0, 1, 2, 127, 128, 254, 255] {
            for b in [0, 1, 2, 127, 128, 254, 255] {
                let args = [Value::Byte(a), Value::Byte(b)];
                let actual = Vm::new(&code).invoke("f", &args);
                match calculate(a, b) {
                    Some(value) => assert_eq!(actual, Ok(Value::Byte(value)), "{a} {operator} {b}"),
                    None => assert!(actual.is_err(), "{a} {operator} {b}"),
                }
                same_result(
                    Interpreter::new(checked.statements()).invoke("f", &args),
                    actual,
                );
            }
        }
    }
}

#[test]
fn mixed_boundary_matrix_widens_and_checks_both_operand_orders() {
    for (operator, calculate) in [
        ("+", i64::checked_add as fn(i64, i64) -> Option<i64>),
        ("-", i64::checked_sub),
        ("*", i64::checked_mul),
        ("/", i64::checked_div),
        ("%", i64::checked_rem),
    ] {
        for reverse in [false, true] {
            let signature = if reverse {
                "byte a, int b"
            } else {
                "int a, byte b"
            };
            let (checked, code) = compile(&format!(
                "int f({signature}) {{\nreturn a {operator} b\n}}\n"
            ));
            for int in [
                i64::MIN,
                i64::MIN + 1,
                -256,
                -1,
                0,
                1,
                256,
                i64::MAX - 1,
                i64::MAX,
            ] {
                for byte in [0, 1, 2, 127, 255] {
                    let (args, expected) = if reverse {
                        (
                            [Value::Byte(byte), Value::Int(int)],
                            calculate(byte as i64, int),
                        )
                    } else {
                        (
                            [Value::Int(int), Value::Byte(byte)],
                            calculate(int, byte as i64),
                        )
                    };
                    let actual = Vm::new(&code).invoke("f", &args);
                    match expected {
                        Some(value) => assert_eq!(actual, Ok(Value::Int(value))),
                        None => assert!(actual.is_err()),
                    }
                    same_result(
                        Interpreter::new(checked.statements()).invoke("f", &args),
                        actual,
                    );
                }
            }
        }
    }
}

#[test]
fn boolean_truth_tables_and_nested_short_circuiting() {
    for expression in [
        "a && b",
        "a || b",
        "!a",
        "a == b",
        "a != b",
        "(a || b) && (!a || !b)",
    ] {
        let (checked, code) = compile(&format!(
            "bool f(bool a, bool b) {{\nreturn {expression}\n}}\n"
        ));
        for a in [false, true] {
            for b in [false, true] {
                let expected = match expression {
                    "a && b" => a && b,
                    "a || b" => a || b,
                    "!a" => !a,
                    "a == b" => a == b,
                    "a != b" => a != b,
                    _ => a != b,
                };
                let args = [Value::Bool(a), Value::Bool(b)];
                let actual = Vm::new(&code).invoke("f", &args);
                assert_eq!(actual, Ok(Value::Bool(expected)));
                same_result(
                    Interpreter::new(checked.statements()).invoke("f", &args),
                    actual,
                );
            }
        }
    }
    for expression in ["false && (1 / 0 == 0)", "true || (1 / 0 == 0)"] {
        let (checked, code) = compile(&format!("bool main() {{\nreturn {expression}\n}}\n"));
        same_result(
            Interpreter::new(checked.statements()).invoke("main", &[]),
            Vm::new(&code).invoke("main", &[]),
        );
    }
}

macro_rules! program_case {
    ($name:ident, $source:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let (checked, code) = compile($source);
            let actual = Vm::new(&code).invoke("main", &[]);
            assert_eq!(actual, Ok($expected));
            same_result(
                Interpreter::new(checked.statements()).invoke("main", &[]),
                actual,
            );
        }
    };
}

program_case!(void_fallthrough, "void main() {}", Value::Void);
program_case!(
    void_explicit_return,
    "void main() {\nreturn\n}",
    Value::Void
);
program_case!(
    nested_branch_early_returns,
    "int main() {\nif (true) {\nif (false) { return 1 } else { return 2 }\n}\nreturn 3\n}",
    Value::Int(2)
);
program_case!(
    while_zero_iterations,
    "int main() {\nint n = 7\nwhile (false) { n += 1 }\nreturn n\n}",
    Value::Int(7)
);
program_case!(
    while_return_unwinds,
    "int main() {\nwhile (true) {\nwhile (true) { return 8 }\n}\nreturn 0\n}",
    Value::Int(8)
);
program_case!(
    for_empty_array,
    "int main() {\nint[] xs = []\nint sum = 0\nfor (int x in xs) { sum += x }\nreturn sum\n}",
    Value::Int(0)
);
program_case!(
    for_return_unwinds,
    "int main() {\nfor (int x in [3, 4]) { return x }\nreturn 0\n}",
    Value::Int(3)
);
program_case!(
    for_snapshot_survives_mutation,
    "int main() {\nint[] xs = [1, 2, 3]\nint sum = 0\nfor (int x in xs) {\nsum += x\nxs[1] = 100\n}\nreturn sum\n}",
    Value::Int(6)
);
program_case!(
    map_snapshot_survives_insertion,
    "str main() {\nmap<str, int> xs = {\"a\": 1, \"b\": 2}\nstr keys = \"\"\nfor (str key in xs) {\nkeys += key\nxs[\"c\"] = 3\n}\nreturn keys\n}",
    Value::Str("ab".into())
);
program_case!(
    map_duplicate_keys_keep_order,
    "str main() {\nmap<str, int> xs = {\"a\": 1, \"b\": 2, \"a\": 3}\nstr keys = \"\"\nfor (str key in xs) { keys += key }\nreturn keys + xs[\"a\"]\n}",
    Value::Str("ab3".into())
);
program_case!(
    nested_array_copies_are_isolated,
    "int main() {\nint[][] a = [[1, 2]]\nint[][] b = a\nb[0][1] = 8\nreturn a[0][1] * 10 + b[0][1]\n}",
    Value::Int(28)
);
program_case!(
    array_parameter_copies_are_isolated,
    "int mutate(int[] x) {\nx[0] = 9\nreturn x[0]\n}\nint main() {\nint[] x = [1]\nint changed = mutate(x)\nreturn changed * 10 + x[0]\n}",
    Value::Int(91)
);
program_case!(
    nested_map_array_write,
    "int main() {\nmap<str, int[]> x = {\"a\": [1, 2]}\nx[\"a\"][1] *= 3\nreturn x[\"a\"][1]\n}",
    Value::Int(6)
);
program_case!(
    nested_struct_copy_isolated,
    "struct Inner { int value }\nstruct Outer { Inner inner }\nint main() {\nOuter a = Outer(inner: Inner(value: 3))\nOuter b = a\nb.inner.value += 2\nreturn a.inner.value * 10 + b.inner.value\n}",
    Value::Int(35)
);
program_case!(
    generic_enum_pattern_payloads,
    "enum Pair<T> { Values(T a, T b) }\nint main() {\nPair<int> p = Pair.Values(3, 4)\nreturn match (p) { Values(int a, int b) => a * 10 + b }\n}",
    Value::Int(34)
);
program_case!(
    match_statement_return_in_loop,
    "int main() {\nfor (int n in [1, 2]) {\noption<int> value = Some(n)\nmatch (value) {\nSome(int x) => { return x }\nNone() => { return 0 }\n}\n}\nreturn 9\n}",
    Value::Int(1)
);
program_case!(
    match_expression_wildcard,
    "int main() {\noption<int> x = None()\nreturn match (x) { Some(int n) => n\n_ => 7 }\n}",
    Value::Int(7)
);
program_case!(
    try_error_unwinds_nested_loop,
    "result<int, str> fail() {\nreturn Err(\"stop\")\n}\nresult<int, str> main() {\nfor (int x in [1]) {\nint y = fail()?\nreturn Ok(y)\n}\nreturn Ok(0)\n}",
    Value::Enum {
        name: "result".into(),
        variant: "Err".into(),
        payload: vec![Value::Str("stop".into())]
    }
);
program_case!(
    try_success_in_nested_calls,
    "result<int, str> one() {\nreturn Ok(1)\n}\nresult<int, str> main() {\nreturn Ok(one()? + one()?)\n}",
    Value::Enum {
        name: "result".into(),
        variant: "Ok".into(),
        payload: vec![Value::Int(2)]
    }
);
program_case!(
    byte_array_element_coercion,
    "byte main() {\nbyte[] xs = [1, 2]\nxs[0] += 3\nreturn xs[0]\n}",
    Value::Byte(4)
);
program_case!(
    byte_map_value_coercion,
    "byte main() {\nmap<str, byte> xs = {\"a\": 2}\nxs[\"a\"] += 3\nreturn xs[\"a\"]\n}",
    Value::Byte(5)
);
program_case!(
    byte_option_payload_coercion,
    "byte main() {\noption<byte> x = Some(7)\nreturn match (x) { Some(byte n) => n\nNone() => 0 }\n}",
    Value::Byte(7)
);
program_case!(
    byte_result_payload_coercion,
    "result<byte, str> main() {\nreturn Ok(7)\n}",
    Value::Enum {
        name: "result".into(),
        variant: "Ok".into(),
        payload: vec![Value::Byte(7)]
    }
);
program_case!(
    named_arguments_permute_values,
    "int f(int a, int b) {\nreturn a * 10 + b\n}\nint main() {\nreturn f(b: 2, a: 1)\n}",
    Value::Int(12)
);
program_case!(
    mutual_recursion,
    "int even(int n) {\nif (n == 0) { return 1 }\nreturn odd(n - 1)\n}\nint odd(int n) {\nif (n == 0) { return 0 }\nreturn even(n - 1)\n}\nint main() {\nreturn even(16)\n}",
    Value::Int(1)
);
program_case!(
    impl_member_script_calls,
    "struct Counter { int value }\nimpl Counter {\nint Twice(int n) { return n * 2 }\n}\nint main() {\nreturn Counter.Twice(6)\n}",
    Value::Int(12)
);

program_case!(
    unicode_strings_round_trip,
    "str main() {\nstr text = \"é🦀\"\nreturn text + \"雪\"\n}",
    Value::Str("é🦀雪".into())
);

#[test]
fn float_ieee_arithmetic_including_nan_and_infinities() {
    for operator in ["+", "-", "*", "/"] {
        let (checked, code) = compile(&format!(
            "float f(float a, float b) {{\nreturn a {operator} b\n}}\n"
        ));
        for a in [
            f64::NEG_INFINITY,
            -1.5,
            -0.0,
            0.0,
            2.25,
            f64::INFINITY,
            f64::NAN,
        ] {
            for b in [
                f64::NEG_INFINITY,
                -1.5,
                -0.0,
                0.0,
                2.25,
                f64::INFINITY,
                f64::NAN,
            ] {
                let args = [Value::Float(a), Value::Float(b)];
                let expected = match operator {
                    "+" => a + b,
                    "-" => a - b,
                    "*" => a * b,
                    _ => a / b,
                };
                for result in [
                    Interpreter::new(checked.statements()).invoke("f", &args),
                    Vm::new(&code).invoke("f", &args),
                ] {
                    let Value::Float(actual) = result.unwrap() else {
                        panic!("float result required")
                    };
                    if expected.is_nan() {
                        assert!(actual.is_nan());
                    } else {
                        assert_eq!(actual.to_bits(), expected.to_bits());
                    }
                }
            }
        }
    }
}

#[test]
fn float_comparisons_obey_ieee_nan_rules() {
    for operator in ["==", "!=", "<", "<=", ">", ">="] {
        let (checked, code) = compile(&format!(
            "bool f(float a, float b) {{\nreturn a {operator} b\n}}\n"
        ));
        for a in [
            f64::NEG_INFINITY,
            -1.0,
            -0.0,
            0.0,
            1.0,
            f64::INFINITY,
            f64::NAN,
        ] {
            for b in [
                f64::NEG_INFINITY,
                -1.0,
                -0.0,
                0.0,
                1.0,
                f64::INFINITY,
                f64::NAN,
            ] {
                let expected = match operator {
                    "==" => a == b,
                    "!=" => a != b,
                    "<" => a < b,
                    "<=" => a <= b,
                    ">" => a > b,
                    _ => a >= b,
                };
                let args = [Value::Float(a), Value::Float(b)];
                let actual = Vm::new(&code).invoke("f", &args);
                assert_eq!(actual, Ok(Value::Bool(expected)));
                same_result(
                    Interpreter::new(checked.statements()).invoke("f", &args),
                    actual,
                );
            }
        }
    }
}

#[test]
fn integer_comparisons_keep_full_precision_at_extremes() {
    for operator in ["==", "!=", "<", "<=", ">", ">="] {
        let (checked, code) = compile(&format!(
            "bool f(int a, int b) {{\nreturn a {operator} b\n}}\n"
        ));
        for a in [i64::MIN, -1, 0, 1, i64::MAX - 1, i64::MAX] {
            for b in [i64::MIN, -1, 0, 1, i64::MAX - 1, i64::MAX] {
                let expected = match operator {
                    "==" => a == b,
                    "!=" => a != b,
                    "<" => a < b,
                    "<=" => a <= b,
                    ">" => a > b,
                    _ => a >= b,
                };
                let args = [Value::Int(a), Value::Int(b)];
                let actual = Vm::new(&code).invoke("f", &args);
                assert_eq!(actual, Ok(Value::Bool(expected)));
                same_result(
                    Interpreter::new(checked.statements()).invoke("f", &args),
                    actual,
                );
            }
        }
    }
}

#[test]
fn unary_negation_checks_minimum_integer() {
    let (checked, code) = compile("int f(int x) {\nreturn -x\n}\n");
    for x in [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX] {
        let args = [Value::Int(x)];
        let actual = Vm::new(&code).invoke("f", &args);
        match x.checked_neg() {
            Some(expected) => assert_eq!(actual, Ok(Value::Int(expected))),
            None => assert!(actual.is_err()),
        }
        same_result(
            Interpreter::new(checked.statements()).invoke("f", &args),
            actual,
        );
    }
}
