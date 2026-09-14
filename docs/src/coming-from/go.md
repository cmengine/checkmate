# Coming from Go

Checkmate will feel structurally familiar: C-family blocks, explicit
types, no inheritance, small surface. The differences that matter are
value semantics, the missing goroutines/runtime, and the capability
boundary.

## The two-minute syntax map

| Go | Checkmate |
| --- | --- |
| `func f(x int) int` | `int f(int x)` |
| `var x int = 1` / `x := 1` | `int x = 1` / `infer x = 1` |
| `if x > 0 { }` | `if (x > 0) { }` — parentheses required |
| `for i, v := range xs` | `for (int v in xs)` — no index form |
| `for cond { }` | `while (cond) { }` |
| `type P struct { ... }` | `struct p { ... }` |
| `switch` / `type switch` | `match` — exhaustive, on enums |
| `error` returns, `if err != nil` | `result<T, E>` and `?` |
| methods with receivers | `impl` blocks, explicit receiver param, qualified calls |
| `nil` | no `null`; `option<T>` |
| `nil` map/slice | no absent collections; declare `map<K,V> m = {}` |
| `panic` / `recover` | runtime failures terminate the invocation cleanly |
| goroutines, channels | nothing — the host owns concurrency |

## Values, not references

Go programmers carry a mental model of "structs are values, slices/maps
are references". Checkmate has **one** rule: everything is a value.

```go
// Go: this mutates the caller's map
func bump(m map[string]int) { m["gold"] += 30 }
```

```checkmate
// Checkmate: the parameter is an independent copy
void bump(map<str, int> loot) {
    loot["gold"] += 30        // invisible to the caller
}
```

Assignment and parameter passing clone (cheaply — copy-on-write under the
hood). "Update" means reassignment:

```checkmate
p = damage(p, 60)
loot["gold"] += 30            // fine — you own `loot`
```

No pointer receiver vs. value receiver decision, no `*p`/`&p`, no
nil-checking discipline — because there are no pointers and no nil.

## Errors: `result` instead of `error`

Go's `if err != nil` becomes a value flow:

```go
// Go
total, err := safeDiv(a, b)
if err != nil {
    return 0, err
}
```

```checkmate
// Checkmate: `?` is the `if err != nil { return err }`
result<int, str> chain(int a, int b) {
    int t = safeDiv(a, b)?
    return Ok(t)
}
```

The error type must match exactly — no `fmt.Errorf` wrapping, no
`errors.Is` gymnastics. For structured errors, declare an error enum and
`match` on it exhaustively.

## No goroutines: the host owns concurrency

This is the biggest conceptual shift. Checkmate has **no scheduler, no
goroutines, no channels, no `select`**. A script invocation is a pure,
bounded computation the host starts and waits on:

- Independent invocations are race-free by construction — modules hold no
  mutable global state, so there is nothing to share.
- Want parallelism (fan-out HTTP, batch work)? The *host* exposes a
  batching capability — a schema function that takes a list and does the
  concurrency internally:

  ```checkmate
  // host-provided; the host parallelizes and joins
  str[] results = engine.http.GetAll(str[] urls)
  ```

- Time, timers, IO: all host capabilities, never language built-ins.
  There is no `time.Now()` in the language.

## The entry point is the host's choice

`func main()` is a Go concept. In Checkmate the host picks what to invoke
— a top-level function or an interface member. `cme run` follows the
convention of calling `main`; an embedded host calls
`context.invoke("tick", ...)` or
`context.invoke_member("engine.gamemode", "OnTick", ...)`. Your mod is a
library of invocable entry points, not a process.

## Structs and methods

```go
type counter struct{ value int }
func (c counter) Bump() counter {
    c.value++
    return c
}
// counter{41}.Bump().value == 42
```

```checkmate
struct counter {
    int value
}

impl counter {
    counter bump(counter c) {
        c.value += 1
        return c
    }
}

// counter.bump(counter(value: 41)).value == 42
```

Differences: the receiver is an explicit named parameter; calls are
qualified (`counter.bump(c)`), never `c.bump()`; there is no embedding,
no interfaces, no method sets. Polymorphism = enums + `match`.

## Packages → mods

A Go module's packages become one Checkmate [m[mod](../mods/mods.md): a
`mod.toml` plus a `src/` tree whose paths are `self.*` import paths.
There is no package manager and no cross-mod imports — mods cannot import
sibling mods at all; inter-mod communication is a host-provided bridge
capability. Where you would reach for a Go interface between packages,
the host's [schema interface](../schema/contracts.md) is the contract.

## What Go has that Checkmate deliberately does not

- **Semicolons-free but newline-significant**: the formatter-friendliness
  of `gofmt` is built into the grammar itself.
- **Generics on functions**: only types are generic today.
- **Defer**: no `defer`; deterministic cleanup is the runtime's ARC, and
  host resources are managed by the host.
- **Reflection**: none. Types are checked, not inspected.

## A tiny side-by-side

```go
package main

import "fmt"

func sum(values []int) int {
    total := 0
    for _, v := range values {
        total += v
    }
    return total
}

func main() {
    fmt.Println(sum([]int{1, 2, 3}))
}
```

```checkmate
int sum(int[] values) {
    int total = 0
    for (int v in values) {
        total += v
    }
    return total
}

int main() {
    return sum([1, 2, 3])
}
```

Run it with `cme run file.cm` — `6` prints in CMON form.

## Where to go next

- [Value Semantics](../language/value-semantics.md) — internalize the one
  rule.
- [Error Handling](../language/error-handling.md) — `option`/`result`/`?`.
- [Mods](../mods/mods.md) — packaging your code.
- If your Go program *embeds* scripting: [Embedding in Rust](../embedding/rust.md)
  (your Go host would speak the same shape through the C ABI —
  [Embedding in C](../embedding/c.md)).
