# Execution Engine

`cme run` and the Rust/C host APIs execute a synchronous, register-based
bytecode VM (§5.1). Loading a program expands megaprograms, resolves imports,
checks types and schema contracts, lowers typed control flow, and generates
verified bytecode. A program with diagnostics never becomes invocable.

Rust hosts retain a `CompiledProgram` and reuse it for invocations. Compilation
happens once per load; calls do not rebuild declaration registries or walk AST
expressions. The retained AST remains available through `statements()` for
tooling. `bytecode()` exposes immutable bytecode with `function_count()` and
`instruction_count()` for inspection; it is not a serialized artifact format.

## Calls and values

Each invocation owns its registers and an explicit script frame stack. It can
call plain functions, local impl members, and schema capabilities through the
existing host provider boundary. Values retain copy isolation. Source spans
survive lowering, so runtime failures still point into the correct module.

Rust calls still return `Result<Value, ExecutionError>` synchronously. C future
handles are already ready or failed on their first poll. The VM introduces no
scheduler or asynchronous host work.

## Budgets

Fuel now counts executed bytecode instructions, including branches, jumps,
calls, and returns. Deadline checks happen before each instruction. The default
script call-depth bound remains 1024, and script recursion no longer depends on
the host's native stack size. See [Execution Limits and Errors](../appendix/limits-and-errors.md)
for unset conventions and migration from operation-count budgets.

## Reference execution and backend status

The tree walker remains available as `cme_interp::Interpreter`. It is the
reference oracle for differential tests and the pure compile-time evaluator
used during megaprogram expansion. Its fuel meter still counts AST operations.

> **Future:** Transparent async (`suspend` and compiler-generated continuations),
> cancellation, LLVM AOT, native artifact loading, ARC/COW buffers, allocation
> budgets, and freestanding support remain unimplemented. A synchronous VM call
> frame is never parked as an async continuation.

For contributors, `cargo bench --bench bytecode` compares front-end/load cost
and repeated invocation throughput against the walker. Set
`CHECKMATE_BENCH_ITERATIONS` to control repetitions. Results depend on the
workload and machine; compilation is an additional cost paid at load time.
