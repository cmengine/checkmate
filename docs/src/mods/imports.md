# Imports and Module Paths

Within a mod, code is split across files and stitched back together with
`import self.*` statements. This page covers the path algebra and the
rules that make it exact.

## `self` and module paths

`self` is the reserved root of the current mod's `src/` tree. A module
path is the file path with `src/` dropped and `.cm` dropped:

| File | Path |
| --- | --- |
| `src/main.cm` | `self.main` |
| `src/gamemode/rules.cm` | `self.gamemode.rules` |
| `src/ui/hud.cm` | `self.ui.hud` |

Importing a module makes its top-level names visible at the import site:

```checkmate
// file: src/main.cm
import self.gamemode.rules
import self.ui.hud

str main() {
    hud.render(rules.title())
    return "ok"
}
```

## Resolution is table-exact

Module paths resolve against the mod's **module table** — the discovered
`src/**.cm` tree — not against the filesystem at import time:

- `import self.a.b` with no `src/a/b.cm` fails with a diagnostic that
  *names the expected file*, at the import's span.
- Names are exact: `self.gamemode.Rules` is not `self.gamemode.rules`.

## Standalone files reject `self`

A single-file build (a `.cm` file that is not part of a mod) **rejects
`self.*` imports outright** — there is no tree to resolve against:

```sh
cme check standalone.cm        # a self.* import here is an error
cme check my_mod/              # ...here it resolves against the mod table
```

If you want the multi-file workflow, make it a mod (a `mod.toml` and a
`src/` directory are all it takes).

## Cross-module linking

Because the whole tree parses and checks as **one program**, cross-module
code behaves exactly like same-file code:

- Top-level names across all modules share one namespace. Two modules both
  declaring `struct player` collide with a duplicate-name diagnostic.
- [`impl` blocks](../language/impl-blocks.md) for one target **union** across
  files — see [Implementing Interfaces Across Files](../mods/impl-union.md).
- Forward references work: module A may call functions declared in module
  B regardless of file order.
- There is no static initialization, no load order, no import cycles to
  worry about — imports make names visible; nothing executes at import
  time.

## What does *not* cross files

- **Megaprogram macro names**: a `mega` declared in module A is not
  invocable as `mega(A.name) { ... }` in module B in this version —
  cross-file macro imports are out of scope with the text-level expander.
  A mega invocation must be imported via its own module's machinery;
  see [Megaprogramming](../mega/overview.md) for the current scope.
- **Schema grants**: the grant comes from the manifest's `[schemas]`
  table and the host's registration — not from imports (see
  [Versioning](../schema/versioning.md)).

## Editor experience

The [language server](../tooling/lsp.md) auto-detects mods: from any open
file it walks up to the nearest `mod.toml`, resolves `self.` imports
against the real tree, links cross-module calls, and re-anchors
diagnostics to the owning module — with completion for `self.` module
segments at import sites.
