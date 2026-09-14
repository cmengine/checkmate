# The Language Server

Checkmate ships its language server in the same binary as the CLI:

```sh
cargo run --features cli -- lsp
```

`cme lsp` speaks LSP over **stdio**. The analysis is incremental
([salsa](https://github.com/salsa-rs/salsa)): every document is a query
input, and an edit recomputes only what it invalidated. The
[Zed extension](editors.md#zed) launches it automatically.

## Core features

- **Diagnostics stream on open and edit** — lexer, parser, validator,
  type checker, §8 megaprogram expansion, and §9 schema pipelines all
  report into the editor.
- **Hover** shows resolved signatures and crystallized `infer` types.
- **Completion** covers struct fields and enum variants after `.`,
  named arguments inside calls, import segments, and scope names.
- **Go-to-definition, find-references, document symbols, semantic
  tokens** are shadowing-aware.

## Mods and schemas: auto-detected, zero configuration

The server understands where your files live:

- From any opened file it **walks up to the nearest `mod.toml`** (§10.1),
  collects the `.cm` schema files around the mod root — including a
  `schemas/` directory next to the mod, the layout this repository's
  host fixtures use — and grants them through the manifest's
  `[schemas]` table exactly like `cme check <mod> --schema` does
  (§9.5, §10.2). No `[schemas]` table ⇒ every discovered namespace, for
  loose host-style projects.
- Scripts in a mod are checked **as one assembled program**
  (§10.3/§10.4): cross-module calls link, `import self.a.b` resolves
  against the real module tree, `impl` blocks union across files, and
  every diagnostic is **re-anchored to the module it came from**.
- Results cache by mtimes plus open-buffer texts, so an **unsaved**
  schema buffer drives the contract scripts see — even before saving.

## Schema-aware authoring

- **Boundary types** (`Sprite`, `Event`) complete and hover like local
  types — kept out of document symbols and go-to-definition, because
  they live in the schema file.
- `game.window.` completes the capability's members with their
  signatures; capability calls fill **named arguments** from the
  schema; import completion offers schema namespaces and capabilities.
- Inside `impl game.gamemode { ... }` the interface's missing members
  complete with their **exact signature** (§9.1/§10.4).

## Schema files are first-class documents

Inside a `.cm` schema file the §9 keywords, member shapes, and types
complete (`optional` only in interfaces, `void` only in return
position), every declaration hovers with its signature and
`since`/`optional` metadata, and the document outline mirrors the file.
A schema file with parse diagnostics falls back to its **last clean
parse** — a mid-edit broken schema never darkens every script; its own
buffer reports the defect.

## Megaprogramming completion

- `mega` and `grammar` complete anywhere.
- Inside a `mega name(pattern) { template }` declaration: the pattern
  position offers the §8.3 fragments, combinators, and editor
  annotations; the template position the §8.4 constructs; grammar bodies
  the §8.2 profile declarations, switching to the pattern language
  inside `rule` bodies.
- **Invocation regions** (`name! { ... }` / `mega(name) { ... }`) and
  heredocs stay **silent** — they are foreign text.
- Files that mention megaprograms still surface expansion diagnostics
  (anchored in the original text), plus the **`cme/expand` custom
  request** returning the expansion preview.

## Editor wiring

Any LSP client works with the standard stdio transport. The in-tree
[Zed extension](editors.md) is configured out of the box; for other
editors, point the client at the `cme` binary with the `lsp`
subcommand.
