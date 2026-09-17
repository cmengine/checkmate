# Embedding Overview

Checkmate exists to be embedded. This chapter gives the architecture and
the concepts every host — Rust or C — shares: the object model, the
invocation pipeline, execution limits, values, errors, capability
dispatch, and the threading rules.

## The object model

Three objects, one direction of dependencies:

```text
Engine  ──loads──▶  CompiledProgram  ──creates──▶  Context  ──invokes──▶  Value / error
```

| Object | Role | Thread rules |
| --- | --- | --- |
| **Engine** | Holds the registered [schemas](../schema/overview.md) and capability providers; compiles sources, files, and mod trees | Immutable once configured; share freely |
| **CompiledProgram** | A checked, immutable program: entry points, interface targets, manifest, modules | Immutable; share freely |
| **Context** | An execution handle over one program, carrying the host's [limits](../appendix/limits-and-errors.md) | `Send + Sync` in Rust; every invocation is independent; do not re-enter a context from one of its own capability calls ([§5.7](../appendix/limits-and-errors.md#reentrancy)) |
| **Value** | A script value crossing the boundary | Plain data; clone freely in Rust; in C, ownership rules apply |

## The load pipeline

Loading applies the **same gate the CLI applies** — a program that
produced *any* diagnostic never becomes invocable:

1. **Megaprogram expansion** (§8) — runs first, only when the source
   mentions the subsystem; per file inside mods.
2. **Parse** — lexer + parser over the expanded text.
3. **Standalone-import check** — `import self.*` is rejected outside a
   mod.
4. **Type check** — against the **active schema** when one is registered:
   imports, capability calls, `impl` completeness, version gating.
5. **Provider-presence check** — a program that calls a capability with
   no registered provider fails the *load*.

Mod loads (`load_mod`) additionally link the whole `src/` tree into one
virtual program and **re-anchor every compile diagnostic to its owning
module** — hosts never see virtual-text coordinates, only
`my_mod/src/ui/hud.cm:12:5`.

## What a host can invoke

There is no implicit entry point (§2.1): the host targets functions
explicitly.

- **Top-level functions** — by name, with positional arguments:
  `invoke("fib", &[Value::Int(10)])`.
- **`impl` members** — by target path + member name, the
  "interface functions" a mod implements:
  `invoke_member("engine.gamemode", "OnTick", &args)`.

Entry-point metadata is discoverable on the program handle
(`entry_points()`, `interface_targets()` in Rust;
`cm_program_entry_count/name`, `cm_program_interface_count/name` in C).

## Execution limits (§5.5)

Every invocation runs under the context's limits:

- **Fuel** — a deterministic operation count; exhausted ⇒ `Budget` error.
- **Deadline** — wall-clock budget in milliseconds, checked at
  safepoints ⇒ `Deadline` error.
- **Call depth** — maximum nested calls (default 1024) ⇒ `CallDepth`
  error.

Each invocation gets a **fresh** fuel cell and deadline, so one context
can serve many independent invocations — including concurrent ones from
several threads. Zero/unset means "unlimited" (depth falls back to the
engine default). Full semantics: [Execution Limits and Errors](../appendix/limits-and-errors.md).

## Values across the boundary

Script values map to a tagged value type on the host side:

| Script | Rust (`cme::Value`) | C (`cm_value_t` kind) |
| --- | --- | --- |
| `void` | `Value::Void` | `CM_VALUE_VOID` |
| `int` | `Value::Int(i64)` | `CM_VALUE_INT` |
| `byte` | `Value::Byte(u8)` | `CM_VALUE_BYTE` |
| `float` | `Value::Float(f64)` | `CM_VALUE_FLOAT` |
| `bool` | `Value::Bool(bool)` | `CM_VALUE_BOOL` |
| `str` | `Value::Str(String)` | `CM_VALUE_STR` |
| struct | `Value::Struct { name, fields }` | `CM_VALUE_STRUCT` |
| enum | `Value::Enum { name, variant, payloads }` | `CM_VALUE_ENUM` |
| array | `Value::Array(Vec<Value>)` | `CM_VALUE_ARRAY` |
| map | `Value::Map(Vec<(Value, Value)>)` | `CM_VALUE_MAP` |

In Rust, `From` impls build scalars from plain data and `as_int()`-style
accessors unpack results. Arguments are **cloned into** the script (§2.13
value semantics: the host's values are never aliased); results come back
as fresh values. In C, constructors build values, builders move children
in, accessors borrow children, and strings handed out are owned copies
freed with `cm_string_free` — see [Embedding in C](../embedding/c.md).

## Opaque host handles

Host-owned resources (textures, entities, sockets) cross into scripts as
**opaque values** — PascalCase struct/enum types declared in the schema,
carrying identifiers rather than raw pointers. Scripts can pass them
around, compare them, store them — and can *never* dereference them.
Invalidation and lifetime rules are entirely the host API's to define.
See [Value Semantics → Where the boundary lives](../language/value-semantics.md#where-the-boundary-lives).

## Capability dispatch

A script call like `engine.graphics.LoadTexture("hero.png")` executes:

```text
script call ──▶ checker-verified signature (compile time)
            ──▶ context's provider snapshot (load time: presence verified)
            ──▶ CapabilityProvider.call("LoadTexture", args)  (Rust)
                cm_capability_fn(user, args, argc, &err)      (C)
            ──▶ returned Value becomes the call's result
```

- Providers are registered per **capability path** (`engine.graphics`).
- The provider sees borrowed, positional `Value` args in schema
  declaration order and returns the member's declared return type (or
  `Void`).
- A provider error fails the invocation with a span anchored at the
  script's call site.

## Reentrancy (§5.7)

A host capability dispatched **from** a running invocation may not invoke
back into the **same context** before the original call returns. The
nested call fails immediately (`ErrorKind::Reentrant` in Rust;
`CM_ERROR_INVALID_ARG` with a §5.7-naming message in C) and the original
invocation continues untouched. Different contexts — and other threads —
stay free. This rule exists so capability implementations cannot build
re-entrancy cycles through the interpreter.

## Errors

Every failure the host sees is classified:

| Kind | Meaning |
| --- | --- |
| `Runtime` | Script failure: overflow, division by zero, out-of-bounds, missing key |
| `Budget` | §5.5 fuel exhausted |
| `Deadline` | §5.5 wall-clock deadline passed at a safepoint |
| `CallDepth` | §5.5 call-depth limit hit |
| `UnknownEntry` | The invoked function / impl member does not exist |
| `Reentrant` | A capability re-entered its own executing context (§5.7) |

Compile failures are a separate surface (rendered, positioned
diagnostics). Runtime errors carry message, line, column, file (mod
builds), and span. Details: [Execution Limits and Errors](../appendix/limits-and-errors.md).

## Design guarantees worth knowing

- **A checked program is the only executable unit.** No host code path
  exists that runs a diagnostic-carrying program.
- **Invocations are isolated.** Each gets fresh limits; concurrent
  invocations race with nothing (no shared mutable script state).
- **Determinism.** Same program, same inputs, same limits ⇒ same
  behavior, including fuel counts — the property future artifact caching
  will build on.

Now pick your language: [Embedding in Rust](../embedding/rust.md) or
[Embedding in C](../embedding/c.md), and for contract-driven hosting,
[Schema-Driven Embedding](../embedding/schema-embedding.md).
