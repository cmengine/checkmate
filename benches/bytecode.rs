//! Run `cargo bench --bench bytecode`. No benchmark dependencies or timing
//! assertions; compare compilation cost and repeated invocation throughput.

use std::hint::black_box;
use std::time::{Duration, Instant};

use cme_compiler::check::{prepare, prepare_typed};
use cme_interp::{Interpreter, Value, bytecode::Vm};

fn timed(mut run: impl FnMut(), iterations: usize) -> Duration {
    let start = Instant::now();
    for _ in 0..iterations {
        run();
    }
    start.elapsed()
}

fn main() {
    let iterations: usize = std::env::var("CHECKMATE_BENCH_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000)
        .max(1);
    let workloads = [
        (
            "scalar loop",
            "int main() {\nint n = 0\nint total = 0\nwhile (n < 1000) {\ntotal += n\nn += 1\n}\nreturn total\n}\n",
            Value::Int(499500),
        ),
        (
            "calls",
            "int fib(int n) {\nif (n <= 1) { return n }\nreturn fib(n - 1) + fib(n - 2)\n}\nint main() {\nreturn fib(12)\n}\n",
            Value::Int(144),
        ),
        (
            "collections",
            "int main() {\nint[] xs = [1, 2, 3, 4, 5, 6, 7, 8]\nint total = 0\nint n = 0\nwhile (n < 100) {\nfor (int x in xs) { total += x }\nn += 1\n}\nreturn total\n}\n",
            Value::Int(3600),
        ),
    ];
    println!("{iterations} invocations per engine; release profile");
    for (name, source, expected) in workloads {
        let parsed = cme_compiler::parse_source(source);
        assert!(parsed.diagnostics.is_empty());
        let checked = prepare_typed(&parsed.statements, &[], None);
        let code = cme_compiler::lower::compile(&checked, &[]).unwrap();
        assert_eq!(
            Interpreter::new(checked.statements())
                .invoke("main", &[])
                .unwrap(),
            expected
        );
        assert_eq!(Vm::new(&code).invoke("main", &[]).unwrap(), expected);
        let walker = timed(
            || {
                black_box(
                    Interpreter::new(checked.statements())
                        .invoke("main", &[])
                        .unwrap(),
                );
            },
            iterations,
        );
        let vm = timed(
            || {
                black_box(Vm::new(&code).invoke("main", &[]).unwrap());
            },
            iterations,
        );
        let loads = (iterations / 10).max(1);
        let ast_load = timed(
            || {
                let parsed = cme_compiler::parse_source(black_box(source));
                black_box(prepare(&parsed.statements, &[], None));
            },
            loads,
        );
        let bytecode_load = timed(
            || {
                let parsed = cme_compiler::parse_source(black_box(source));
                let checked = prepare_typed(&parsed.statements, &[], None);
                black_box(cme_compiler::lower::compile(&checked, &[]).unwrap());
            },
            loads,
        );
        println!(
            "{name}: invoke walker={walker:?} VM={vm:?}; {loads} loads AST={ast_load:?} bytecode={bytecode_load:?}"
        );
    }
}
