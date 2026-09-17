# Mod Layout

A **mod** is Checkmate's unit of distribution: a self-contained directory
holding a manifest and a `src/` tree, compiled as *one* program.

## Directory structure

```text
my_game_mod/
├── mod.toml
└── src/
    ├── main.cm
    ├── gamemode/
    │   ├── rules.cm
    │   └── events.cm
    └── ui/
        └── hud.cm
```

- `mod.toml` — the manifest (below).
- `src/` — the code. Every `**.cm` file under `src/` becomes a module.
- A mod is loaded by pointing the toolchain or host at the **directory**
  (or at the `mod.toml` path itself):

  ```sh
  cme check my_game_mod
  cme run my_game_mod
  cme run my_game_mod/mod.toml    # equivalent
  ```

## The manifest: `mod.toml`

```toml
name = "advanced_rules"
version = "1.0.0"
checkmate_version = "0.5.0"

[schemas]
engine = "1.4.0"
physics = "1.0.0"
```

| Field | Meaning |
| --- | --- |
| `name` | The mod's name |
| `version` | The mod's own version, `X.Y.Z` |
| `checkmate_version` | The language/toolchain version the mod targets, `X.Y.Z` |
| `[schemas]` | Optional table of schema namespace → target version |

- Versions must be `X.Y.Z`. Comments, bare or quoted keys, and
  single/double-quoted strings are accepted; typed values, multiline
  strings, and dotted keys are rejected with line-anchored diagnostics.
- The `[schemas]` table **narrows the grant**: only the listed namespaces
  are visible to the mod, and only at (or below) the declared target
  version. A namespace the host registered but the manifest does not list
  is invisible; members introduced `since` a later version are hidden. See
  [Versioning](../schema/versioning.md).
- When a host loads a mod with no `[schemas]` table and no registered
  schemas, the pre-schema behavior applies (host-rooted imports are
  accepted as host-style paths).

## The module tree

File paths under `src/` map directly to **module paths** rooted at `self`:

| File | Module path |
| --- | --- |
| `src/main.cm` | `self.main` |
| `src/gamemode/rules.cm` | `self.gamemode.rules` |
| `src/ui/hud.cm` | `self.ui.hud` |

- Directory and file names must be valid identifier segments; names that
  could never be imported (`weird-name.cm`, spaces) are rejected at load
  with a pointed diagnostic rather than silently becoming unimportable.
- Discovery is deterministic (sorted by module path), so builds are
  reproducible.

## Compilation model

The toolchain **links** the whole tree into one virtual program:

1. Each module's [megaprograms](../mega/overview.md) expand (per file).
2. The expanded sources concatenate into one program text with recorded
   per-module byte ranges.
3. Parse + type check run **once** over the whole thing.

This is what makes cross-file code feel like one file: top-level names
share one namespace (duplicates collide with an exact diagnostic), `impl`
blocks for one target union across the tree, and `main` — the CLI's entry
convention — may live in **any** module.

Diagnostics stay per file: a problem in `src/ui/hud.cm` is reported at
`my_game_mod/src/ui/hud.cm:12:5`, not in virtual-text coordinates.

## `main` and entry points

The language has no implicit entry point. The CLI's `run` follows the
convention of invoking the function named `main`, wherever it lives in the
mod. Embedded hosts invoke **whatever they choose**: any top-level
function, or any [`impl` member](../language/impl-blocks.md) by its dotted
target path — see [Embedding Overview](../embedding/overview.md).

## Isolation

Mods cannot import sibling mods — no `import other_mod.*` exists. If two
mods must communicate, the **host** exposes an explicit bridge capability
through the schema:

```checkmate
since 1.0.0 capability engine.modBridge {
    void EmitEvent(str eventName, EventPayload payload)
    void Subscribe(str eventName)
}
```

This keeps every mod independently loadable, gateable, and revocable, and
it keeps the dependency graph of a host application a star, not a web.
