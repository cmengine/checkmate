# Checkmate Compiler Hygiene, AST Spans, and CLI Diagnostics Plan

## Status and Progress

This file is the single source of truth for the current implementation plan. A fresh agent should read this section first and continue from the first unfinished item. Keep it continuously updated so it always reflects the exact current state of the work.

- [x] Set up the working branch/jj change and record the starting commit here.
- [x] Extract `cme-core`'s nested AST into `crates/cme-core/src/ast.rs` while preserving `cme_core::ast` and `cme_core::Span` paths.
- [x] Convert `Expr` and `Stmt` into span-carrying wrapper structs with `ExprKind` and `StmtKind`, adding `span: Span` to both wrappers and updating all compiler and CLI construction/matching sites.
- [ ] Implement fallible Logos callbacks for integer and float literals so out-of-range values produce `NumericLiteralOutOfRange { span }` instead of panicking; retain normal invalid-token behavior for unrecognized input and recovery-to-next-newline behavior.
- [x] Add real expression and statement span tracking during parsing and replace the placeholder `expr_span` helper so mixed logical-operator diagnostics have source-accurate spans.
- [ ] Move insignificant-newline preprocessing out of `Parser` into a dedicated compiler module/function, preserving statement-boundary newline behavior and bracket-scoped insignificant newlines.
- [ ] Make `Parser::parse_statement` crate-private and add the crate-level all-diagnostics facade `cme_compiler::parse_program_with_errors(source: &str) -> (Vec<Stmt>, Vec<Diagnostic>)` that lexes, preprocesses, parses, and aggregates lex/preprocess/parse diagnostics.
- [ ] Add optional workspace dependencies for the CLI: `clap` with derive support and `miette` with a source-snippet-capable feature, both gated behind the root `cli` feature.
- [ ] Move CLI implementation into `src/cli.rs`, expose `cme <lex|ast> <file.cm>` via `clap` subcommands, and keep `main.rs` minimal.
- [ ] Convert compiler diagnostics into `miette` diagnostics in the CLI layer, using `NamedSource` and proper source snippets; do not couple `cme-compiler` to `miette`.
- [ ] Remove placeholder `add` functions and tests from `cme-interp` and `cme-runtime`.
- [ ] Remove empty dependency tables and normalize inherited package metadata across workspace manifests.
- [ ] Add/update tests listed in the Test Plan and run the full validation suite.
- [ ] Update this progress checklist after every completed or materially changed task, including enough implementation context for handoff.

### Progress Notes

- The working copy started from the `feat: implement unary operations` state with no uncommitted changes.
- The working change was `wluovvpv` / `dea64e04 refactor: clean up codebase` before implementation began; do not use version-control commands directly without user direction.
- Completed AST extraction: `cme_core::ast` now lives in `crates/cme-core/src/ast.rs`; `Expr` and `Stmt` are wrapper structs with `span` and `kind`, with kinds in `ExprKind` and `StmtKind`.
- Completed parser span tracking for variable declarations, assignments, compound assignments, literals, identifiers, unary/binary/parenthesized expressions, and mixed logical-operator diagnostics. `Parser::parse_statement` is now `pub(crate)`.
- Lexer work is partially implemented: `IntLit` and `FloatLit` callbacks now return `Option<i64>` / `Option<f64>`, and `LexError::NumericLiteralOutOfRange { span }` exists. The current loop distinguishes numeric out-of-range errors from invalid-token errors and preserves recovery to the next newline.
- Existing lexer/parser unit tests passed after the AST conversion. Numeric-literal work was interrupted before the full compiler test suite was rerun after the final token-shape cleanup; several lexer tests still need their literal expectations updated to the `Option` payload shape.
- The incomplete numeric work has one known warning risk: the `Err(_)` branch may contain a stale `numeric_error` check from an earlier iteration and should be simplified while preserving `Invalid` behavior.
- Remaining major areas are unchanged: finish numeric tests, move newline preprocessing, add the public all-diagnostics facade, update CLI dependencies and implementation, remove placeholder functions/dependencies, add the Test Plan coverage, run full validation, and update this checklist.
- The user approved breaking public AST and parser APIs because this is an early-stage workspace.
- Numeric literals outside parseable `i64` or valid finite `f64` values are errors, not clamped values.
- `clap` and `miette` must only be introduced through the optional `cli` dependency stack; default builds must remain dependency-light.

## Summary

- Implement the approved review outcomes: safe numeric literal diagnostics, source spans in the AST, all-error parsing, compiler/CLI cleanup, and richer CLI diagnostics.
- Preserve Checkmate's current language behavior and feature-gated architecture; no parser or interpreter behavior changes beyond improved diagnostics.
- Use `clap` for CLI argument parsing and `miette` for source-aware diagnostics, both optional and enabled by the root `cli` feature.

## Public/API Changes

- `cme-core`: replace bare `Expr` and `Stmt` enums with span-carrying wrapper structs:
  - `Expr { span: Span, kind: ExprKind }`
  - `Stmt { span: Span, kind: StmtKind }`
  - Existing variant payloads move into `ExprKind` and `StmtKind`; public paths remain `cme_core::ast::*` and `cme_core::Span`.
- `cme-compiler`:
  - Remove the placeholder `expr_span` helper and construct real spans during parsing.
  - Replace `LexError::Invalid` for numeric failures with `NumericLiteralOutOfRange { span }`; regular unrecognized input remains invalid.
  - Keep `Parser::parse_statement` crate-private.
  - Move newline preprocessing from `Parser` into a dedicated compiler module/function.
  - Add `cme_compiler::parse_program_with_errors(source: &str) -> (Vec<Stmt>, Vec<Diagnostic>)`, which returns statements and all collected diagnostics.
- Root CLI: move implementation into `src/cli.rs`; expose the same `cme <lex|ast> <file.cm>` workflow through `clap` subcommands.

## Implementation Plan

- Lexer:
  - Change integer/float Logos callbacks to fallible callbacks so out-of-range literals become lexer errors instead of panics.
  - Preserve recovery-to-next-newline behavior and add focused range tests.
- Parser:
  - Track spans while constructing unary, binary, parenthesized, primary, assignment, compound assignment, and declaration nodes.
  - Use real expression/statement spans for “mixed && and || require parentheses” diagnostics.
  - Adjust all parser tests to the new wrapper AST and add tests for representative spans.
- Compiler organization:
  - Add a standalone insignificant-newline preprocessing API and remove that responsibility from `Parser`.
  - Implement the crate-level parser facade that returns statements and all collected diagnostics; avoid duplicated CLI/test setup.
- CLI:
  - Add optional workspace dependencies: `clap` with derive support and `miette` with a source-snippet-capable feature.
  - Gate both dependencies and `src/cli.rs` behind the `cli` feature.
  - Convert compiler diagnostics into `miette` diagnostics in the CLI layer; use `NamedSource` for rendered snippets.
  - Keep `main.rs` minimal and safe to build without the `cli` feature.
- Cleanup:
  - Move `cme_core`'s nested AST into `crates/cme-core/src/ast.rs`.
  - Remove placeholder `add` functions and tests from `cme-interp` and `cme-runtime`.
  - Remove empty dependency tables and normalize inherited package metadata across workspace manifests.

## Test Plan

- Run `cargo test --workspace --features cli`.
- Add lexer tests for large `i64` and `f64` literals, ensuring diagnostic rendering contains no panics.
- Add parser span tests for literals, identifiers, unary/binary expressions, parenthesized expressions, and each statement form.
- Add mixed-logical-operator tests asserting non-zero, source-accurate spans.
- Add all-diagnostics tests where lex, preprocessing, and parse errors occur together.
- Add CLI tests or integration assertions for `lex` and `ast`, including a diagnostic-rendering smoke test.
- Verify `cargo build` with default features remains dependency-light, while `--features cli` enables the CLI stack.
- Run `cargo fmt` and `cargo clippy --workspace --all-targets`.

## Assumptions

- This is an early-stage workspace, so breaking public AST and parser APIs is acceptable.
- Numeric literals outside `i64` or valid finite `f64` parsing are errors, not clamped values.
- The root `cli` feature remains the only feature that introduces `clap` or `miette`.
- Empty interpreter/runtime crates are acceptable until real APIs are designed.
- Newline handling is intentionally significant at statement boundaries and must be preserved.
