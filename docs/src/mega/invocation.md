# Invocation and Heredocs

How a `mega(name) { ... }` invocation finds its region, how regions nest,
and the escape hatch when brace balancing is not enough.

## Region location

The parser resolves `name` in the **macro namespace** (imports precede
use; unresolved names are parse errors), then locates the region by
brace balancing under the **composed profile**: the comment, string, and
island forms of the entry grammar *and of every grammar its entry pattern
references*, transitively. At each position the scanner tries comment
forms longest-first, then string forms longest-first; everything else
counts braces.

Composition is what makes nesting *just work*. One profile could never
know all three of these, but the pattern's own reference graph does:

- `console.log("}")` — a js string containing a brace,
- `// it's fine` — a js comment with a quote,
- `` `a ${b} c` `` — a js multiline string with an island,
- `console.log(\`val: ${ mega(json.value) { 1 } }\`)` — a nested mega
  inside a js island, balanced through the island.

## Region normalization

The region excludes one line terminator immediately after `{`, one
immediately before `}`, and horizontal whitespace at the region's start
and end. Line-oriented grammars therefore begin matching at the first
content character — `mega(def) {⏎    def clamp(...` matches `def` directly,
and `mega(re.compile) {⏎    ^[\w.…` does not silently absorb the
indentation into the regex (std.re's skip set is empty).

## No speculative extension

If the pattern fails at the region's last character, the diagnostic
reports the furthest failure plus a scan hint where relevant — *an inner
`}` invisible to every composed profile (a brace inside an embedded regex
literal, say) may have closed the region early; the heredoc form is
exact.* The scanner never guesses a larger extent: a wrong guess could
silently absorb host code into the macro.

## Heredocs

When brace balancing is the wrong tool, the **heredoc form** delimits the
region by a line tag:

```checkmate
mega(name) <<TAG ...foreign text, braces irrelevant...
TAG
```

The region extends verbatim to the first line whose content is exactly
the tag; only the usual edge trims apply. Use it when the embedded
language's braces cannot be trusted under any profile — or when you
simply want a hard, visible boundary.

## Declare before invoke

Invocation names resolve at parse time: a mega must be **imported, or
declared earlier in the same file, before it is invoked**. (Template-authored
invocations — code a mega generates that itself invokes megas — are
exempt from the static check and resolve during expansion.) Megaprogram
declarations live in module scope alongside functions and types.

## The expansion queue

`mega()` invocation nodes enter one worklist from exactly three places,
and all three are processed identically:

1. literal template text;
2. text parsed at compile time — `$raw` captures, `$expr`/`$type`/`$block`
   islands, and `cm.parse*` results;
3. `code` values returned by `@`-functions.

After each pass, the expander sweeps the tree in fixed depth-first source
order and enqueues every remaining invocation, repeating until none
remain — bounded by the expansion-tree depth cap of 64 (nesting depth,
not pass count; sibling invocations at the same depth expand in the same
pass). Origin is recorded only for diagnostics; every node goes through
exactly one name-resolution and type-check pass at the end.

## `cme expand`

Files that declare or invoke megaprograms expand automatically before
`check`/`run` — the source behaves exactly like its expansion. To see the
generated program:

```sh
cme expand mega.cm
```

writes `mega_expanded.cm` side by side with the original — pure Checkmate,
every megaprogram invocation replaced by its generated code — then parses
and checks that file. Add `--provenance` to annotate each root mega site
with `// @ name! src:line:col`; without the flag the output is
byte-deterministic. Expansion runs per file inside a mod too: each
module's megaprograms expand before the mod links into one program.

The [language server](../tooling/lsp.md) exposes the same preview through
the `cme/expand` custom request.

## Invocation regions are foreign text

Inside `name! { ... }` / `mega(name) { ... }` regions, the editor stays
silent — the text belongs to the embedded language, and Checkmate
tooling does not darken it. Completion, hover, and diagnostics inside
invocation regions come from the pattern's diagnostics instead
([Patterns → Failure semantics](../mega/patterns.md#failure-semantics-and-diagnostics)).
