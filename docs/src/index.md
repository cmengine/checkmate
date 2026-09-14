# Introduction

**Checkmate** (CME) is a statically typed, embeddable scripting language
implemented in Rust. It is designed for host applications — game engines,
editors, agents, simulations — that want scripts to be safe, predictable,
and completely under the host's control.

The design in one sentence: *scripts are pure, sandboxed, value-semantic
programs that can only reach the outside world through capabilities the
host explicitly grants.*

```checkmate
struct vec2 {
    float x
    float y
}

float distance(vec2 a, vec2 b) {
    float dx = b.x - a.x
    float dy = b.y - a.y
    return dx * dx + dy * dy
}
```

If you have used Go, C#, Rust, Python, or TypeScript, the surface will feel
familiar within minutes: C-family control flow, braces and significant
newlines, typed function signatures, algebraic data types, and `match`
exhaustiveness. What is unusual is what Checkmate *leaves out* — and what
that buys you.

## What makes Checkmate different

- **No ambient authority.** A script cannot touch the filesystem, network,
  clock, or memory unless the host grants a capability through a versioned
  [schema](schema/overview.md). The sandbox is enforced by the type checker,
  not by convention.
- **Value semantics everywhere.** Assignment and parameter passing behave
  as if values were copied. Mutating a struct in a function never affects
  the caller; you reassign the returned copy. There is no aliasing to reason
  about, ever.
- **No mutable global state.** Modules declare types and functions — that is
  all. Persistent state lives in the host and is reached through explicit
  handles. Independent script invocations are therefore race-free by
  construction.
- **Deterministic execution.** The host can cap every invocation with
  [fuel](appendix/limits-and-errors.md) (a deterministic operation count), a
  wall-clock deadline, and a call-depth limit. Infinite loops and recursive
  blowups terminate cleanly, never a hang.
- **Megaprogramming.** A declarative macro system lets any formal language —
  HTML, CSS, JSON, YAML, TOML, regexes — live inside Checkmate source with
  character-precise diagnostics. The shipped `std.json` and friends are
  ordinary user-space megaprograms, not compiler hooks.
- **One contract, many bindings.** The host/script interface is declared once
  in `.cm` [schema files](schema/overview.md). The same file gates the script
  compiler, generates compile-time-verified Rust traits and proxies, and
  emits a C header whose `_Static_assert`s check every host implementation.

## What Checkmate is not

- Not a standalone application language. There is no `main` in the language
  itself — the *host* picks the entry point it invokes.
- Not a package ecosystem. No package manager, no dependency resolver; mods
  are self-contained trees and cannot import sibling mods.
- Not concurrent by itself. No thread pool, event loop, or scheduler in the
  language; the host owns all concurrency.

## Where to go next

- New to the language? Start with [Installation](getting-started/installation.md)
  and [Hello, Checkmate](getting-started/hello-world.md), then take the
  [ten-minute tour](getting-started/tour.md).
- Arriving from another language? Jump straight to
  [Coming from Go](coming-from/go.md), [C#](coming-from/csharp.md),
  [Rust](coming-from/rust.md), [Python](coming-from/python.md), or
  [TypeScript](coming-from/typescript.md).
- Embedding Checkmate in your application? Read the
  [embedding overview](embedding/overview.md), then the detailed
  [Rust](embedding/rust.md) or [C](embedding/c.md) guide.
- Want the normative specification? The language specification lives in
  [`WHITEPAPER.md`](https://github.com/cmengine/checkmate/blob/mom/WHITEPAPER.md)
  at the repository root. These docs are the user-facing distillation of it;
  where the whitepaper describes future work, these pages tell you what runs
  *today*.

## About this book

This book documents the language, the toolchain, and the host APIs as they
are implemented today: a full front end (lexer, parser, type checker), a
tree-walking interpreter, megaprogramming, multi-file mods, the schema
system, and complete Rust and C host APIs. See
[Status of This Documentation](status.md) for the precise boundary between
"implemented" and "specified, not yet built".
