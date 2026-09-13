# tree-sitter-checkmate

A [Tree-sitter](https://tree-sitter.github.io/) grammar for [Checkmate (CME)](../../WHITEPAPER.md) — the static, safe, embeddable scripting language.

The grammar covers the full implemented language surface:

- **Core language** — newline-significant statements, Appendix A operator
  precedence, structs/enums/generics, arrays and maps, `match` (statement and
  expression), the `?` propagation operator, impl blocks, dotted imports,
  and `$"..."` string interpolation with `{expr}` islands.
- **Megaprogramming (§8)** — `grammar` declarations with lexical profiles
  (`skip`/`comment`/`string`/`island`), the whole pattern language
  (`oneof`, `each sep`, `until`, `indent`, `where`, fragments with validators,
  `#complete`/`#hover`/`#token` annotations, ...), `mega` declarations with
  expansion templates, `name! { region }` invocations with
  brace-balanced foreign-content regions, and heredoc regions
  (`name! <<TAG ... TAG`, via an external scanner in `src/scanner.c`).
- **Schema files (§9)** — `schema`, `capability`, `interface`, `since`,
  `requires`, `optional`, `suspend`.

## Build and test

Requires the tree-sitter CLI (`npm install -g tree-sitter-cli`) and a C
compiler.

```sh
# regenerate the parser after editing grammar.js
tree-sitter generate

# run the corpus tests
tree-sitter test

# parse a file and print the syntax tree
tree-sitter parse ../../syntax.cm
```

## Validation against the repository fixtures

Every valid fixture in the repository parses without ERROR or MISSING nodes:

```sh
tree-sitter parse ../../syntax.cm   # the full-language surface fixture
tree-sitter parse ../../mega.cm    # the megaprogramming fixture
```

`boom.cm`, `broken_syntax.cm`, and `tests/fixtures/recovery/*` are
*intentionally damaged* recovery fixtures — errors there are expected and
correct behavior.

## Layout

| Path                  | Purpose                                             |
| --------------------- | --------------------------------------------------- |
| `grammar.js`          | The grammar source                                  |
| `src/scanner.c`       | External scanner for heredoc regions (§8.6)         |
| `src/parser.c` etc.   | Generated parser (committed so downstream builds work without regenerating) |
| `test/corpus/*.txt`   | Corpus tests: expected syntax trees per construct   |
| `queries/highlights.scm` | Canonical highlight query (copied into the Zed extension) |

## Consumers

- The Zed extension in [`editors/zed`](../../editors/zed) references this
  grammar directory.
- Any editor with tree-sitter support (Neovim, Helix, Emacs) can build from
  this repository: the scope is `source.checkmate`, file type `.cm`.
