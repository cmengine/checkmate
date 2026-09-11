# CME — Checkmate Engine

CME is a statically typed, embeddable scripting language implemented in Rust. The full whitepaper language surface parses, type-checks, and runs end to end on the tree-walking interpreter, the §8 megaprogramming system (grammar-driven `magic` macros with a packrat pattern engine and compile-time function evaluation) is implemented at the source-text level with its own `cme expand` command, and multi-file mods (§10) load, link, and run from a `mod.toml` + `src/` tree. The bytecode VM, AOT compiler, and host schema system remain future work.

## Current Status

- `cme-core` defines the spanned AST for the full language surface: primitive/inferred/void types, named and generic types, arrays and maps, literals, identifiers, variable declarations, functions, calls with positional and named arguments, structs and enums with type parameters, enum variant construction, impl blocks with function members, qualified and dotted-path calls, field and index access, `match` (statement and expression forms with patterns), `for`-in, `if`/`else`, `while`, `return`, the `?` operator, interpolated strings, and lvalue assignment targets.
- `cme-compiler` provides the lexer (including block comments and `$"..."` interpolation tokens), parser, diagnostics, validator, and one-call `parse_source` for the full surface. The type checker resolves struct and enum declarations with generics, the built-in `option<T>` / `result<T, E>`, arrays and maps; it enforces §2.6–§2.16, §10.4 (impl targets, member unions and duplicates, member bodies), §11, and Appendix A, including match exhaustiveness, `?` error-type agreement, named-argument coverage, and §2.16 crystallization.
- `cme-interp` runs the full language surface end to end: plain Rust values with structural equality for structs/enums/arrays/maps, overflow-checked arithmetic, truncating division, short-circuit logic, §A.6 stringification, CMON-style display, value-semantics cloning, impl member and path calls (§10.4), named arguments bound by name, and a fixed 1024 call-depth limit. Hosts invoke interface members through `Interpreter::invoke_member`. `cme-runtime` is a placeholder.
- The `cme` CLI has working `lex`, `ast`, `check`, and `run` commands with rendered diagnostics; `run` refuses to execute any program that produced a diagnostic.
- Megaprogramming (§8) is implemented in `cme-compiler::mega`: `grammar` and `magic` declarations, invocation regions balanced under composed lexical profiles (islands and heredoc regions included), a packrat pattern engine with furthest-failure diagnostics, indentation-aware blocks, fragment validators, parse-integrated `$expr`/`$type`/`$block`/`$raw`/`$template` extents, `@`-function compile-time evaluation (§8.5), fixpoint expansion with a depth cap, `grammar ts extends js` inheritance, and profile static checks. `magic.cm` at the repository root is the living fixture: HTML, CSS, JS, Python, JSON, YAML, TOML, SQL, and RE megaprograms, all expanding and running.
- `syntax.cm` at the repository root is the full-language fixture: it exercises every whitepaper syntax feature — including §10.4 impl blocks on structs, enums, and host-style dotted path targets — and returns 0 when all of its internal checks pass. `basic.cm` remains the zero-diagnostics pin for the front end and `boom.cm` the recovery stress fixture.

The language specification is maintained in [`WHITEPAPER.md`](./WHITEPAPER.md).

## Megaprogramming: `cme expand`

Files that declare or invoke `magic` are expanded automatically before `check`/`run` (the source behaves exactly like its expansion). To see the generated program itself:

```sh
cargo run --features cli -- expand magic.cm
```

This writes `magic_expanded.cm` side by side with the original — pure Checkmate, every magic invocation replaced by its generated code — and then parses and checks that file. Add `--provenance` to annotate each root magic site with a `// @ magic(name) src:line:col` comment; without the flag the output is byte-deterministic. Expansion runs per file inside a mod too: each module's megaprograms expand before the mod links into one program.

## Host Embedding APIs (Rust and C)

Checkmate embeds through the WHITEPAPER §13 host APIs over the shipped front end and tree-walking interpreter. Programs load from a source text, a `.cm` file, or a whole §10 mod tree; every load applies the same gate the CLI applies (megaprogram expansion when present, parse, standalone-import check, type check), so a program that produced a diagnostic is never invocable. Contexts carry the §5.5 execution limits — fuel, wall-clock deadline, call depth — per invocation, and hosts invoke top-level functions or §10.4 impl members (`engine.gamemode.OnTick`-style), which is the same surface the schema system will gate once it lands.

The Rust shape (behind the facade's `api` feature):

```rust
use cme::{Engine, ExecutionLimits};

let engine = Engine::new();
let program = engine.load_source("int main() {\nreturn 40 + 2\n}\n")?;
let context = engine.create_context(&program, ExecutionLimits {
    fuel: Some(1_000_000),
    deadline_ms: Some(50),
    max_call_depth: 64,
});
let answer = context.invoke("main", &[])?; // Value::Int(42)
# Ok::<(), Box<dyn std::error::Error>>(())
```

The C shape (`crates/cme-ffi/include/cme.h`, built as `libcme_ffi.a` / `.so`):

```c
cm_context_t* ctx = cm_engine_create_context(engine, program, &limits);
cm_future_t* future = cm_invoke(ctx, "engine.gamemode", "OnTick", NULL, 0);
cm_poll_result_t result;
while ((result = cm_future_poll(future, NULL)) == CM_PENDING) {
    /* drive host IO; unreachable over the synchronous interpreter */
}
if (result == CM_ERROR) {
    cm_error_t err = cm_future_get_error(future);
    printf("Script execution failed: %s\n", err.message);
    cm_error_free(&err);
}
cm_future_destroy(future);
```

`apps/rust_host` is a working Rust consumer (`cme-rust-host <file.cm | mod_dir> <entry> [args...] --fuel N --deadline-ms N --depth N`), and `apps/c_host` is a working C consumer with a Makefile — the same C file also runs inside `cargo test` via cme-ffi's build script, so the workspace test run exercises the real C client end to end.

## Workspace

The repository is a Cargo workspace with focused crates:

| Crate | Purpose | Status |
| --- | --- | --- |
| `cme-core` | Shared AST and language data models | Full language surface |
| `cme-compiler` | Lexer, parser, diagnostics, validator, type checker, `parse_source`, the `mega` megaprogram subsystem, and the `mods` multi-file mod loader | Working front-end for the full surface plus §8 megaprogramming and §10 mods |
| `cme-interp` | Interpreter | Working tree-walking interpreter (full surface) |
| `cme-api` | Rust host embedding API (§13.1): `Engine`, `CompiledProgram`, `Context`, `ExecutionLimits` | Working over source files and mod trees |
| `cme-ffi` | Stable C host API (§13.2): `cme.h`, `cm_*` ABI, staticlib + cdylib | Working; the C host app runs in `cargo test` |
| `cme-runtime` | Runtime services and built-ins | Placeholder |
| `cme` | Facade package and optional CLI | Working lex/ast/check/run/expand toolchain; check/ast/run accept mod directories |
| `apps/rust_host` | Rust host application (§13.1 consumer) | Working `cme-rust-host` CLI |
| `apps/c_host` | C host application (§13.2 consumer) | 208 self-checks, run by `cargo test` and standalone |

The root `cme` package exposes workspace crates through optional `core`, `compiler`, `interp`, `runtime`, and `api` features. Enabling `cli` enables the toolchain crates; `api` enables the Rust host API (flat re-exports `Engine`, `ExecutionLimits`, `CompiledProgram`, `Context`, `Value`). The default build intentionally exposes no root APIs.

## Development

Build the toolchain:

```sh
cargo build --features cli
```

Run tests:

```sh
cargo test --workspace
```

Run tests with the complete facade enabled:

```sh
cargo test --workspace --features cli
```

Check formatting and lints:

```sh
cargo fmt
cargo clippy --workspace --all-targets
```
