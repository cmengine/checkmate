# The Standard Grammar Library

The `std` root ships a library of grammars and megas alongside the core
language. Deleting them and re-implementing them in user space yields
identical behavior — they are ordinary megaprograms, with no private
compiler hooks.

| Grammar | Coverage | Key mechanisms exercised |
| --- | --- | --- |
| `std.json` | RFC 8259, strict (leading zeros rejected) | flow skipper, `oneof`, recursion, `scan`, `sep` |
| `std.toml` | TOML 1.0 incl. multi-line arrays and dotted keys | line mode + `soft`, explicit key charset, `require` |
| `std.yaml` | YAML 1.2 core: block/flow, anchors, tags, multi-doc, block scalars, merge keys, plain multi-line scalars | `indent`, `indent verbatim`, transparent comments, `peek`, `require` |
| `std.re` | ECMAScript regex plus PCRE lookbehind, named groups, possessive/atomic groups, inline flags; POSIX classes; conditionals/subroutines/`\Q…\E` rejected with diagnostics | guarded classes, ordered prefix choice, `until`, `@` codegen |
| `std.html` | HTML5 syntax minus foreign content; raw-text termination per spec | `i$tag`, whole-tail `until`, late parsing, delegation |
| `std.css` | selectors incl. nesting and `&`, `:has()`/`:is()`, at-rules, custom properties, `!important` | `context`, `$tt`, `scan`, string forms |
| `std.js` / `std.ts` | full ES statement/expression surface incl. ASI and restricted productions; TS via `grammar ts extends js` | line mode, `soft`, `eol`-ASI, postfix repetition, delegation |

Coverage is stated exactly; where a construct is out of scope, the grammar
rejects it **with a diagnostic** rather than silently mis-parsing it.

## JSON: the whole grammar, verbatim

```checkmate
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]

    rule value {
        oneof {
            null   => "null"
            bool   => oneof { t => "true", f => "false" }
            number => number
            string => $str text
            array  => ( "[" each sep "," { value } as items "]" )
            object => ( "{" each sep "," { member } as fields "}" )
        }
    }

    rule member {
        $str key ":" value
    }

    rule number {
        optional { "-" }
        oneof { zero => "0", pos => ( [1-9] as first scan [0-9] as rest ) }
        optional { "." scan [0-9] as frac }
        // ...
    }
}

mega value(json.value as v) {
    @toValue($v)
}
```

And the consumer side:

```checkmate
import std.json

infer config = mega(json.value) {
    {
        "host": "db.local",
        "ports": [5432, 6432],
        "retries": 3
    }
}
```

`@toValue` turns the capture tree into typed `code` constructing a
`jsonValue` enum; a CMON-oriented variant splices directly into struct
literals.

## TOML: line mode plus `soft`

TOML is line-oriented (tables are lines) with `soft` exactly where TOML
1.0 allows spanning: arrays and inline tables. Bare key segments are
`scan [A-Za-z0-9_-]` — deliberately **without** `.`, because `$tag` would
maximal-munch the dots and collapse `owner.name` into one segment. Table
reopening is semantic validation — a template `require` over the capture
tree, not a matching rule.

Trailing comments are consumed by `eol`; standalone comment lines are
transparent — the discipline lives in the machinery, not sprinkled
through the grammar.

## YAML: indentation, anchors, block scalars

The YAML grammar leans on the line machinery:

- `peek { line }` / `peek { not { line } }` dispatch same-line values from
  block values,
- `indent { ... }` with the strictly-deeper rule separates nested maps
  from following siblings — an outdented sibling at column 0 ends the
  nested block instead of being swallowed,
- `indent verbatim` captures block scalars (comments and blank lines
  included) column-exactly,
- aliases/anchors resolve through a template `require` over the whole
  tree; `<<` merge keys are ordinary rules.

## Regular expressions: the character-level acid test

`std.re` is ~90 rules over an empty skip set. Three guards make it parse
correctly: class items refuse `]` so a class never swallows its closer;
literal atoms are single characters so quantifiers attach to the last
character (`ab*` is `a` then `b*`); and the escape fallback accepts
escaped *punctuation only* — `\Q`, `\g`, or any unsupported alphanumeric
escape is a parse error pointing at its span. PCRE constructs outside the
declared scope (conditionals, subroutines, `\Q…\E`) **fail loudly instead
of silently becoming literals**.

The codegen dispatch is ordinary compile-time Checkmate:
backreference- and lookaround-free patterns compile to a linear-time
Thompson NFA; the rest to a memoized backtracking matcher. The macro
author owns the strategy.

```checkmate
import std.re

infer isEmail = mega(re.compile) {
    ^[\w.+-]+@[\w-]+(\.[\w-]+)+$
}
```

## HTML with embedded CSS and JavaScript: the composition showcase

`std.html` demonstrates the profile composition story end to end:

- tag and attribute names use `i$tag`, folded to lowercase — HTML is
  case-insensitive, so `<BR>` is void and `</DIV>` closes `<div>`,
- text nodes stop only at tag starts (`until { "<" [a-zA-Z!/] }`), so
  `1 < 2` is text, not a parse error,
- raw-text and script/style bodies are captured **lexically** —
  `until { "</" i$tag close where close == name }` — because that is
  HTML's actual rule (`<script>document.write("</script>")</script>`
  ends the script inside the string), and the bodies are parsed **late**
  (`cm.parse(js.program, ...)`, `cm.parse(css.sheet, ...)`) so string
  handling follows the *embedded* language exactly,
- non-void self-closing tags (`<div/>`) are errors — `std.html` is a
  validator; foreign content (SVG/MathML) is out of scope and rejected
  with a pointed diagnostic, not mis-parsed.

## JavaScript and TypeScript: line mode and ASI

`std.js` is line-oriented because **ASI is defined over line boundaries**.
Statements end at `;` (consuming following line ends) or at end of line;
`return` is the restricted production — its value is deliberately *not*
wrapped in `soft`, so `return` followed by a newline returns and the next
line starts a new statement, exactly as JavaScript specifies. Member/call
chains are a postfix repetition (no left recursion); template literals
declare an `island ( "${" "}" )` so nested megas inside `${ ... }` are
discovered and expanded. TypeScript is `grammar ts extends js` — the
inheritance chapter of [Grammars](../mega/grammars.md#grammar-inheritance)
made real.
