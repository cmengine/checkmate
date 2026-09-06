# CME — Checkmate Engine

CME is a statically typed, embeddable scripting language implemented in Rust. The project is in an early foundation-building stage: the full whitepaper language surface parses, type-checks, and runs end to end on the tree-walking interpreter; the bytecode VM, AOT compiler, host schema system, and megaprogramming remain future work.

## Current Status

- `cme-core` defines the spanned AST for the full language surface: primitive/inferred/void types, named and generic types, arrays and maps, literals, identifiers, variable declarations, functions, calls with positional and named arguments, structs and enums with type parameters, enum variant construction, impl blocks with function members, qualified and dotted-path calls, field and index access, `match` (statement and expression forms with patterns), `for`-in, `if`/`else`, `while`, `return`, the `?` operator, interpolated strings, and lvalue assignment targets.
- `cme-compiler` provides the lexer (including block comments and `$"..."` interpolation tokens), parser, diagnostics, validator, and one-call `parse_source` for the full surface. The type checker resolves struct and enum declarations with generics, the built-in `option<T>` / `result<T, E>`, arrays and maps; it enforces §2.6–§2.16, §10.4 (impl targets, member unions and duplicates, member bodies), §11, and Appendix A, including match exhaustiveness, `?` error-type agreement, named-argument coverage, and §2.16 crystallization.
- `cme-interp` runs the full language surface end to end: plain Rust values with structural equality for structs/enums/arrays/maps, overflow-checked arithmetic, truncating division, short-circuit logic, §A.6 stringification, CMON-style display, value-semantics cloning, impl member and path calls (§10.4), named arguments bound by name, and a fixed 1024 call-depth limit. Hosts invoke interface members through `Interpreter::invoke_member`. `cme-runtime` is a placeholder.
- The `cme` CLI has working `lex`, `ast`, `check`, and `run` commands with rendered diagnostics; `run` refuses to execute any program that produced a diagnostic.
- `syntax.cm` at the repository root is the full-language fixture: it exercises every whitepaper syntax feature — including §10.4 impl blocks on structs, enums, and host-style dotted path targets — and returns 0 when all of its internal checks pass. `basic.cm` remains the zero-diagnostics pin for the front end and `boom.cm` the recovery stress fixture.

The language specification is maintained in [`WHITEPAPER.md`](./WHITEPAPER.md).

## Workspace

The repository is a Cargo workspace with focused crates:

| Crate | Purpose | Status |
| --- | --- | --- |
| `cme-core` | Shared AST and language data models | Full language surface |
| `cme-compiler` | Lexer, parser, diagnostics, validator, type checker, and `parse_source` | Working front-end for the full surface |
| `cme-interp` | Interpreter | Working tree-walking interpreter (full surface) |
| `cme-runtime` | Runtime services and built-ins | Placeholder |
| `cme` | Facade package and optional CLI | Working lex/ast/check/run toolchain |

The root `cme` package exposes workspace crates through optional `core`, `compiler`, `interp`, and `runtime` features. Enabling `cli` enables all of them. The default build intentionally exposes no root APIs.

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
