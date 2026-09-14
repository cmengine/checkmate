# Imports and Namespaces

`import` grants access to two kinds of namespaces: **host-provided
capabilities** (declared in a [schema](../schema/overview.md)) and **internal
modules** of the current [mod](../mods/mods.md).

## The two forms

```checkmate
import engine.graphics     // a host capability namespace
import engine.input        // another host namespace
import self.gamemode.rules // an internal module of the current mod
```

- The first segment is a **root**: `engine`, `physics`, `ui`, ... name
  schema namespaces the host registered; `self` names the current mod's own
  tree.
- Further segments drill into that root: `engine.graphics` targets the
  `graphics` capability of the `engine` namespace; `self.gamemode.rules`
  targets the module file `src/gamemode/rules.cm` of the mod.
- Imports appear before use; the checker treats them as preconditions for
  the names they unlock.

## What an import unlocks

**Host capability imports** unlock *calls into the host*:

```checkmate
import engine.graphics

void draw(vec2 pos) {
    engine.graphics.DrawTexture(texture, pos)
}
```

With a schema active, the import is **gated**: the namespace must be
granted to your program (directly, or via the mod manifest's `[schemas]`
table), capability calls type-check against the schema signatures, and if
the capability `requires` an interface, your mod must implement that
interface completely before the import (let alone the call) compiles — see
[Capabilities and Interfaces](../schema/contracts.md).

**Self imports** unlock *cross-module code within a mod*:

```checkmate
import self.gamemode.rules

str main() {
    return rules.describe(gameEvent.Damage(25))
}
```

Module paths map one-to-one onto the `src/` tree:
`src/gamemode/rules.cm` is `self.gamemode.rules`. Details and the
standalone-file rule are in [Imports and Module Paths](../mods/imports.md).

## Imports are explicit and scoped

- No wildcard imports, no star-globs, no transitive re-export. If module A
  imports `engine.graphics` and module B imports A, B does **not** thereby
  see `engine.graphics` — B writes its own import.
- Because modules hold no mutable global state, imports carry no
  initialization order, no side effects, and no cycles to untangle: an
  import makes *names* visible, nothing else.
- The [language server](../tooling/lsp.md) completes import segments
  (schema namespaces, capabilities, and `self.` module paths) so you rarely
  type one blind.

## Mod isolation

Mods **cannot import sibling mods**. There is no `import other_mod.*`. If
two mods must communicate, the host exposes an explicit bridge capability
(an event bus, a message queue) through the schema — keeping every mod a
self-contained unit the host can load, gate, and revoke independently. See
[Mod Isolation](../mods/mods.md#isolation).
