# CME — Checkmate Engine

CME is a statically typed, embeddable scripting language implemented in Rust. The project is in an early foundation-building stage: it now has the first AST, lexer, and parser building blocks, while the interpreter, runtime, and CLI remain placeholders.

## Current Status

- `cme-core` defines the initial AST, including primitive/inferred types, literals, identifiers, and variable declarations.
- `cme-compiler` provides the first lexer and recursive-descent parser for the current language subset.
- `cme-interp` and `cme-runtime` are placeholders.
- The `cme` CLI is a placeholder toolchain entry point.

The language specification is maintained in [`WHITEPAPER.md`](./WHITEPAPER.md).

## Workspace

The repository is a Cargo workspace with focused crates:

| Crate | Purpose | Status |
| --- | --- | --- |
| `cme-core` | Shared AST and language data models | Initial implementation |
| `cme-compiler` | Lexer and parser | Initial implementation |
| `cme-interp` | Interpreter | Placeholder |
| `cme-runtime` | Runtime services and built-ins | Placeholder |
| `cme` | Facade package and optional CLI | Initial facade; placeholder CLI |

The root `cme` package exposes workspace crates through optional `core`, `compiler`, `interp`, and `runtime` features. Enabling `cli` enables all of them. The default build intentionally exposes no root APIs.

## Development

Build the placeholder toolchain:

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
