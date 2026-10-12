# Execution Limits and Errors

How the host bounds script execution (§5.5), what happens when a bound
trips, and the full error taxonomy a host sees.

## The three limits

Every invocation runs under the **context's** limits — a fresh fuel
cell and deadline per call:

| Limit | Rust field | C field | Semantics |
| --- | --- | --- | --- |
| **Fuel** | `fuel: Option<u64>` | `uint64_t fuel` | A deterministic **bytecode instruction count** — every executed instruction, including calls, branches, jumps, and returns, charges one unit. Identical bytecode and inputs consume identical fuel regardless of machine speed. |
| **Deadline** | `deadline_ms: Option<u64>` | `uint64_t deadline_ms` | Wall-clock budget in milliseconds, evaluated **at safepoints only** — real time is never read asynchronously. |
| **Call depth** | `max_call_depth: usize` | `size_t max_call_depth` | Maximum nested script call frames. The VM uses an explicit frame stack, so script recursion does not recurse on the native Rust stack. |

Unset conventions:

| Value | Fuel | Deadline | Depth |
| --- | --- | --- | --- |
| `None` / `0` (Rust) | unmetered | none | engine default |
| `0` (C) | unmetered | none | engine default |
| `Some(0)` (Rust) | exhausts at first safepoint | expires immediately | — |

The engine default depth is **1024** (`MAX_CALL_DEPTH`). A nonzero value
below the default lowers the bound; values at or above the default keep it.
The VM checks the bound before pushing a script frame and reports `CallDepth`.

Fuel thresholds changed with the bytecode backend: the previous tree walker
charged statements and expressions. Recalibrate existing host budgets; the
same numeric budget need not stop at the same source location. There is no
legacy accounting mode. Compile-time megaprogram evaluation still uses the
reference walker's deterministic operation meter. Instruction counts may also
change with future compiler versions or optimizations.

## Safepoints

The synchronous VM checks the deadline before every bytecode instruction,
then charges fuel. Even an empty loop executes metered branches and jumps.
A host capability call runs synchronously and cannot be interrupted by the
VM; the deadline is checked again before the next instruction after it returns.

> **Future:** Native AOT execution will inject cooperative safepoints at
> function entries, loop backedges, and continuation splits (§5.5).

## What a tripped limit does

The invocation **terminates cleanly**: the host receives a classified
error — never a hang, never an asynchronous thread kill, never a
panic. Cancelling a *suspended* invocation (future async backend) will drop the
pending continuation deterministically, decrementing refcounts on
captured values (§5.6).

## The error taxonomy

### Runtime (script-side) failures

| Failure | Trigger |
| --- | --- |
| Integer overflow | Arithmetic outside the `int` or `byte` range; mixed `int`/`byte` operations widen to `int` and remain checked |
| Division / remainder by zero | `a / 0`, `a % 0` on `int` |
| Array index out of bounds | `xs[i]`, `i < 0` or `i >= xs.length` |
| Missing map key | Reading `m[k]` for an absent `k` |
| Defensive shape violations | A checker-invariant violation found at runtime (a bug, not your code) |

### Limit-family failures

| Kind | Rust `ErrorKind` | C `cm_error_kind_t` |
| --- | --- | --- |
| Fuel exhausted | `ErrorKind::Budget` | `CM_ERROR_LIMIT` |
| Deadline passed | `ErrorKind::Deadline` | `CM_ERROR_LIMIT` |
| Call depth exceeded | `ErrorKind::CallDepth` | `CM_ERROR_LIMIT` |

The C ABI groups all three under `CM_ERROR_LIMIT`; the message names
which.

### Host-side failures

| Kind | Meaning |
| --- | --- |
| `UnknownEntry` / `CM_ERROR_UNKNOWN_ENTRY` | The invoked function or impl member does not exist (§2.1: the host's target was wrong) |
| `Reentrant` / `CM_ERROR_INVALID_ARG` | A capability invoked back into its own executing context — the §5.7 prohibition |
| `CM_ERROR_COMPILE` | The load gate rejected the program (parse/type/schema diagnostics) |
| `CM_ERROR_IO` | A file or mod tree could not be read |
| `CM_ERROR_INVALID_ARG` | A host argument was unusable (NULL, not UTF-8, bad capability path) |

## Reentrancy (§5.7)

A host capability called **by** a running invocation may not invoke
script functions on the **same context** before the original call
returns:

```text
script ──calls──▶ capability ──tries cm_invoke(same ctx)──▶ REJECTED
                                  (ErrorKind::Reentrant / CM_ERROR_INVALID_ARG)
original invocation: unaffected
```

- The rejection happens **before any script code runs** in the nested
  call; the original invocation survives.
- The guard is keyed on a shared context identity — a **clone** of a
  context is the same logical context and is guarded identically.
- Different contexts, and invocations from other threads, are always
  free.

## Positional information

Every runtime/limit error carries, where applicable:

- `line` / `column` — 1-based, **character** column;
- `file` — the owning module's display path for mod builds
  (`my_mod/src/ui/hud.cm`); unset for loose sources;
- `span` — the failure's source span (Rust hosts).

Entry-miss errors carry no position. Hosts never see virtual-text
coordinates: mod diagnostics are re-anchored to the owning module at
both compile and run time.

## Choosing limits in practice

| Scenario | Suggested shape |
| --- | --- |
| UI/gameplay tick scripts | tight: `fuel: Some(100_000)`, `deadline_ms: Some(4)`, depth 64 |
| Build-time / offline generation | generous fuel, no deadline, default depth |
| Untrusted third-party mods | fuel + deadline + depth **all set**; pair with OS-level isolation for adversarial threat models |
| Deterministic tests | fuel only — reproducible failure points, no wall-clock noise |

Language-level sandboxing prevents unauthorized *API access* by
construction; for **adversarial** code, pair it with OS-level process
isolation (the honest-isolation guarantee of §7).
