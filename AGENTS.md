# AGENTS.md

## Repository Overview

Checkmate (CME) is a statically typed, embeddable scripting language implemented as a Cargo workspace in Rust (edition 2024). The implementation is early-stage: `cme-core` and `cme-compiler` contain the first real language functionality, while the interpreter and runtime remain placeholders.

The workspace root package is `cme`. It re-exports workspace crates behind feature flags. The intended development workflow uses `--features cli` (or the individual feature flags); the default build intentionally exposes no root APIs.

## Crates

- `crates/cme-core` — Foundation crate with no dependencies. Defines the spanned AST, source spans, and diagnostic references: primitive/inferred/void types, literals, identifiers, variable declarations, functions, calls, if/else, while, and return. Shared by compiler and future language components.
- `crates/cme-compiler` — Depends on `cme-core` and `logos`. Provides the lexer, parser, diagnostics, validator, and the one-call `parse_source` front-end for the current subset, including functions, control flow, bare calls, newline-sensitive statement parsing, and insignificant-newline handling inside brackets.
- `crates/cme-interp` — Dependency-free placeholder. Currently contains starter code only; interpreter behavior is not implemented.
- `crates/cme-runtime` — Dependency-free placeholder. Currently contains starter code only; runtime services and built-ins are not implemented.
- Root package `cme` — Facade package and optional CLI. Feature-gated re-exports are: `core`, `compiler`, `interp`, and `runtime`; `cli` enables all four and provides the working lex/ast toolchain binary.

## Whitepaper Policy

`WHITEPAPER.md` at the repository root is the language specification.

- **Read `WHITEPAPER.md` only through `mdpeek`.** This applies to every reading mechanism: shell tools, scripts, editors, search, source inspection tools, file listing with content output, and any tool or agent that can access file contents. Never use `cat`, `read`, `sed`, CodeGraph, or any other method to read it or any Markdown file.
- Treat the whitepaper as a human-owned language source of truth. Run `mdpeek toc FILENAME.md` to get the ToC of the file with numbers attached to them, then run `mdpeek fetch FILENAME.md [comma-seperated numbers]` to get relative bodies. Try fetching only the parts you need.
- When a task requires clarification about Checkmate language design, syntax, semantics, precedence, intended compiler behavior, or any other specification detail, **ask the user and stop** until the user answers.
- Do not infer language behavior from the whitepaper's file metadata, size, or any partial content exposure.

## Working Rules

- Always use `git` and never use `jj` commands directly.
- Keep changes focused on the affected crate and avoid implementing unspecified language behavior by guessing.
- Preserve the feature-gated structure of the root facade. Adding functionality to a workspace crate should not require enabling that crate by default in the root package.
- Lexer and parser changes must respect `cme-core` AST ownership: language data models belong in `cme-core`, while recognition/parsing remains in `cme-compiler`.
- Update lexer, parser, validator, and AST pieces together when language-facing changes require it, and add focused unit tests in the relevant crate.

## Code Review Process

- Perform code review in report-first mode: inspect the affected source and produce findings before changing code.
- For each finding, present remediation options with a recommended option first and an explicit discard option last; do not implement remediation until the user chooses.
- Keep review findings tied to concrete source locations and distinguish correctness issues from style-level changes.
- After review decisions are approved, record the accepted work in the relevant task brief or commit plan, then implement against it.
- Run `cargo fmt --check` (or `cargo fmt` when preparing changes), `cargo test --workspace`, and `cargo clippy --workspace --all-targets` as part of validation. Use the complete facade with `--features cli` when CLI-facing behavior changes.

## Commands

- Build the toolchain: `cargo build --features cli`
- Run tests: `cargo test --workspace`
- Run tests with the complete facade enabled: `cargo test --workspace --features cli`
- Format: `cargo fmt`
- Lint: `cargo clippy --workspace --all-targets`

## mdpeek

- `mdpeek toc [PATH]`: list numbered headings. Omitted `PATH` uses `WHITEPAPER.md` or the sole `.md` file.
- `mdpeek fetch PATH SECTIONS`: print selected sections, comma-separated numbers such as `1,4,23`.
- Fetch includes subsections, sorts requested sections, and exits `2` if any number is invalid.
- Always run `toc` first and use its numbers; fenced code headings are ignored.

## Repository State Notes

- The root binary reports how to build with the `cli` feature when that feature is absent.
- Newline handling is intentionally significant at statement boundaries; do not remove this behavior without explicit user direction.

<!-- CODEGRAPH_START -->

## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->
