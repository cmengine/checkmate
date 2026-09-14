# Installation

Checkmate ships as a single Rust workspace. There are no releases to
download yet — you build the toolchain from source with
[Rust](https://rustup.rs) (edition 2024, so use a recent stable toolchain).

## Prerequisites

- **Rust** 1.85 or newer, via [rustup](https://rustup.rs). Check with
  `rustc --version`.
- A C compiler is only needed if you plan to embed Checkmate from C (see
  [Embedding in C](../embedding/c.md)); building the toolchain itself is pure
  Rust.

## Build the CLI

```sh
git clone https://github.com/cmengine/checkmate.git
cd checkmate
cargo build --features cli
```

The binary lands at `target/debug/cme`. Verify it:

```sh
cargo run --features cli -- check basic.cm
```

`basic.cm` is a zero-diagnostics pin fixture at the repository root; a
silent exit means the toolchain works.

> The root `cme` package is a facade: its default build intentionally
> exposes no library APIs. The `cli` feature enables the command-line
> toolchain; `api` and `schema-macro` enable the host-embedding surface.
> See [Embedding in Rust](../embedding/rust.md) for the dependency shape hosts
> use.

## The commands you will actually use

| Command | Purpose |
| --- | --- |
| `cme check <file-or-mod>` | Type-check a file or a whole mod tree; render diagnostics |
| `cme run <file-or-mod>` | Check, then invoke `main` and print the result |
| `cme expand <file>` | Show what megaprograms expand to |
| `cme lsp` | Run the language server over stdio |
| `cme schema <file.cm>` | Validate a schema file |
| `cme codegen-c <schema.cm>` | Emit a C header from a schema |

The full surface is documented in [The `cme` CLI](../tooling/cli.md).

## Editors

Official editor support lives in-tree:

- **Zed**: the dev extension under `editors/zed` — see its
  [README](https://github.com/cmengine/checkmate/tree/mom/editors/zed).
- **VS Code / Sublime / GitHub**: the TextMate grammar under
  `editors/textmate`.
- Any LSP-capable editor: launch `cme lsp` and point your client at it.
  The Zed extension does this automatically.

## Try the fixtures

The repository root doubles as a test bench. Three fixtures are worth
running on day one:

```sh
cargo run --features cli -- run syntax.cm      # the full language surface, self-checking
cargo run --features cli -- run mega.cm        # megaprogramming showcase (HTML, CSS, JS, YAML, ...)
cargo run --features cli -- check boom.cm      # error-recovery stress fixture
```

`cme run syntax.cm` prints a report and a final `failures=0` line — every
language feature exercised and passing on your machine.

## Next steps

- [Hello, Checkmate](hello-world.md) writes and runs your first program.
- [A Ten-Minute Tour](tour.md) walks the whole language quickly.
