# The Pattern Language

The pattern inside `mega name( ... ) { ... }` describes the foreign
region's shape. It is a packrat grammar language: deterministic, memoized,
with furthest-failure diagnostics.

## Match domain and the skipper

Matching runs on a cursor over the raw region text. Three rules govern
skipping:

1. The skipper runs **immediately before exactly these elements**:
   literals, classes, fragments, rule references, groups, `oneof` branches,
   `each` iterations, `optional` attempts, `soft` regions, and `indent`
   starts. It **never** runs before `eol`, `eof`, `line`, `where`,
   `label`, `peek`/`not` — those observe the cursor as it stands.
2. **Atomic matchers never skip internally.** `$str`, `$word`, `$tag`,
   `$int`, `$float`, `$ident`, `$text`, `$raw`, `$expr`, `$type`, `$block`,
   `scan`, `until`, `lineRest`, `$tt`, `$template`, and `raw { ... }`
   consume their extent in one step; skip characters and comments inside
   that extent are data.
3. Zero-width assertions (`peek`, `not`, `line`) restore the cursor
   entirely.

Rule 2 settles the comment-in-string problem by construction. Rule 1 has a
corollary called **loose string matching** — building strings from element
sequences lets the skipper eat string-internal content:

```checkmate
// wrong: the skipper runs before `any`, eating spaces inside the string
rule badString { "\"" each { not { "\"" } any } as chars "\"" }

// right: an atomic fragment, or a raw region
rule goodString { $str text }
rule alsoGood   { "\"" raw { each { not { "\"" } any } as chars } "\"" }
```

## Literals and classes

| Form | Matches |
| --- | --- |
| `"lit"` / `i"lit"` | exact character sequence (`i`: case-insensitive) |
| `[a-z0-9_]` / `[^...]` | exactly **one** character in / not in the set |
| `any` | any single character |
| `scan [...]` | maximal run of ≥ 1 characters from the set |

```checkmate
rule dottedKey {
    each sep "." {
        oneof { bare => scan [A-Za-z0-9_-] as part, quoted => $str part }
    } as parts
}
```

## Combinators

| Form | Matches | Capture |
| --- | --- | --- |
| `optional { p }` | `p` or nothing, atomically | `opt` capture |
| `each { p }` / `each+` | zero-or-more / one-or-more | list `as xs` |
| `each sep pattern { p }` | repetition with separator | list `as xs` |
| `each sep p trailing { p }` | ...allowing a trailing separator | list |
| `each [n, m] { p }` | bounded repetition | list |
| `oneof { label => ( p ) ... }` | first matching branch, in order | tagged record |
| `peek { p }` / `not { p }` | zero-width positive/negative lookahead | — |
| `( p )` | grouping | `as name` |
| `ruleName` / `grammar.ruleName` | rule reference / delegation | `as name` |
| `recur` | innermost enclosing rule | — |
| `where cond` | validates captures in scope | — |
| `raw { p }` | `p` with the skipper suspended | — |
| `soft { p }` | line mode: newlines join the skip set within `p`; atomic | — |
| `label "msg" { p }` | diagnostic context for failures inside `p` | — |

**Ordered choice with complete fall-through.** A `oneof` tries branches in
declaration order; a branch fails if *any* element of its sequence fails —
including a failed `where` — and the next alternative is tried. **There is
no point where entering a branch becomes irreversible.** Prefix-colliding
alternatives (regex's `(?<!`, `(?<=`, `(?<name`) are written
longest-prefix-first, which is exactly how ordered choice stays
deterministic.

## Fragments

Fragments are pre-parameterized matchers that bind a capture directly:

| Fragment | Matches | Capture kind |
| --- | --- | --- |
| `$ident` | a Checkmate-valid identifier | ident |
| `$word` | foreign identifier `[A-Za-z_$][0-9A-Za-z_$]*` | text |
| `$tag` | relaxed token: letters, digits, `-`, `.`, `_` | text |
| `$int` / `$float` | numeric literal forms (incl. `0x`/`0o`/`0b`) | int / float |
| `$str` | double-quoted string with escapes | str |
| `$tt` / `$tt<"{{" "}}">` | a token or balanced delimiter tree, honoring profile string forms | text |
| `$text` | effective tail if one exists, else remainder of region | text |
| `$template` / `$template<open close rule>` | tail split at island delimiters | tagged parts |
| `$raw` / `$raw<grammar.rule>` | tail up to the next sibling, parsed as Checkmate code / by a rule | code / record |
| `$expr` / `$type` / `$block` | a live Checkmate island, parsed by the real parser | code |

Case-insensitive forms: `i"lit"`, `i$tag` (folds the capture to lowercase —
how HTML's tag names stay first-class).

Any fragment accepts a **validator** — `$ident<self.notReserved>`,
`$word<std.html.voidTag>` — naming a grammar rule (the matched text must
match it) or a pure function from the same module. Validators never touch
host schemas.

**Live islands** make embedded data contain real, type-checked Checkmate
expressions:

```checkmate
mega ui.banner($template body) {
    ui.compound([each in body {
        match ($item) {
            text => ui.label($"{$item.text}")
            expr => ui.live($item.value)
        }
    }])
}

ui.banner {
    Welcome back, {{ playerName }}!
    You have {{ player.score }} points.
}
```

## `where` constraints

`where` is an ordinary pattern element: it consumes no input and succeeds
only if its compile-time condition over captures is truthy. Its scope is
every capture bound within the **current rule invocation** plus the rule's
declared `context` — deliberately *not* captures from the calling rule,
which is what keeps packrat memoization sound.

Conditions may use equality and comparison, `&&`/`||`/`!`,
`some x in xs { ... }` / `all x in xs { ... }`, `present(x)` for optional
captures, capture accessors (`.line`, `.col`, `.span`), and calls to pure
functions.

```checkmate
rule element {
    "<" i$tag name
    // ...
    oneof {
        scriptEl => ( where name == "script" ">" ... )
        selfClose => ( where @isVoid(name) "/>" )
        // ...
    }
}
```

The division of labor with templates: a check that must **steer which
alternative matches** belongs in `where`; a check that only needs to
**reject the final result** (needing captures from anywhere in the tree)
belongs in the template's `require(cond, "message")`. See
[Templates](../mega/templates.md#require) and
[Compile-Time Functions](../mega/compile-time.md).

## Indentation: `indent`

For line-oriented grammars, `indent { p }` matches an
indentation-delimited block. The protocol, in brief:

1. **Start** at end of line: consume the newline, skip transparent lines,
   and set the base column `B` from the next non-transparent line.
2. **Depth**: a nested block's base must be **strictly greater** than its
   parent's — this is what separates a nested block from a following
   sibling, and what kills the "swallow the rest of the document"
   ambiguity.
3. **Iteration**: each iteration starts at a line whose column equals `B`;
   `p` matches; at most a comment may trail.
4. **Termination**: the block ends cleanly when the next line dedents
   below `B` (one dedent closes several nested blocks at once); a line at
   an unmatched column ≥ B is a **committed-block failure** — backtrackable
   like any failure, but reported in preference to ordinary furthest
   failures.

`indent verbatim as name` captures every line at column ≥ B verbatim —
blank and comment lines included; this is how YAML block scalars and
Python-style raw bodies work. Tabs advance to the next multiple of 8;
mixed tabs-and-spaces in one indentation prefix is a committed-block
failure.

## Failure semantics and diagnostics

Matching failures report at the **furthest position reached**, with the
alternatives expected there, contextualized by `label` blocks and rule
names; committed-block failures take precedence; and if a later branch
succeeds, all recorded failures are discarded:

```text
error[mega]: mods/hud/src/hud.cm:17:5
    constraint failed: close == name  ('div' ≠ 'p')
    element opened at mods/hud/src/hud.cm:15:5
    ┆ <p class="hud">
    ┆  ^^^ while matching 'html.element' → branch 'normal' → 'close'
```

A mega invocation's pattern must consume the entire region; leftover
content is reported the same way.

## The pattern grammar

The condensed EBNF lives in §8.3.10 of the
[whitepaper](https://github.com/cmengine/checkmate/blob/mom/WHITEPAPER.md).
The `#complete(...)` / `#hover("...")` / `#token("...")` annotations may
appear between any terms; they are **inert editor metadata** consumed by
the language server — the only place host registries are visible.
