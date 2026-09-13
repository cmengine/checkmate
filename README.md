# CME — Checkmate Engine

CME is a statically typed, embeddable scripting language implemented in Rust. The full whitepaper language surface parses, type-checks, and runs end to end on the tree-walking interpreter, the §8 megaprogramming system (grammar-driven `magic` macros with a packrat pattern engine and compile-time function evaluation) is implemented at the source-text level with its own `cme expand` command, multi-file mods (§10) load, link, and run from a `mod.toml` + `src/` tree, and the §9 schema system gates host/script contracts with compile-time-verified bindings for Rust and C hosts. The bytecode VM and AOT compiler remain future work.

## Current Status

- `cme-core` defines the spanned AST for the full language surface: primitive/inferred/void types, named and generic types, arrays and maps, literals, identifiers, variable declarations, functions, calls with positional and named arguments, structs and enums with type parameters, enum variant construction, impl blocks with function members, qualified and dotted-path calls, field and index access, `match` (statement and expression forms with patterns), `for`-in, `if`/`else`, `while`, `return`, the `?` operator, interpolated strings, and lvalue assignment targets.
- `cme-compiler` provides the lexer (including block comments and `$"..."` interpolation tokens), parser, diagnostics, validator, and one-call `parse_source` for the full surface. The type checker resolves struct and enum declarations with generics, the built-in `option<T>` / `result<T, E>`, arrays and maps; it enforces §2.6–§2.16, §10.4 (impl targets, member unions and duplicates, member bodies), §11, and Appendix A, including match exhaustiveness, `?` error-type agreement, named-argument coverage, and §2.16 crystallization.
- `cme-interp` runs the full language surface end to end: plain Rust values with structural equality for structs/enums/arrays/maps, overflow-checked arithmetic, truncating division, short-circuit logic, §A.6 stringification, CMON-style display, value-semantics cloning, impl member and path calls (§10.4), named arguments bound by name, and a fixed 1024 call-depth limit. Hosts invoke interface members through `Interpreter::invoke_member`. `cme-runtime` is a placeholder.
- The `cme` CLI has working `lex`, `ast`, `check`, and `run` commands with rendered diagnostics; `run` refuses to execute any program that produced a diagnostic.
- Megaprogramming (§8) is implemented in `cme-compiler::mega`: `grammar` and `magic` declarations, invocation regions balanced under composed lexical profiles (islands and heredoc regions included), a packrat pattern engine with furthest-failure diagnostics, indentation-aware blocks, fragment validators, parse-integrated `$expr`/`$type`/`$block`/`$raw`/`$template` extents, `@`-function compile-time evaluation (§8.5), fixpoint expansion with a depth cap, `grammar ts extends js` inheritance, and profile static checks. `magic.cm` at the repository root is the living fixture: HTML, CSS, JS, Python, JSON, YAML, TOML, SQL, and RE megaprograms, all expanding and running.
- `syntax.cm` at the repository root is the full-language fixture: it exercises every whitepaper syntax feature — including §10.4 impl blocks on structs, enums, and host-style dotted path targets — and returns 0 when all of its internal checks pass. `basic.cm` remains the zero-diagnostics pin for the front end and `boom.cm` the recovery stress fixture.

The language specification is maintained in [`WHITEPAPER.md`](./WHITEPAPER.md).

## Editor Support: one grammar surface, three consumers

Checkmate ships its own editor tooling in-tree, next to the compiler it must
stay in sync with:

| Directory | What it is | Who consumes it |
| --- | --- | --- |
| `grammars/tree-sitter-checkmate` | Tree-sitter grammar + corpus tests + external heredoc scanner | Zed extension, Neovim/Helix/Emacs |
| `editors/zed` | Zed extension: highlighting, brackets, outline, indents, text objects | Zed (`extensions: Install Dev Extension` → `editors/zed`) |
| `editors/textmate` | TextMate grammar (`source.checkmate`) + installable VS Code wrapper | VS Code, Sublime, GitHub |

All three cover the full implemented surface — the core language, §8
megaprogramming (`grammar`/`magic` declarations, the pattern language,
expansion templates, brace-balanced invocation regions, heredocs), and §9
schema files. The tree-sitter grammar is validated against every fixture in
this repository: `syntax.cm`, `magic.cm`, the schema files, and the mod trees
all parse without error nodes (the deliberately damaged recovery fixtures
`boom.cm`, `broken_syntax.cm`, `tests/fixtures/recovery/*` are expected to
produce them).

Build and test the grammar:

```sh
cd grammars/tree-sitter-checkmate
tree-sitter generate && tree-sitter test
```

See `editors/zed/README.md` and `editors/textmate/README.md` for
per-editor install instructions.


## Megaprogramming: `cme expand`

Files that declare or invoke `magic` are expanded automatically before `check`/`run` (the source behaves exactly like its expansion). To see the generated program itself:

```sh
cargo run --features cli -- expand magic.cm
```

This writes `magic_expanded.cm` side by side with the original — pure Checkmate, every magic invocation replaced by its generated code — and then parses and checks that file. Add `--provenance` to annotate each root magic site with a `// @ magic(name) src:line:col` comment; without the flag the output is byte-deterministic. Expansion runs per file inside a mod too: each module's megaprograms expand before the mod links into one program.

## Language Server: `cme lsp`

The developer toolchain ships as one binary, and the language server rides in it:

```sh
cargo run --features cli -- lsp
```

`cme lsp` speaks LSP over stdio (WHITEPAPER §14). Diagnostics stream on open and edit — lexer, parser, validator, type checker, §8 megaprogram expansion, and §9 schema pipelines all report into the editor. Hover shows resolved signatures and crystallized `infer` types, completion covers struct fields and enum variants after `.`, named arguments inside calls, import segments, and scope names, and go-to-definition, find-references, document symbols, and semantic tokens are shadowing-aware. Files that mention megaprograms surface expansion diagnostics (anchored in the original text) plus a `cme/expand` custom request returning the expansion preview; their parse/check spans live in expanded-text coordinates, so they are not published into such buffers. The analysis is incremental through [salsa](https://github.com/salsa-rs/salsa): every document is a query input, and an edit recomputes only what it invalidated. The [Zed extension](editors/zed/README.md) launches `cme lsp` automatically.

## Host Embedding APIs (Rust and C)

Checkmate embeds through the WHITEPAPER §13 host APIs over the shipped front end and tree-walking interpreter. Programs load from a source text, a `.cm` file, or a whole §10 mod tree; every load applies the same gate the CLI applies (megaprogram expansion when present, parse, standalone-import check, type check), so a program that produced a diagnostic is never invocable. Contexts carry the §5.5 execution limits — fuel, wall-clock deadline, call depth — per invocation, and hosts invoke top-level functions or §10.4 impl members (`engine.gamemode.OnTick`-style) — with the §9 schema system active, loads gate on the registered contract and capability calls dispatch to host providers. Invocations also enforce the §5.7 reentrancy prohibition: a capability dispatched mid-invocation cannot invoke back into the same context (the nested call fails with `ErrorKind::Reentrant`; the C ABI reports it as `CM_ERROR_INVALID_ARG`), while different contexts and other threads stay free.

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

## The Schema System (§9): contracts with compile-time-verified host bindings

Hosts declare their contract in `.cm` schema files — one namespace root per file (§9.2), capabilities the host provides, interfaces scripts implement with `impl` blocks, `since`/`optional` versioning (§9.5), and `requires` edges (§9.4):

```checkmate
// schemas/engine.cm
schema engine v1.4.0

struct TextureHandle {
    int id
}

capability graphics {
    since 1.0.0 TextureHandle LoadTexture(str path)
}

interface gamemode {
    since 1.0.0 GameState InitGame(GameConfig config)
}
```

Register the schema (with the engine or the CLI's `--schema` flag) and every load enforces the contract at compile time: capability calls type-check against the schema signatures and require the import, `import engine.assets` itself demands the capability's prerequisite (the §9.4 "cannot import or call"), `impl engine.gamemode` blocks must implement every required visible member with the exact signature, an impl naming an ungranted or unknown namespace is an error, members introduced after the program's target version are hidden (§9.5), and `requires` edges gate both directions. Schema-authoring defects are rejected too: an `optional` capability member (§9.5 defines the flag for interfaces) and a member tagged `since` beyond the schema's own version both fail validation.

**Rust hosts get compile-time-verified bindings** (§9.6) through a procedural macro that runs the real schema parser at host build time:

```rust
cme_schema_bindings!("schemas/engine.cm"); // generates the `engine` module

struct HostGraphics;
impl engine::EngineGraphicsCapability for HostGraphics {
    fn load_texture(&self, path: String) -> engine::TextureHandle { /* ... */ }
    // missing members or wrong signatures = host COMPILE error
}

let mut engine = cme::Engine::new();
engine.register_schema(engine::schema())?;          // same file, no re-parse
engine::register_engine_graphics(&mut engine, std::sync::Arc::new(HostGraphics))?;
```

The macro also generates typed interface proxies for host → script calls and native Rust structs/enums for the §9.3 boundary types, packing and unpacking script values by name. Proxies construct through `engine::EngineGamemodeProxy::new(&context)` or the WHITEPAPER §13.1 shape, `context.get_interface::<EngineGamemodeProxy>()?` — construction fails when the loaded program never implemented the interface. Enable the facade's `schema-macro` feature.

**C hosts get a generated header**: `cme codegen-c schemas/engine.cm` emits function-pointer typedefs per capability member, a vtable, and a registration macro whose `_Static_assert`s (via `_Generic`) verify every host implementation's signature AT COMPILE TIME — a missing member fails the preprocessor, a wrong signature fails the assert. Interface members become exact-arity invocation helpers, and schema structs/enums gain pack/unpack helpers over the `cm_value` ABI (primitives, owned `char*` strings, nested schema types by address; container shapes ride the generic accessors). One header per namespace; several headers coexist in one translation unit. The header is not just a demo: `cme-ffi`'s build generates it from `apps/c_host/engine.cm` and the C host application compiles against it, so `cargo test` exercises the full flow.

A program that calls a capability with no registered provider fails the LOAD — wiring mistakes are deterministic before any invocation, and capability calls at runtime dispatch to the provider through an engine-agnostic boundary the tree walker uses today and the bytecode VM will use unchanged.

`apps/rust_host` is a working Rust consumer (`cme-rust-host <file.cm | mod_dir> <entry> [args...] --fuel N --deadline-ms N --depth N --schema <schema.cm>`, plus `--schema-demo` for the full §9.6 generated-bindings walkthrough over its own `schemas/game.cm` — the host implements the macro-generated capability trait, so its signatures are verified at the app's compile time), and `apps/c_host` is a working C consumer with a Makefile — the same C file also runs inside `cargo test` via cme-ffi's build script, which generates the schema header the consumer includes, so the workspace test run exercises the real C client AND its compile-time-verified bindings end to end.

## Workspace

The repository is a Cargo workspace with focused crates:

| Crate | Purpose | Status |
| --- | --- | --- |
| `cme-core` | Shared AST and language data models | Full language surface |
| `cme-compiler` | Lexer, parser, diagnostics, validator, type checker, `parse_source`, the `mega` megaprogram subsystem, and the `mods` multi-file mod loader | Working front-end for the full surface plus §8 megaprogramming and §10 mods |
| `cme-interp` | Interpreter | Working tree-walking interpreter (full surface) |
| `cme-api` | Rust host embedding API (§13.1): `Engine`, `CompiledProgram`, `Context`, `ExecutionLimits`, schema registration, capability providers | Working over source files and mod trees, schema-gated |
| `cme-ffi` | Stable C host API (§13.2): `cme.h`, `cm_*` ABI, staticlib + cdylib, schema + provider surface | Working; the C host app runs in `cargo test` |
| `cme-schema-macro` | `cme_schema_bindings!` — compile-time-verified Rust host bindings from `.cm` schema files (§9.6) | Working; capability traits, proxies, descriptors |
| `cme-lsp` | Language server (`cme lsp`): salsa incremental analysis, real-time diagnostics, hover, completion, definition, references, document symbols, semantic tokens, megaprogram expansion preview | Working over single files and §9 schema files; serves stdio from the unified `cme` binary |
| `cme-runtime` | Runtime services and built-ins | Placeholder |
| `cme` | Facade package and optional CLI | Working lex/ast/check/run/expand/schema/codegen-c toolchain; check/ast/run accept mod directories and `--schema` contracts |
| `apps/rust_host` | Rust host application (§13.1 consumer) | Loads file/mod, limits flags, `--schema` registration, and `--schema-demo`: the full generated-bindings flow (compile-time-verified provider, typed proxies) |
| `apps/c_host` | C host application (§13.2 consumer) | 246 self-checks incl. the schema flow, the §5.7 reentrancy rejection, AND the generated schema header consumed for real, run by `cargo test` and standalone |

The root `cme` package exposes workspace crates through optional `core`, `compiler`, `interp`, `runtime`, `api`, `schema-macro`, and `lsp` features. Enabling `cli` enables the toolchain crates plus the language server; `api` enables the Rust host API (flat re-exports `Engine`, `ExecutionLimits`, `CompiledProgram`, `Context`, `Value`); `schema-macro` enables `cme::cme_schema_bindings`. The default build intentionally exposes no root APIs.

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
