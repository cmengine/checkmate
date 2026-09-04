# CME — Checkmate Engine

CME is a statically typed, embeddable scripting language implemented in Rust. The project is in an early foundation-building stage: it has a working front-end, a tree-walking interpreter that runs the basic subset end to end, and a CLI; the runtime remains a placeholder.

## Current Status

- `cme-core` defines the spanned AST, including primitive/inferred/void types, literals, identifiers, variable declarations, functions, calls, if/else, while, and return.
- `cme-compiler` provides the lexer, parser, diagnostics, validator, and one-call `parse_source`. Functions, if/else, while, and return now parse. The type checker enforces Appendix A for the basic subset (§A.4–§A.7 operator typing, §2.10–§2.16 declarations, functions, control flow, and `infer` crystallization).
- `cme-interp` runs the basic subset end to end: plain Rust values, overflow-checked arithmetic, truncating division, short-circuit logic, §A.6 string concatenation, and a fixed 1024 call-depth limit. `cme-runtime` is a placeholder.
- The `cme` CLI has working `lex`, `ast`, `check`, and `run` commands with rendered diagnostics; `run` refuses to execute any program that produced a diagnostic.

The language specification is maintained in [`WHITEPAPER.md`](./WHITEPAPER.md).

## Workspace

The repository is a Cargo workspace with focused crates:

| Crate | Purpose | Status |
| --- | --- | --- |
| `cme-core` | Shared AST and language data models | Initial implementation |
| `cme-compiler` | Lexer, parser, diagnostics, validator, type checker, and `parse_source` | Working front-end |
| `cme-interp` | Interpreter | Working tree-walking interpreter (basic subset) |
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
