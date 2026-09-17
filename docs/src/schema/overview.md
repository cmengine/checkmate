# The Schema System — Overview

The **schema system** is Checkmate's host contract: the single source of
truth for what scripts may call, what they must implement, and which
types cross the boundary.

## The problem it solves

Embedding a scripting language usually means hand-maintaining a binding
layer: a C function here, a Rust trait there, a doc page somewhere, and
three places that drift apart. Checkmate declares the contract **once** in
a `.cm` schema file, and everything else derives from it:

```text
                    ┌──────────────────┐
                    │   schema file    │
                    │  engine 1.4.0    │
                    └────────┬─────────┘
        ┌────────────────────┼────────────────────┐
        ▼                    ▼                    ▼
 script-side          host Rust            host C
 checking: imports,   bindings:            header: vtables,
 capability calls,    capability traits,   registration macros,
 impl completeness,   typed proxies,       pack/unpack helpers,
 version gating       boundary types       _Static_asserts
```

## The four declaration kinds

A schema file (one **namespace root** per file) declares:

- **`struct` / `enum`** — *boundary types*: shared data-interchange
  layouts crossing the FFI boundary (§9.3).
- **`capability`** — functions the **script calls** and the **host
  provides** (`engine.graphics.LoadTexture(...)`).
- **`interface`** — functions the **script implements** (with
  [`impl` blocks](../mods/impl-union.md)) and the **host calls into**
  (`engine.gamemode.OnTick`).

```checkmate
// File: schemas/engine.cm
schema engine 1.4.0

struct TextureHandle {
    int id
}

since 1.0.0 capability graphics {
    TextureHandle LoadTexture(str path)
    void DrawTexture(TextureHandle tex, vec2 position)
}

since 1.0.0 interface gamemode {
    GameState InitGame(GameConfig config)
    void OnTick(GameState state, float deltaTime)
}
```

## What registering a schema does

Register the schema with the engine (or the CLI's `--schema` flag) and
every subsequent load enforces the contract **at compile time**:

1. **Capability calls type-check** against schema signatures and require
   the import — `import engine.graphics` before
   `engine.graphics.LoadTexture(...)`.
2. **`requires` edges gate both directions** — importing or calling a
   capability that requires an interface demands a complete
   implementation of that interface (§9.4).
3. **`impl` completeness** — `impl engine.gamemode` must implement every
   required visible member with the exact signature; optional members may
   be skipped.
4. **Version hiding** — members introduced after the program's target
   version are invisible (§9.5).
5. **Boundary capitalization** — PascalCase names belong to the contract;
   script-internal names stay camelCase (§2.5).
6. **Load-time provider presence** — a program calling a capability with
   no registered provider fails the *load*, so wiring mistakes are
   deterministic before any invocation.

A program that produced any diagnostic never becomes invocable — the same
gate the CLI applies.

## Reading the chapters

- [Schema Files](../schema/schema-files.md) — file anatomy, types, naming.
- [Capabilities and Interfaces](../schema/contracts.md) — the two contract
  directions and their gates.
- [`requires` and Versioning](../schema/versioning.md) — dependency edges,
  `since` tags, `optional`, target versions.

## The generated bindings

- **Rust hosts** get compile-time-verified bindings through a procedural
  macro that runs the real schema parser at host build time:
  capability traits, typed interface proxies, and native structs/enums
  for boundary types. See [Schema-Driven Embedding](../embedding/schema-embedding.md).
- **C hosts** get a generated header (`cme codegen-c`): per-member
  function-pointer typedefs, a vtable, a registration macro whose
  `_Static_assert`s verify every host implementation's signature at
  compile time, and typed pack/unpack helpers over the `cm_value` ABI.

## Where to see it working

- [`apps/rust_host`](https://github.com/cmengine/checkmate/tree/mom/apps/rust_host)
  — the full §9.6 flow over its own `schemas/game.cm` (`--schema-demo`).
- [`apps/c_host`](https://github.com/cmengine/checkmate/tree/mom/apps/c_host)
  — the C flow including the generated header consumed for real.
- [`host_api_test/Rust`](https://github.com/cmengine/checkmate/tree/mom/host_api_test/Rust)
  — a schema-gated mod with cross-namespace `requires`.
