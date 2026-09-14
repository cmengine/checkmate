# Implementing Interfaces Across Files

A mod satisfies a host interface with `impl` blocks, and those blocks may
be **spread across the whole mod tree**. The compiler unions every
`impl <target>` block into one implementation, then checks it.

## The pattern

```checkmate
// File: src/gamemode/rules.cm
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }
}
```

```checkmate
// File: src/gamemode/events.cm
impl engine.gamemode {
    void OnTick(GameState state, float deltaTime) {
        // per-tick logic here
    }
}
```

The two blocks together are *the* implementation of `engine.gamemode` for
this mod. Members are called by qualified path from script code:

```checkmate
GameState s = engine.gamemode.InitGame(config)
s = engine.gamemode.OnTick(s, 0.016)
```

And the host invokes them by target path:

```rust
// Rust host:
let state = context.invoke_member("engine.gamemode", "InitGame", &[config_value])?;
```

```c
/* C host: */
cm_future_t* f = cm_invoke(ctx, "engine.gamemode", "InitGame", args, 1);
```

## The union rules

- All `impl <same-target>` blocks across the mod tree merge into one
  member set.
- **Completeness**: with a [schema](../schema/overview.md) active, every
  required visible member of the interface must be implemented, with the
  **exact signature** (parameter types, names may differ, return type). A
  missing member fails compilation.
- **Exactness**: implementing the same member twice — in one block or two
  — is a duplicate-member error. There is no overriding.
- **Optional members** (marked `optional` in the schema) may be skipped
  without breaking the implementation.
- **Unknown or ungranted targets are errors**: an `impl` naming a
  namespace the mod cannot see fails the build. An impl is a claim, and
  claims get validated — silence would ship an interface the schema never
  checked.

## Why union instead of "one file per interface"

Because mods are *organized by your domain, not by the contract*. Game
rules, audio hooks, and UI callbacks for one interface can each live in
the file where they belong, and the compiler stitches them together. The
union check then guarantees the *whole* is complete — no matter how the
pieces are arranged.

## Local targets, same machinery

Unioning applies to plain local targets too — `impl counter` split across
files behaves identically (see
[`impl` Blocks](../language/impl-blocks.md#multiple-blocks-union)). The
dotted-path shape is simply the boundary-facing case.

## A complete walkthrough

The repository's host fixtures exercise this end to end:

- [`host_api_test/Rust/shop_mod/`](https://github.com/cmengine/checkmate/tree/mom/host_api_test/Rust/shop_mod)
  implements a schema interface across files, gated by its `mod.toml`
  `[schemas]` table.
- [`apps/rust_host`](https://github.com/cmengine/checkmate/tree/mom/apps/rust_host)
  consumes it with generated, compile-time-verified bindings — see
  [Schema-Driven Embedding](../embedding/schema-embedding.md).
