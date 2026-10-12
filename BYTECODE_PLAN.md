# Synchronous bytecode milestone

## Accepted scope

The owner approved the shared typed IR and synchronous register VM milestone,
and selected **bytecode instruction fuel metering** (option 1). Existing fuel
thresholds may change; document that compatibility change.

The owner also approved repairing `tests/fixtures_suite.rs`: pin each fixture's
intended result explicitly (`basic.cm` computes 3; self-checking fixtures return
0; `syntax.cm` returns a pinned report), and run megaprogram fixtures through expansion before checking/execution.

- Preserve checker-resolved expression types, resolved imports, call targets,
  lexical slots, control flow, and source spans in an AST-free core IR.
- Generate register bytecode in `cme-interp` and execute it with an explicit
  frame stack. Keep the tree walker as the independent reference oracle and
  compile-time megaprogram evaluator.
- Compile once when loading host programs. Use bytecode for CLI and Rust/C host
  invocations after differential coverage passes.
- Preserve synchronous language behavior, schema gates, provider snapshots,
  errors and module-local locations, deadline/depth limits, and reentrancy.
- Keep default facade features empty and the dependency graph acyclic. Add no
  dependencies: compiler -> interp -> core already exists.
- Update implemented-behavior docs and fuel accounting references together.

Async, continuation splitting, LLVM/AOT, artifact formats, ARC/COW, allocation
budgets, freestanding support, and new language syntax are subsequent work.

The owner approved repairing inherited mixed `int`/`byte` arithmetic: use
checked addition/subtraction/multiplication in both operand orders in the shared
value helper, with debug/release regression coverage for the walker and VM.

## Validation and completion

1. Typed lowering and bytecode generation: checked fixtures produce typed
   registers and resolved call targets; bytecode owns no executable AST.
2. Differential execution: runnable fixtures and focused edge cases agree with
   the tree walker on values and runtime failures (kind, message, span).
3. Integration: existing CLI, mod, schema, Rust/C host, concurrency, limits, and
   facade suites pass through bytecode. Verify zero/exact fuel, loop metering,
   explicit recursion frames, and fresh budgets on repeated invocations.
4. Add a reproducible benchmark comparing load and invocation costs.
5. Run formatting, workspace tests, and clippy, including complete CLI and
   schema-macro facade coverage. Update this brief with final validation.

## Milestone validation before follow-up

- `cargo fmt --check` and `git diff --check`: clean.
- `cargo test --workspace`: 1,580 passed, zero failures (before the additional
  approved mixed-arithmetic regression).
- `cargo test --workspace --features cli,schema-macro`: 1,608 passed,
  zero failures; four pre-existing ignored doctests. Includes the exhaustive
  prefix/recovery suites, real Rust/C consumers, schemas/mods, concurrency,
  generated bindings, and 17 new bytecode/host tests.
- `cargo clippy --workspace --all-targets --features cli,schema-macro`: clean,
  no warnings.
- Release-mode mixed-arithmetic regression: passed for both engines, both
  operand orders, overflow and adjacent non-overflow boundaries.
- Updated documentation links resolve. `mdbook` is not installed locally, so
  no local book-render result is claimed.
- No dependencies were added; the facade's default feature set is unchanged.

`cargo bench --bench bytecode` completed on the final code. This is one local,
unmetered release run, not a portable performance guarantee. Each invocation
column totals 1,000 calls; each load column totals 100 front-end loads.

| Workload | Walker invokes | VM invokes | AST loads | Bytecode loads |
| --- | --- | --- | --- | --- |
| Scalar loop | 1,675.12 ms | 1,053.80 ms | 1.76 ms | 3.52 ms |
| Recursive calls | 598.93 ms | 681.54 ms | 3.88 ms | 6.00 ms |
| Collections | 1,093.60 ms | 883.82 ms | 3.76 ms | 6.57 ms |

Loops and collections improved in this run; recursive calls were about 14%
slower. Bytecode generation adds load cost. Register reuse/frame allocation and
ARC/COW remain later optimization work; the current milestone makes no claim
that the VM outperforms the reference walker on every workload.

## Follow-up review and test expansion

The owner requested fixing all spotted problems and substantially expanding
tests. This authorizes remediation of concrete correctness findings.

- Confirmed: `Context::run` tracked only the innermost context ID, allowing
  A -> B -> A indirect reentry, including through a clone of A. Track every
  active context on the current thread; keep cross-context and cross-thread
  calls legal and release the guard on errors and unwinding. Add regression
  coverage and update embedding documentation.
- Expand differential and independently expected boundary/control-flow tests,
  bytecode verifier rejection coverage, and host invocation recovery tests.

### Follow-up results

- Added 103 further tests: 120 bytecode/host/verifier tests now cover control
  flow, snapshots, deep copies, generic/option/result payloads, calls, malformed
  IR references, recovery after failures and provider panics, and indirect
  reentrancy through cloned contexts. Numeric matrices include 1,907 boundary
  cases with independent checked-integer/IEEE floating-point expectations.
- `cargo test --workspace`: 1,684 passed, zero failures.
- `cargo test --workspace --features cli,schema-macro`: 1,711 passed, zero
  failures. Both complete runs retain the four pre-existing ignored doctests;
  no runnable tests were skipped.
- `cargo fmt --check`, `git diff --check`, and clippy over all workspace targets
  with both default and CLI/schema-macro features: clean, no lint warnings.
- Release-mode bytecode, verifier, and host API suites: 120 passed, zero
  failures.
- Embedding Rust/C/overview documentation explains indirect reentrancy. All
  checked relative links in updated documentation resolve.
