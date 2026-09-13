# Checkmate for Zed

A [Zed](https://zed.dev) extension providing language support for
[Checkmate (CME)](../../WHITEPAPER.md) — scripts (`.cm`), multi-file mods, and
schema files.

## Features

- **Tree-sitter parsing** via the grammar in
  [`grammars/tree-sitter-checkmate`](../../grammars/tree-sitter-checkmate) —
  the full implemented surface including §8 megaprogramming (`grammar`,
  `magic`, patterns, templates, regions, heredocs) and §9 schema files.
- **Syntax highlighting** mapped onto standard Zed captures (`@keyword`,
  `@type`, `@function`, `@string.special`, `@embedded`, ...), so every theme
  works out of the box.
- **Language server** (`cme lsp`, WHITEPAPER §14) with
  - real-time diagnostics (parse, type check, §8 megaprogram expansion, §9
    schema validation),
  - hover documentation (locals, functions, structs, enums, variants,
    fields, imports, built-in constructors),
  - completion: struct fields and enum variants after `.` (including array
    `.length`), named arguments in calls and constructions, import path
    segments, and scope/keyword suggestions,
  - go-to-definition and find-references (shadowing-aware),
  - document symbols (functions, structs, enums, impl blocks with members),
  - semantic tokens that complement the tree-sitter highlights, and
  - a `cme/expand` custom request returning the megaprogram expansion
    preview.
- **Bracket matching** with rainbow-bracket colorization (quotes excluded).
- **Code outline** for functions, structs, enums, impl targets, imports,
  grammar rules, magic declarations, and schema members.
- **Auto-indentation** driven by the syntax tree plus newline-aware fallback
  patterns (statements are newline-delimited).
- **Vim text objects** (`af`/`if` around/inside functions, `ac`/`ic` for
  structs/enums/impl blocks, `gc` for comments).

## Layout

```
editors/zed/
├── extension.toml                  # extension manifest + grammar/language-server registration
├── Cargo.toml                      # the extension crate (wasm) — language server launch
├── src/lib.rs                      # resolves the `cme` binary, starts `cme lsp`
└── languages/checkmate/
    ├── config.toml                 # language metadata, brackets, indent patterns
    ├── highlights.scm              # syntax highlighting queries
    ├── brackets.scm                # bracket matching
    ├── outline.scm                 # outline / project search structure
    ├── indents.scm                 # syntax-aware auto indentation
    └── textobjects.scm             # Vim-mode text objects
```

## The language server

The server is the same single `cme` binary as the CLI toolchain
(WHITEPAPER: one binary for the whole developer setup). The extension
resolves it in this order:

1. **Your setting** — point at any binary:
   ```json
   {
     "lsp": {
       "cme": {
         "binary": { "path": "/path/to/cme", "args": ["lsp"] }
       }
     }
   }
   ```
2. **`cme` on your `$PATH`** — e.g. after
   `cargo install --path . --features cli` from the checkmate repository.
3. **The repository's own build** — when the opened project IS the
   checkmate repository and `cargo` has built it, `target/debug/cme` is
   used, so LSP changes can be exercised against the source tree.

Anchoring notes: megaprogram (§8) files surface expansion diagnostics —
those are anchored in the original text — while the expansion preview is
available through the `cme/expand` server request; parse/check spans of
expanded programs live in expanded-text coordinates, so they are not
published into such buffers. Schema (§9) files get schema diagnostics.
Everything else gets the full feature set.

## Installing for development

Zed loads extensions from disk in development mode:

1. Make sure this repository is cloned locally.
2. Open Zed → command palette → `extensions: Install Dev Extension` →
   select `editors/zed`.
3. Zed builds the grammar from `grammars/tree-sitter-checkmate` and
   registers `Checkmate` for `.cm` files.

Even for a local dev install, Zed fetches the grammar through git: it
clones `[grammars.checkmate] repository` at `rev` and builds the parser from
`path` inside that checkout. It never uses grammar files sitting next to the
extension, so `repository`/`rev` must resolve to a checkout that contains the
grammar. While the grammar commits live only on this machine, `extension.toml`
points at a local `file://` URL:

```toml
[grammars.checkmate]
repository = "file:///absolute/path/to/checkmate"
rev = "<commit containing grammars/tree-sitter-checkmate>"
path = "grammars/tree-sitter-checkmate"
```

After changing the grammar, commit it and bump `rev` to the new commit so Zed
rebuilds against it (uncommitted edits are invisible to the clone):

```sh
git rev-parse HEAD   # then set [grammars.checkmate] rev in extension.toml
```

Once the grammar commits are pushed, switch `repository` back to
`https://github.com/cmengine/checkmate` with the pushed `rev` so the
extension also installs on other machines.

## Publishing

This extension follows the [Zed publishing prerequisites](https://zed.dev/docs/extensions/prerequisites):

- kebab-case id (`checkmate`), no `zed`/`extension` in the id,
- grammars defined for every language provided,
- user-facing text in English.

Once the repository is public, publishing reduces to adding the extension as
a git submodule of `zed-industries/extensions` and an entry in
`extensions.toml` — see the [publishing guide](https://zed.dev/docs/extensions/publishing-guide).
