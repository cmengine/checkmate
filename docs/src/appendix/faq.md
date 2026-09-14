# FAQ

Quick answers to the questions that come up most. Each answer links to
the chapter with the details.

## Language

### Why no semicolons? Are newlines significant?

Yes — deliberately. A statement ends at end of line; expressions
continue across lines when the line ends with an operator or you are
inside parentheses. It kills ASI-style ambiguity and makes formatting
canonical. See [Significant Newlines](../language/newlines.md).

### Is there a `main` function?

Not in the language. §2.1: the **host** picks the entry point.
`cme run` follows the convention of invoking `main` (in a mod, from any
module); embedded hosts invoke whatever they choose via
`invoke`/`invoke_member` (Rust) or `cm_invoke` (C). See
[Functions → Entry points](../language/functions.md#entry-points-there-is-no-main-in-the-language).

### Why can't I call `c.peek()` on a struct? Why `counter.peek(c)`?

`impl` members are associated functions with an **explicit receiver
parameter** — no implicit `self`, no method-call syntax. Every call
site shows the target it dispatches to. See
[`impl` Blocks](../language/impl-blocks.md).

### How do I convert `int` to `float`?

You cannot implicitly — and there is no cast operator in this version.
Design signatures so types agree; watch the
[status page](../status.md#small-divergences-worth-knowing) for the
cast's arrival. `1 + 2.5` is an error today.

### Why is `a < b < c` an error?

Chained comparisons are a classic silent-mistake source; Checkmate
makes you write the real condition: `a < b && b < c`. Similarly, `&&`
and `||` may not mix without parentheses. See
[Mandatory Parenthesization](../language/operators.md#two-mandatory-parenthesization-rules).

### Where is `print`?

There are no runtime built-ins yet (`cme-runtime` is a placeholder) —
no `print`, no `len`, no math functions. `cme run` prints your
`main`'s return value in CMON form; embedded hosts receive values and
render them (`Display` in Rust, `cm_value_to_string` in C). Arrays have
`.length`; that is currently the only collection built-in. See
[Status](../status.md#small-divergences-worth-knowing).

### How do I handle "no value"?

`option<T>` with `match`. There is no `null` and no optional chaining;
the match is the only door and exhaustiveness guarantees the `None`
case is handled. See [Error Handling](../language/error-handling.md).

### Can functions throw?

No exceptions anywhere. Expected failure is `result<T, E>` + `?`;
unexpected runtime failure (overflow, division by zero, bad index)
terminates the invocation with a clean host-visible error. See
[Error Handling → Runtime failures](../language/error-handling.md#runtime-failures-are-not-values).

### Is `x` mutable?

All variables are mutable by default; there is no `mut`/`val`/`final`
local modifier. Types never change after declaration. See
[Variables](../language/variables.md).

### When do I use `infer`?

When the initializer makes the type obvious and you want concise
locals: `infer pos = vec2(x: 1.0, y: 2.0)`. It fails on ambiguity
(`infer xs = []`); use an explicit type there. See
[Variables → `infer`](../language/variables.md#infer-explicit-crystallization).

## Semantics

### Do assignments copy?

Everything copies — semantically. Assignment, parameters, returns,
struct fields, loop elements. Mutating a value is visible only through
the variable you mutated; "update elsewhere" means reassign. The
implementation shares buffers with copy-on-write, so copies of
unmutated data are cheap. See
[Value Semantics](../language/value-semantics.md).

### What happens on integer overflow?

The invocation terminates with a clean, positioned error. Same for
division by zero, out-of-bounds indexing, and missing map keys. Nothing
wraps silently; nothing panics the host. See
[Execution Limits and Errors](limits-and-errors.md).

### Can two invocations race?

Not on script data: modules hold no mutable global state, and every
invocation gets its own interpreter frame and limits. The host may run
as many invocations concurrently as it likes. See
[Embedding Overview](../embedding/overview.md).

## Megaprogramming

### Do megaprograms run at compile time?

They *expand* at compile time (before name resolution) — pure, cached,
deterministic — and the generated code is then checked like handwritten
code. `@`-function calls execute during expansion under fuel budgets.
See [Compile-Time Functions](../mega/compile-time.md).

### My braces keep closing the region early. What do I do?

That is what [heredocs](../mega/invocation.md#heredocs) are for:
`mega(name) <<TAG ... TAG` extends the region verbatim to the tag line.
The scanner never speculatively extends brace-balanced regions — a
wrong guess could absorb host code.

### Can I use a mega declared in another module of my mod?

Cross-file macro imports are out of scope in this version — a mega and
its invocation must resolve within the same file's expansion (see
[Megaprogramming → Current scope](../mega/overview.md#current-scope)).

### Which embedded languages ship?

JSON, TOML, YAML (1.2 core incl. anchors and merge keys), regex
(ECMAScript + much of PCRE), HTML5, CSS, JavaScript, and TypeScript —
all as ordinary megaprograms under `std.*`. See
[The Standard Grammar Library](../mega/std-library.md).

## Schemas and embedding

### What happens if a script calls a capability the host didn't register?

The **load fails** — the provider-presence check runs at load time, so
wiring mistakes are deterministic before any invocation. See
[Capabilities and Interfaces](../schema/contracts.md#the-three-gate-checklist).

### Can a script import `std::fs`-like functionality?

No ambient authority exists. Everything environmental is a
schema-declared capability the host registers — if the host did not
grant it, the import fails to compile. See
[The Schema System](../schema/overview.md).

### How do mods talk to each other?

They don't — no `import other_mod.*`. The host exposes an explicit
bridge capability (an event bus, a message queue) through the schema.
See [Mod Isolation](../mods/mods.md#isolation).

### Are the Rust and C bindings generated or hand-written?

Generated from the same schema file: the Rust procedural macro runs the
real schema front end at host build time; the C header is emitted by
`cme codegen-c` with `_Static_assert` signature checks. Signatures
cannot drift between script checking and host bindings. See
[Schema-Driven Embedding](../embedding/schema-embedding.md).

### Can a capability call back into the script?

Not into the **same context** mid-invocation — the §5.7 reentrancy
prohibition rejects it deterministically. Other contexts (and other
threads) are fine. See
[Execution Limits and Errors → Reentrancy](limits-and-errors.md#reentrancy-57).

## Tooling

### How do I see what a megaprogram generated?

`cme expand file.cm` writes `<stem>_expanded.cm` next to the original;
add `--provenance` to annotate each root mega site. The language server
exposes the same preview via the `cme/expand` request. See
[The `cme` CLI](../tooling/cli.md#expand).

### Where is the formatter?

Specified (§2.16.1's `--auto-crystallize` included), not shipped — see
the [status page](../status.md#specified-but-not-yet-implemented).

### Which editors are supported?

Zed (first-class extension + bundled LSP), VS Code/Sublime/GitHub
(TextMate), Neovim/Helix/Emacs (tree-sitter), and anything that speaks
LSP (run `cme lsp`). See [Editor Support](../tooling/editors.md).
