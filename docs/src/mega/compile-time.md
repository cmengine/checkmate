# Compile-Time Functions

Templates can call any **pure** Checkmate function with the `@` prefix.
Unmarked calls are ordinary runtime code emitted into the output;
`@`-marked calls *execute during expansion*, in the sandboxed interpreter,
under compile-time budgets.

## The shape

```checkmate
grammar py {
    // ... rules ...
    // plus ordinary pure functions in the same module:
}

mega def(py.def as d) {
    // ...
    ifStmt  => if ($cond) { @py.emitBody($body) }
    // ...
}
```

`@py.emitBody($body)` runs *now*, at compile time, with the captured
records/lists/texts/spans as arguments, and its result — a value or
**`code`** — is spliced into the expansion. The generated code that a
plain call `py.emitBody(x)` would have produced is *not* what happens
here: the `@` mark is the difference between calling code and running it.

## The `code` type

`code` is a compile-time-only syntax-fragment type. It is constructed by:

- the parsers — `$raw` captures, and the live islands `$expr` / `$type` /
  `$block`;
- the **builder API** — `cm.code.call`, `cm.code.fn`, and friends;
- parsing text (below).

A `code` value is an AST fragment that splices cleanly into templates. It
exists only during compilation — this is metaprogramming, not dynamic code
execution.

## Parsing APIs

- `cm.parseExpr(text, span)` / `cm.parseStmts(text, span)` — parse
  Checkmate expressions and statements. The text may itself contain
  `mega( ... ) { ... }` invocations, which enter the expansion queue like
  any other.
- `cm.parse(grammar.rule, text, span)` — delegate to any grammar rule
  (*late delegation*): parse previously captured text by a rule that was
  not the entry pattern. This is how `std.html` parses its captured
  `<script>` bodies with `js.program` and `<style>` bodies with
  `css.sheet` — after the HTML-level `until` has fixed the extent.

The `span` argument threads **provenance**: every node parsed from the
text carries it, so diagnostics for generated code keep pointing at the
embedded-language source even through recursive generators
(`@py.emitBody`, `@std.html.emitElement`, `@std.re.emitMatcher` are all
ordinary recursive Checkmate functions).

## Purity, enforced

Compile-time code may import only `self` modules and the core library.
**Schema imports inside compile-time-evaluated code are compile errors —
no exceptions, validators included.** No host access, no I/O, no clock, no
ambient compiler state. Purity is enforced by the import checker, not by
convention, and it is what makes expansions byte-reproducible.

## Budgets

Compile-time evaluation runs under the §5.5 **fuel** metering — an
*operation count*, never wall-clock time. Determinism again: the same
source consumes the same fuel everywhere. If a pathological grammar or
generator burns the budget (a `$raw` boundary search, a runaway
fixpoint), expansion terminates with a budget error naming the site
rather than hanging the build. The expansion-tree **depth cap of 64**
(counting all origins) bounds recursion; breadth is unbounded.
