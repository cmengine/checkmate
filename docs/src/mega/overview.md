# Megaprogramming — Overview

Checkmate's megaprogramming system is a declarative macro facility with one
ambition: **any formal language — HTML, CSS, JavaScript, JSON, YAML, TOML,
regex — must be embeddable in Checkmate source, and the definition of that
embedding must itself be pleasant to read.**

The acid test is not "can macros generate boilerplate". It is: can a macro
author write a grammar for a real-world language — every feature — in a
page of pattern code, and get precise error messages pointing *into that
language's source*? The standard grammar library (`std.json`, `std.yaml`,
`std.toml`, `std.re`, `std.html`, `std.css`, `std.js`, `std.ts`) does
exactly that, using only the public system — no private compiler hooks.

## What it looks like

A megaprogram has two halves. The **declaration** — a pattern in
parentheses, a template in braces:

```checkmate
mega agent.spawn(
    "model:" $tag model
    "effort:" $word effort

    $text prompt
) {
    engine.SpawnAgent(
        model: $"{$model}"
        effort: $"{$effort}"
        prompt: $prompt
    )
}
```

And the **invocation** — the macro name applied to a brace-balanced region
of foreign text:

```checkmate
mega(agent.spawn) {
    model: claude-opus-latest
    effort: high

    Hi. Coordinate the player NPC patrol routes.
}
```

The region is *not* Checkmate. It is whatever the pattern says it is. The
pattern matches it, captures named pieces, and the template splices those
pieces into generated Checkmate code — which is then type-checked exactly
like hand-written code.

## The three artifacts

| Artifact | Role | Analogy |
| --- | --- | --- |
| `grammar` | A named, importable library of matching rules | The lexer + parser |
| `mega` | An entry point binding a pattern to an expansion template | The semantic action |
| Compile-time function | An ordinary pure Checkmate function invoked with `@` | The code generator |

Simple megaprograms need only a `mega` declaration. Complex ones compose
grammars, validators, and compile-time functions.

## How expansion works

```text
mega(name) { …region… }
     │
     ▼
┌────────────────────────────┐
│ Region scan & normalize     │ brace balancing under the composed
│ (strings/comments/islands)  │ lexical profile; edge trimming
└─────────────┬──────────────┘
              ▼
┌────────────────────────────┐
│ Packrat pattern match       │ grammar rules → capture tree
│ (deterministic, memoized)   │ every capture carries source spans
└─────────────┬──────────────┘
              ▼
┌────────────────────────────┐
│ Template elaboration        │ captures + template → Checkmate code
│ (@-calls run in the         │ spans remapped to the call site
│  sandboxed interpreter)     │
└─────────────┬──────────────┘
              ▼
repeat until no mega(…) invocation remains
              ▼
Name resolution & type checking of the final program
```

Key properties:

- **Expansion happens before checking.** Generated code is type-checked
  against the host schema exactly like handwritten code; errors in it
  point at the embedded-language source that produced it.
- **Megaprograms are purely script-side.** No host capabilities, no I/O,
  no clock — identical expansion on every platform.
- **Deterministic and terminating.** Ordered choice + packrat memoization;
  operation-count [fuel](../mega/compile-time.md#budgets) bounds compile-time
  work; the expansion-tree depth is capped at 64.

## Reading the chapters

1. [Grammars and Lexical Profiles](../mega/grammars.md) — how a grammar's
   `skip`/`comment`/`string`/`island` declarations define what "a brace"
   means inside a region.
2. [The Pattern Language](../mega/patterns.md) — literals, fragments,
   combinators, `where` constraints, indentation.
3. [Expansion Templates](../mega/templates.md) — splicing captures into
   generated code.
4. [Compile-Time Functions](../mega/compile-time.md) — `@`-calls, the `code`
   type, parsing APIs.
5. [Invocation and Heredocs](../mega/invocation.md) — region discovery,
   nesting, `<<tag` heredocs, `cme expand`.
6. [The Standard Grammar Library](../mega/std-library.md) — what ships, and
   the techniques each grammar demonstrates.

## Current scope

Megaprogramming is implemented **at the text level**: the expander emits
real Checkmate source (that is what `cme expand` writes out), expansion
runs per file inside mods before linking, and files that mention megas
expand automatically before `check`/`run`. One known limit: **cross-file
macro imports** — invoking another module's mega by qualified name — are
not supported yet; a mega and its invocation must share a file (or the
invocation must live where the mega is visible after expansion).
