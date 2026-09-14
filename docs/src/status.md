# Status of This Documentation

Checkmate is an early-stage language under active development. The
[whitepaper](https://github.com/cmengine/checkmate/blob/mom/WHITEPAPER.md)
specifies the full design; this book documents what is *implemented and
working today*. Where the two differ, this page is the map.

Everything documented in the body chapters of this book — unless a callout
says otherwise — reflects the current implementation. Use this page to
check any specific area at a glance.

## Implemented and documented here

| Area | Status | Where |
| --- | --- | --- |
| Lexical structure: comments, identifiers, literals, newlines | <span class="badge badge-implemented">Implemented</span> | [Lexical Structure](language/lexical-structure.md) |
| Scalar types, structs, enums, generics, arrays, maps | <span class="badge badge-implemented">Implemented</span> | [Types](language/types.md) |
| `option<T>` / `result<T, E>`, the `?` operator | <span class="badge badge-implemented">Implemented</span> | [Error Handling](language/error-handling.md) |
| `match` with patterns, destructuring, exhaustiveness | <span class="badge badge-implemented">Implemented</span> | [Pattern Matching](language/match.md) |
| `infer` type crystallization | <span class="badge badge-implemented">Implemented</span> | [Variables](language/variables.md) |
| `impl` blocks on local types and host-style dotted paths | <span class="badge badge-implemented">Implemented</span> | [`impl` Blocks](language/impl-blocks.md) |
| Full Appendix A operator surface, including newline rules | <span class="badge badge-implemented">Implemented</span> | [Operators](language/operators.md) |
| String interpolation `$"..."` | <span class="badge badge-implemented">Implemented</span> | [Strings](language/strings.md) |
| Multi-file mods (`mod.toml`, `src/`, `import self.*`) | <span class="badge badge-implemented">Implemented</span> | [Mods](mods/mods.md) |
| Megaprogramming: `grammar`, `mega`, pattern language, templates, `@` functions, heredocs | <span class="badge badge-implemented">Implemented</span> | [Megaprogramming](mega/overview.md) |
| Standard grammar library (`std.json`, `std.yaml`, …) | <span class="badge badge-implemented">Implemented</span> | [Standard Grammar Library](mega/std-library.md) |
| Schema system: namespaces, capabilities, interfaces, `requires`, `since`/`optional`, version gating | <span class="badge badge-implemented">Implemented</span> | [Schema System](schema/overview.md) |
| Compile-time-verified Rust bindings (`cme_schema_bindings!`) | <span class="badge badge-implemented">Implemented</span> | [Schema-Driven Embedding](embedding/schema-embedding.md) |
| Generated C headers (`cme codegen-c`) | <span class="badge badge-implemented">Implemented</span> | [Schema-Driven Embedding](embedding/schema-embedding.md) |
| Rust host API (`Engine` / `CompiledProgram` / `Context`) | <span class="badge badge-implemented">Implemented</span> | [Embedding in Rust](embedding/rust.md) |
| C host API (`cme.h`, `cm_*` ABI) | <span class="badge badge-implemented">Implemented</span> | [Embedding in C](embedding/c.md) |
| Execution limits: fuel, deadline, call depth; reentrancy guard | <span class="badge badge-implemented">Implemented</span> | [Limits and Errors](appendix/limits-and-errors.md) |
| CLI: `lex`, `ast`, `check`, `run`, `expand`, `schema`, `codegen-c`, `lsp` | <span class="badge badge-implemented">Implemented</span> | [The `cme` CLI](tooling/cli.md) |
| Language server (stdio, incremental) with schema- and mega-aware features | <span class="badge badge-implemented">Implemented</span> | [Language Server](tooling/lsp.md) |

## Specified, but not yet implemented

These areas appear in the whitepaper but **must not be relied on** in
current code. This book mentions them only in sidebars like this one.

| Area | Status | Notes |
| --- | --- | --- |
| Bytecode VM and AOT/LLVM compilation | <span class="badge badge-future">Future</span> | The current engine is a tree-walking interpreter. §5.5 fuel/deadline/depth limits are enforced by it today. |
| `suspend` members and transparent async (§4) | <span class="badge badge-future">Future</span> | `suspend` is rejected at parse time. The future-polling shape of the C API is already in place. |
| Native artifact caching (§5.4) | <span class="badge badge-future">Future</span> | Depends on the AOT backend. |
| `no_std` freestanding runtime (§6) | <span class="badge badge-future">Future</span> | `cme-runtime` is a placeholder crate. |
| Official formatter, `--auto-crystallize` (§2.16.1) | <span class="badge badge-future">Future</span> | Not shipped. |
| Debug Adapter Protocol (`cme-dap`) (§14) | <span class="badge badge-future">Future</span> | Not shipped. |
| Allocation budgets as a host limit (§5.5) | <span class="badge badge-future">Future</span> | Fuel, deadline, and call depth exist today; allocation budgets do not. |
| `SuspendState`-style continuation syntax | <span class="badge badge-future">Future</span> | Whitepaper illustration of the future lowering; not surface syntax. |

## Small divergences worth knowing

- **Boundary capitalization (§2.5) is enforced when a schema is active.**
  With no schema registered, the checker keeps the pre-schema behavior and
  does not reject mis-capitalized top-level names. Follow the convention
  anyway; the schema gate will enforce it.
- **`main` is a CLI convention.** §2.1 is absolute — the language has no
  implicit entry point. `cme run` happens to invoke the function named
  `main` (in a mod, it may live in any module); every other host picks its
  own targets explicitly.
- **Array `.length` is the only collection built-in today.** The whitepaper
  §11 core library (string utilities, math helpers) is specified but the
  `cme-runtime` crate is a placeholder, so there are no other built-in
  functions yet.
- **Megaprogram expansion is text-level.** `cme expand` writes real
  Checkmate source, and cross-file macro imports (invoking another module's
  mega by qualified name) are not yet supported.

## Versions

- The whitepaper is versioned in-document (currently **0.6**).
- The implementation workspace version lives in the root `Cargo.toml`.
- Schema contracts are versioned per namespace with `since` tags and mod
  target versions — see [Versioning](schema/versioning.md).

Docs sync policy: any change to the language, toolchain, or host APIs must
check this book for drift in the same change (see `AGENTS.md`). If you find
a page contradicting the implementation, please
[open an issue](https://github.com/cmengine/checkmate/issues).
