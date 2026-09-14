# Types

Checkmate is statically typed: every expression has a type known at compile
time, and the type checker rejects anything that does not line up. There
are no implicit conversions, no `any`, no unions-by-inference.

## The five scalar types

| Type | Values | Notes |
| --- | --- | --- |
| `int` | Signed 64-bit integers | Arithmetic is overflow-checked; overflow terminates the invocation |
| `float` | IEEE 754 double-precision | Division follows IEEE 754; `NaN != NaN` |
| `bool` | `true`, `false` | The only operand type of `&&`, `||`, `!` |
| `str` | Immutable UTF-8 text | Compared by content; concatenation via `+` |
| `void` | The absence of a value | Return type only; a `void` call is a statement |

Numeric conversions between `int` and `float` are **strictly explicit in
intent** — implicit coercions are disallowed, including through operators.
`1 + 2.0` is a compile error, not `3.0`. (A general cast/conversion
operator is not part of the language yet.)

## Structs

A struct is a product type with named, typed fields:

```checkmate
struct player {
    str name
    vec2 position
    int health
    bool alive
}
```

Fields are newline-delimited — no commas or semicolons. Construction uses
named arguments; see [Structs](../language/structs.md).

## Enums

Enums are tagged unions where variants may carry typed payloads — full
algebraic data types:

```checkmate
enum gameEvent {
    Damage(int amount)
    Heal(int amount)
    Spawn(str enemyKind, vec2 position)
    PlayerDied()
}
```

Constructors are qualified by the enum type: `gameEvent.Damage(25)`. See
[Enums](../language/enums.md).

## Generics

Structs and enums support type parameters:

```checkmate
struct pair<A, B> {
    A first
    B second
}

enum maybe<T> {
    Just(T value)
    Nothing()
}

pair<int, str> labeled = pair(first: 7, second: "seven")
maybe<vec2> spot = maybe.Just(vec2(x: 8.0, y: 9.0))
```

Generic arguments nest freely — `pair<int, pair<str, bool>>` closes with
two separate `>` tokens; there is no `>>` shift token to disambiguate.

## The built-in control enums

Two generic enums are built into the type checker and used pervasively:

```text
option<T> = Some(T value) | None()
result<T, E> = Ok(T value) | Err(E error)
```

They behave exactly like user-declared enums (constructors, `match`
patterns), plus the `?` early-return operator for `result`. See
[Error Handling](../language/error-handling.md).

## Arrays and maps

Compound collection types use postfix `[]` for arrays and `map<K, V>` for
keyed maps:

```checkmate
int[] scores = [90, 85, 77]
int[][] grid = [[1, 2], [3, 4]]
map<str, int> loot = { "gold": 120 }
map<int, str> names = { 1: "one" }
```

Arrays expose `.length`; both are indexed with `[]`; both follow value
semantics (assignment clones). See
[Collections](../language/collections.md).

## Type names and capitalization

Type names follow the [boundary convention](../language/lexical-structure.md#boundary-capitalization):
script-internal types are camelCase (`vec2`, `gameEvent`); schema-declared
boundary types are PascalCase (`TextureHandle`). When a schema is active,
the checker enforces the split against the contract.

## Equality and comparison

- `==` and `!=` require both operands to be **the same type** —
  cross-type equality is a type error, never a coercion.
- Equality is **structural**: structs compare by type name and fields,
  enums by variant and payloads, arrays element-wise, maps
  order-insensitively by key/value pairs, strings by content.
- `float` equality follows IEEE 754: `NaN == NaN` is `false`.
- Ordering comparisons (`<`, `<=`, `>`, `>=`) exist only for `int` and
  `float` pairs.

```checkmate
failures += expect([1, 2] == [1, 2])           // true
failures += expect(gameEvent.Damage(5) == gameEvent.Damage(5))   // true
failures += expect(vec2(x: 1.0, y: 2.0) == vec2(x: 1.0, y: 2.0)) // true
```

## What a declaration looks like

Everything above composes into declarations like:

```checkmate
struct party {
    str name
    int[] scores
    player leader
}

int total(party p) {
    int sum = 0
    for (int s in p.scores) {
        sum += s
    }
    return sum
}
```

The type system has no subtyping, no interfaces-in-the-OOP-sense, no trait
objects, and no dynamic dispatch *within* script code. Polymorphism comes
from generics (compile-time) and from enums (runtime, via `match`). The
[`impl`](../language/impl-blocks.md) system adds associated functions to
types; it is not inheritance.
