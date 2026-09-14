# The `cme` CLI

`cme` is the unified toolchain binary. Build it with:

```sh
cargo build --features cli
# binary at target/debug/cme (or target/release/cme)
```

## Command overview

```text
Usage: cme <lex|ast|check|run|expand> <file.cm> [--provenance] [--schema <schema.cm>]
       cme <check|ast|run> <mod_dir | path/to/mod.toml> [--schema <schema.cm>]
       cme schema <schema.cm>
       cme codegen-c <schema.cm>
       cme lsp
```

| Command | Input | Purpose |
| --- | --- | --- |
| `lex` | single `.cm` | Dump the token stream (debugging the front end) |
| `ast` | file or mod | Dump the parsed AST |
| `check` | file or mod | Type-check and render diagnostics; exit non-zero on any diagnostic |
| `run` | file or mod | Check, then invoke `main` and print its value |
| `expand` | single `.cm` | Expand megaprograms and write `<stem>_expanded.cm` |
| `schema` | `.cm` schema | Validate one schema file |
| `codegen-c` | `.cm` schema | Emit the generated C host header to stdout |
| `lsp` | — | Serve the language server over stdio |

## The `--schema` flag

`--schema <file.cm>` registers a §9 schema contract; the flag is
**repeatable** and may appear anywhere:

```sh
cme check --schema schemas/engine.cm program.cm
cme run --schema schemas/engine.cm --schema schemas/physics.cm my_mod/
```

- Schema-aware loads gate imports, capability calls, `impl`
  completeness, `since` version hiding, and boundary capitalization —
  see [The Schema System](../schema/overview.md).
- In mod mode, the manifest's `[schemas]` table **narrows** the grant to
  the listed namespaces at their target versions.
- Diagnostics always render against the **resolved file argument** (the
  `--schema` pairs are consumed first, so
  `cme check --schema s.cm x.cm` renders `x.cm:…`).

## `check`

The workhorse. Parses, validates, type-checks, and renders diagnostics —
nothing executes:

```sh
cme check syntax.cm
cme check my_mod/
```

Exit code is non-zero when any diagnostic exists; the output is
`path:line:column: message` lines (mod diagnostics name their owning
module).

## `run`

`run` **never executes a program that produced a diagnostic** — the gate
runs first, always. On a clean program it invokes the function named
`main` (the CLI's convention; §2.1 means the language itself has no
entry point) with no arguments, and prints the result:

- `void main()` prints nothing;
- any other return value prints in the canonical
  [CMON](../language/strings.md#cmon-checkmate-object-notation) form —
  `42`, `"hello"`, `Player(name: "Hero", health: 100)`, arrays, maps,
  enums.

Runtime failures (overflow, division by zero, missing key, fuel
exhaustion) terminate the invocation with a clean positioned error —
never a crash.

```sh
cme run syntax.cm        # the self-checking full-surface fixture
cme run my_mod/          # main may live in any module of the mod
```

## `expand`

Files that declare or invoke megaprograms expand automatically before
`check`/`run`. `expand` shows the result:

```sh
cme expand mega.cm                 # writes mega_expanded.cm next to the original
cme expand mega.cm --provenance    # also annotates each root mega site:
                                   # // @ name! src:line:col
```

Without `--provenance` the output is byte-deterministic. The expanded
file is pure Checkmate — every megaprogram invocation replaced by its
generated code — and the command then parses and checks it. `lex` and
`expand` are the only single-file commands; `check`, `ast`, and `run`
also accept mods (expansion runs per module before linking).

## `schema`

Validates one schema file: parse defects, member shapes, `since`
beyond the schema's own version, `optional` on capabilities, unresolved
`requires` — everything that would fail registration:

```sh
cme schema schemas/engine.cm
```

## `codegen-c`

Emits the generated C host header for a schema to stdout:

```sh
cme codegen-c schemas/engine.cm > engine_schema_gen.h
```

Byte-deterministic output; see
[Schema-Driven Embedding](../embedding/schema-embedding.md) for what the
header contains and how hosts consume it.

## `lsp`

Starts the language server over stdio — see [The Language Server](lsp.md).
Editors that speak LSP launch it automatically (the Zed extension does).

## Exit codes and errors

- Diagnostic-bearing programs: non-zero exit, rendered diagnostics on
  output — never partial execution.
- IO failures (unreadable files): distinct error with the path.
- Usage mistakes: the usage text.

## Environment notes

- `*_expanded.cm` outputs are generated artifacts (the repo's
  `.gitignore` excludes them).
- The repo fixtures double as smoke tests: `cme run syntax.cm` should
  always end with `failures=0`; `cme run mega.cm` exercises 21
  megaprogram invocations across HTML, CSS, JS, TS, Python, JSON, YAML,
  TOML, SQL, and regex.
