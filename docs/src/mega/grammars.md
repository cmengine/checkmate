# Grammars and Lexical Profiles

A **grammar** is a named, importable library of rules — the matching
machinery behind megaprograms. Grammars and their rules are script-internal
and follow camelCase naming.

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

    // rule number: optional "-" etc.
}
```

## The lexical profile

Five lexical declarations define a grammar's **profile** — its opinions
about raw text:

| Declaration | Meaning |
| --- | --- |
| `skip [ ... ]` | Characters ignored between elements (the skip set) |
| `comment ( "/*" until "*/" )` | A comment form (start/end, or one delimiter to end-of-line) |
| `string ( '"' )` | A string form: opens/closes on the delimiter, backslash escapes honored |
| `string ( '`' multiline island ( "${" "}" ) )` | A multiline string with an interpolation island |
| `island ( "${" "}" )` | Attached to a string form: delimiters transparent to brace balancing |

The profile matters far outside matching: the **invocation-region scanner**
uses it to decide where a mega region actually ends (see
[Invocation](../mega/invocation.md)), and `$tt` balancing honors its string
forms.

## Line-oriented vs. flow-oriented

If the skip set contains a line terminator, the grammar is
**flow-oriented**: newlines are layout, skipped between elements, and
`eol`/`line`/`indent` are unavailable. Otherwise it is **line-oriented**:
the skipper never crosses a line boundary, and line structure is visible —
`eol`, `line`, and `indent` are first-class.

This is the most consequential choice a grammar author makes:

- JSON, CSS, and HTML are flow-oriented.
- YAML, TOML, Python — and, necessarily, JavaScript — are **line-oriented**.
  JavaScript's ASI and restricted productions are *defined* over line
  boundaries; a grammar that cannot see lines cannot express them. Inside a
  line-oriented grammar, `soft { p }` grants newline-skipping exactly where
  the language allows it (inside brackets, after operators) — see
  [Patterns → Combinators](../mega/patterns.md#combinators).

## Comments in line mode

In flow grammars, comment forms are consumed by the skipper. In line
grammars they are **not** auto-skipped; instead:

- a **transparent line** — only skip-set characters, optionally plus comment
  forms — is skipped by `eol`, `eof`, and the `indent` protocol, and
- `eol` consumes the line terminator plus the following **transparent tail**
  (skip characters and consecutive comment forms).

Interior comments are matched explicitly, as pattern alternatives. The
consequence: a comment line — at *any* column — can never open, close, or
swallow an indented block. TOML's trailing `# comment` is consumed by
`eol`; YAML's outdented comment lines are transparent; nothing strands.

## Strings and islands

String forms are consumed by the profile's *consumers* — the region scanner
and `$tt` — never by the skipper. In-pattern matching uses fragments
(`$str`) and explicit rules, so `"a # not a comment"` in TOML and
`content: "/* keep */"` in CSS settle correctly **by construction**: by the
time the skipper next runs, the string — `#` and all — was already consumed
atomically.

An **island** declaration makes the scanner treat the island delimiters as
transparent to brace balancing, so nested macro invocations inside string
interpolations are discovered and expanded:

```checkmate
// js declares: string ( '`' multiline island ( "${" "}" ) )
mega(js.run) {
    const greeting = `Hello, ${ mega(json.value) { "world" } }!`
}
```

## Rules

Rules reference each other by bare name (recursion is a self-reference) or
qualified name across grammars; `recur` is sugar for the innermost
enclosing rule. Rules may declare **context** parameters — ancestor data
flowing down recursions:

```checkmate
grammar css {
    skip    [ ' ', '\t', '\r', '\n' ]
    comment ( "/*" until "*/" )
    string  ( '"' )
    string  ( "'" )
    island  ( "${" "}" )

    rule styleRule(context { selector parent = none }) {
        selector sel "{"
        each { declaration } as decls
        each { styleRule with context { parent: sel } } as nested
        "}"
    }
    // ...
}
```

Context flows only downward, is always explicit, and is visible to `where`
clauses and templates. Fields may declare defaults (`= none`), so a
top-level call without `with context` composes with delegated calls. See
[Patterns → Ancestor context](../mega/patterns.md#ancestor-context).

## Grammar inheritance

Grammars **extend** other grammars:

```checkmate
grammar ts extends js {
    rule type { ... }
}
```

- The child inherits the parent's rules and **lexical profile**
  (`skip`, `comment`, `string`, `island`), and may override or append to
  them. `std.ts` is `grammar ts extends js` plus type syntax — the whole
  TypeScript story rides on rule override plus profile inheritance.
- This is also how user megaprograms patch the shipped grammars: extend
  `std.json`, override one rule, and your dialect works everywhere the
  grammar is referenced.

## Profile inheritance for megas

A mega whose entry pattern is a rule reference **inherits that grammar's
profile** for matching *and* for region scanning. A mega with an inline
pattern (fragments directly in the parentheses) uses the default profile
(horizontal and newline skipping, `"` strings, no comments). Line-mode
machinery (`indent`, `eol`, `line`) is reachable only through
line-oriented grammars; the compiler rejects it in flow-oriented contexts
and inside `soft`.
