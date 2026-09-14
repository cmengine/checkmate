# Coming from Rust

Checkmate's implementation is Rust, and its type story will feel familiar
— ADTs, exhaustiveness, no `null`, `Result`. What is gone is the borrow
checker, and what replaces ownership is **value semantics with implicit,
cheap clones**. This page maps the two worlds.

## The two-minute syntax map

| Rust | Checkmate |
| --- | --- |
| `fn add(a: i32, b: i32) -> i32` | `int add(int a, int b)` |
| `let x: i32 = 1;` / `let x = 1;` | `int x = 1` / `infer x = 1` |
| `let mut x = 1;` | `int x = 1` — mutable by default |
| `struct P { x: f64 }` | `struct p { float x }` |
| `enum E { A(i32), B }` | `enum e { A(int amount), B() }` |
| `match e { A(v) => ..., _ => ... }` | `match (e) { A(int v) => ... _ => ... }` |
| `Option<T>`, `Some`/`None` | `option<T>`, `Some`/`None` |
| `Result<T, E>`, `Ok`/`Err`, `?` | `result<T, E>`, `Ok`/`Err`, `?` — same operator |
| `impl P { fn f(&self) }` | `impl p { int f(p self_) { ... } }` — explicit receiver param |
| `&T` / `&mut T` | nothing — values, cloned on assignment/pass |
| `Box`/`Rc`/`Arc` | nothing visible — ARC + COW under the hood |
| `Clone::clone()` | implicit and cheap |
| `trait` | schema `interface` (a host contract), not a language feature |
| generics | on structs and enums |
| `String`/`&str` | `str` — one immutable UTF-8 type |
| `Vec<T>` | `T[]` |
| `HashMap<K, V>` | `map<K, V>` |
| `panic!` | clean invocation-terminating error (never a host crash) |
| `unwrap()` | none — `match` or `?` |
| cargo crate | a [m[mod](../mods/mods.md) |

## No borrow checker — because there are no borrows

Every value is owned by the variable holding it. Assignment is `Clone`
semantics with implementation sharing (ARC + copy-on-write), so the cost
profile of a "clone" is a refcount bump until mutation:

```rust
// Rust
let a = vec![1, 2, 3];
let b = a.clone();        // explicit
```

```checkmate
// Checkmate
int[] a = [1, 2, 3]
int[] b = a               // a copy — semantically; COW in practice
b[0] = 99                 // b diverges here; a still [1, 2, 3]
```

There is no `&`, no `&mut`, no lifetimes to annotate, no `Send`/`Sync` to
reason about in script code, no `Rc<RefCell<...>>` dance. Mutation is
always *local*; "update elsewhere" is reassignment. If Rust taught you to
design around ownership, design Checkmate the way you would design an
all-`Clone`-derive API: pure functions in, new values out.

## The receiver is a parameter

```rust
// Rust
impl Counter {
    fn bump(mut self) -> Counter { self.value += 1; self }
}
counter.bump()
```

```checkmate
// Checkmate
impl counter {
    counter bump(counter c) {
        c.value += 1
        return c
    }
}

counter.bump(c)     // qualified call; receiver passed explicitly
```

There is no method-call syntax and no `self` keyword — the receiver is an
ordinary first parameter named whatever you like, and value semantics
apply to it exactly like any other parameter.

## Enums, `option`, `result`, `match`, `?`

This half is nearly one-to-one:

```checkmate
option<int> findEven(int[] values) {
    for (int v in values) {
        if (v % 2 == 0) {
            return Some(v)
        }
    }
    return None()
}

result<int, str> chain(int a, int b, int c) {
    int first = safeDiv(a, b)?
    int second = safeDiv(first, c)?
    return Ok(first + second)
}
```

Differences worth knowing:

- Patterns bind **with types**: `Some(int v)`, not `Some(v)`.
- `match` arms are newline-delimited and need no commas.
- The `?`-propagated error type must match the enclosing function's error
  type **exactly** — no `From` conversions, no `Box<dyn Error>`.
- There is no `Option<T>` method surface (`.unwrap()`, `.map()`,
  `.and_then()`): `match` is the combinator. Write helpers where you
  would write them in a no-std context: as plain functions.

## Strings, arrays, maps

```checkmate
str s = "hp: " + 100                    // concatenation stringifies
str t = $"x={p.x} y={p.y}"              // interpolation
int[] xs = [1, 2, 3]
xs.length                                // 3
map<str, int> m = {"gold": 120}
m["gems"] = 3                            // insert
```

- `str` is immutable; there is no `String` vs `&str` split.
- Arrays have `.length`; there is no iterator/adapter ecosystem (no
  `iter()`, `map`, `filter`) — loops are the idiom.
- Maps insert via index assignment; **reading a missing key terminates
  the invocation** (it is not `Option`-returning).
- No indexing panics: out-of-bounds is a clean, positioned error.

## What replaces traits

Script code has no traits, no generics-on-functions, no dyn dispatch.
Runtime variation = enums + `match`. Compile-time variation = generic
structs/enums. **At the host boundary**, the schema `interface` plays the
trait role: the script implements it with `impl` blocks, the host's
generated Rust trait is compile-time-verified against the same schema —
see [Schema-Driven Embedding](../embedding/schema-embedding.md).

## Concurrency: none, by design

No `std::thread`, no `tokio`, no channels. A script invocation is a pure
computation between host-supplied inputs and host-owned state; the host
may run many invocations concurrently — they are race-free because they
share nothing mutable. Long-running host work is a capability call, and
the planned `suspend` machinery will make those calls yield to the host's
executor transparently (still no `async` keywords in source).

## Where a Rustacean should slow down

1. **Significant newlines.** No semicolons; a statement ends at line end.
   Continuation requires a trailing operator or parentheses —
   [Significant Newlines](../language/newlines.md).
2. **No implicit numeric conversions** — and no `as` casts yet either.
   Design signatures to avoid cross-type arithmetic.
3. **Mandatory parenthesization** — `a && b || c` and `a < b < c` are
   compile errors ([Operators](../language/operators.md)).
4. **The host boundary is a capability wall.** No `std::fs`, no
   `std::net`, no clock: everything environmental is a schema-gated
   capability the host grants ([The Schema System](../schema/overview.md)).
5. **`main` is not special** (in the language). The host invokes
   `context.invoke("entry", ...)`; `cme run`'s `main` is a CLI convention.

## Reading the implementation

The interpreter is the reference oracle for observable behavior —
[`crates/cme-interp`](https://github.com/cmengine/checkmate/tree/mom/crates/cme-interp)
— and the whitepaper's Appendix A is normative for operators. If you are
embedding Checkmate *in* your Rust project, go straight to
[Embedding in Rust](../embedding/rust.md).
