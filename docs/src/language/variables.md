# Variables, Mutability, and `infer`

## Declaration

Variables are declared by placing the type before the identifier, with an
initializer. **Uninitialized declarations do not exist** — every variable
is born with a value.

```checkmate
int score = 0
float speed = 4.5
str title = "Checkmate"
bool active = true
vec2 pos = vec2(x: 10.0, y: 5.0)
int[] values = [1, 2, 3]
map<str, int> loot = {}
```

Empty collection literals are fine *with* a declared type:

```checkmate
int[] empty = []            // ok: element type comes from the declaration
map<str, int> fresh = {}    // ok
```

## Mutability by default

Variables are **mutable by default**. There is no `let`, no `var`/`val`
distinction, no `mut` — the type is the whole story:

```checkmate
int score = 0
score = score + 10
score += 5                  // compound assignment; see Operators
```

If you want immutability, simply do not reassign. The language does not
have a `final`/`const` local modifier in this version.

Assignment is a **statement**, not an expression: it yields no value and
cannot be chained. `a = b = c` is a compile error. Compound assignment
(`+=`, `-=`, `*=`, `/=`, `%=`) is likewise a statement; `x += y += 1` is an
error. See [Operators → Compound assignment](../language/operators.md#compound-assignment).

## Scopes

Variables are block-scoped: a declaration lives from its statement to the
end of the enclosing block (`{ ... }` of a function body, `if`/`else`
branch, loop body, or `match` arm block). Shadowing an outer name with a
new declaration in an inner block behaves like in C-family languages; the
[language server](../tooling/lsp.md) is shadowing-aware in its
go-to-definition and reference features.

## `infer` — explicit crystallization

Checkmate **forbids silent declaration inference**. When you want concise
locals, you ask for type crystallization visibly with `infer`:

```checkmate
infer wow = 10.0                    // crystallizes to float
infer name = "Hero"                 // crystallizes to str
infer pos = vec2(x: 10.0, y: 5.0)   // crystallizes to vec2
infer evt = gameEvent.Damage(7)     // crystallizes to gameEvent
infer numbers = [1, 2, 3]           // crystallizes to int[]
infer tally = {"k": 4}              // crystallizes to map<str, int>
infer computed = fib(9)             // crystallizes to the function's return type
```

The rules are strict:

- The initializer must yield an **unambiguous static type**.
- Ambiguous initializers fail compilation:
  ```checkmate
  infer items = []     // error: cannot infer type for 'items'; ambiguous initializer
  ```
  Declare the type instead: `int[] items = []`.
- The crystallized type is fixed forever — `infer` is not dynamic typing.
  After `infer health = 100`, `health` is an `int` exactly as if you had
  written `int health = 100`.

`infer` variables are mutable like any other:

```checkmate
infer health = 100
health += 5              // fine; still int
```

> The whitepaper §2.16.1 describes a future formatter pass,
> `--auto-crystallize`, that rewrites `infer` declarations to their explicit
> types for production review. The formatter is not shipped yet; treat
> `infer` as a source-level convenience today.

## Which form to use

| Situation | Use |
| --- | --- |
| Clarity matters, reviewing, public code | `int health = 100` |
| The type is obvious from an obvious initializer | `infer pos = vec2(...)` |
| Empty collections | Always the explicit form: `int[] xs = []` |

Both forms produce identical programs. The explicit form documents the
type; `infer` documents that the initializer *is* the type story. Neither
ever changes what the compiler checks.
