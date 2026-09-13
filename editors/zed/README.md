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
├── extension.toml                  # extension manifest + grammar registration
└── languages/checkmate/
    ├── config.toml                 # language metadata, brackets, indent patterns
    ├── highlights.scm              # syntax highlighting queries
    ├── brackets.scm                # bracket matching
    ├── outline.scm                 # outline / project search structure
    ├── indents.scm                 # syntax-aware auto indentation
    └── textobjects.scm             # Vim-mode text objects
```

There is intentionally no Rust code and no language server: the extension is
purely declarative (grammar + queries), matching Zed's guidance for language
extensions without an LSP. When `cme-lsp` (WHITEPAPER §14) lands, it plugs in
as a `[language_servers]` entry here.

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
- no Rust code (no language server is shipped),
- user-facing text in English.

Once the repository is public, publishing reduces to adding the extension as
a git submodule of `zed-industries/extensions` and an entry in
`extensions.toml` — see the [publishing guide](https://zed.dev/docs/extensions/publishing-guide).
