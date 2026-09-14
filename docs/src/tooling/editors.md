# Editor Support

Checkmate ships its own editor tooling in-tree, next to the compiler it
must stay in sync with. One grammar surface, three consumers:

| Directory | What it is | Who consumes it |
| --- | --- | --- |
| `grammars/tree-sitter-checkmate` | Tree-sitter grammar + corpus tests + external heredoc scanner | Zed extension, Neovim/Helix/Emacs |
| `editors/zed` | Zed extension: highlighting, brackets, outline, indents, text objects | Zed |
| `editors/textmate` | TextMate grammar (`source.checkmate`) + installable VS Code wrapper | VS Code, Sublime, GitHub |

All three cover the full implemented surface: the core language, §8
megaprogramming (`grammar`/`mega` declarations, the pattern language,
expansion templates, brace-balanced invocation regions, heredocs), and
§9 schema files. The tree-sitter grammar is validated against every
fixture in the repository — `syntax.cm`, `mega.cm`, the schema files,
and the mod trees all parse without error nodes (the deliberately
damaged recovery fixtures are expected to produce them).

## Zed

The Zed extension lives at
[`editors/zed`](https://github.com/cmengine/checkmate/tree/mom/editors/zed)
(see its README for the current install steps):

```text
Zed → extensions: Install Dev Extension → editors/zed
```

The extension bundles the tree-sitter grammar (highlighting, brackets,
outline, indents, text objects) and **launches `cme lsp` automatically**
for diagnostics, completion, hover, go-to-definition, and the schema-
and megaprogramming-aware features described in
[The Language Server](lsp.md).

## VS Code / Sublime / GitHub

The TextMate grammar at
[`editors/textmate`](https://github.com/cmengine/checkmate/tree/mom/editors/textmate)
(`source.checkmate`) drives syntax highlighting in VS Code (via its
installable wrapper), Sublime Text, and GitHub's code rendering. See
that directory's README for installation.

## Neovim / Helix / Emacs

Use the tree-sitter grammar directly from
`grammars/tree-sitter-checkmate` (parser + `highlights.scm` queries),
and point any LSP client at `cme lsp` over stdio:

```lua
-- Neovim sketch
require('lspconfig').configurations.checkmate = {
  cmd = { '/path/to/cme', 'lsp' },
  filetypes = { 'checkmate' },
}
```

Build and test the grammar locally:

```sh
cd grammars/tree-sitter-checkmate
tree-sitter generate && tree-sitter test
```

## Language server, everywhere

Any editor that speaks LSP gets the full analysis by running the `cme`
binary with the `lsp` subcommand — no per-editor plugin required beyond
generic LSP configuration. See [The Language Server](lsp.md) for the
feature list and the auto-detection rules for mods and schemas.
