# Checkmate TextMate grammar

A [TextMate](https://macromates.com/manual/en/language_grammars) grammar for
[Checkmate (CME)](../../WHITEPAPER.md), scoped as `source.checkmate`. It powers
syntax highlighting in VS Code, Sublime Text, GitHub, and any editor that
consumes `.tmLanguage` grammars.

## Coverage

- Core language: keywords, types, functions, operators, numbers, comments
- Strings with the accepted escape set, plus `$"..."` interpolation with
  `{expr}` islands highlighted as live expression scopes
- Megaprogramming (§8): `grammar`/`mega` declarations, the pattern language
  (fragments `$str`/`$tt<...>`, character classes, annotations
  `#complete`/`#hover`/`#token`), and templates (`$splice`, `@fn(...)`,
  `[each ...]`, `[when ... else ...]`)
- Magic invocation regions rendered as embedded foreign content
  (brace-balanced, string/comment aware) and heredoc regions
  (`name! <<TAG ... TAG`) as verbatim strings
- Schema files (§9): `schema`, `capability`, `interface`, `since`,
  `requires`, `suspend`, `optional`, version literals like `v1.4.0`

## Files

| File                          | Purpose                                    |
| ----------------------------- | ------------------------------------------ |
| `checkmate.tmLanguage.json`   | The grammar itself                         |
| `language-configuration.json` | Bracket pairing, comments, folding rules   |
| `package.json`                | VS Code extension wrapper                  |

## Use in VS Code

The directory is a minimal, installable VS Code extension:

```sh
cd editors/textmate
code --install-extension .   # or: npx vsce package && code --install-extension *.vsix
```

VS Code then opens every `.cm` file (scripts, mods, and schema files) with
Checkmate highlighting.

## Use in other TextMate hosts

Point your editor at `checkmate.tmLanguage.json` with scope
`source.checkmate` for files with the `.cm` extension. Sublime Text users can
copy the JSON into a `.sublime-syntax`-adjacent package; GitHub Linguist picks
grammars up via `languages.yml` contributions.

## Relation to the tree-sitter grammar

This grammar and
[`grammars/tree-sitter-checkmate`](../../grammars/tree-sitter-checkmate)
describe the same language and are kept intentionally in sync. Tree-sitter
drives Zed (and other tree-sitter editors); the TextMate grammar covers
regex-based highlighters.
