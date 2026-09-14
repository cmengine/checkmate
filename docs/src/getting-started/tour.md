# A Ten-Minute Tour

This chapter sweeps the whole language in one file. Every snippet is real
Checkmate — the tour is a condensed walk of the same ground the
[`syntax.cm`](https://github.com/cmengine/checkmate/blob/mom/syntax.cm)
fixture covers.

## Variables and `infer`

Declarations put the type first. Variables are mutable by default.

```checkmate
int score = 0
float speed = 4.5
str title = "Checkmate"
bool active = true

score = score + 10
speed *= 2.0
```

When you want concise locals, ask for type crystallization *explicitly*
with `infer` — silent inference does not exist:

```checkmate
infer pos = vec2(x: 10.0, y: 5.0)   // crystallizes to vec2
infer computed = fib(9)             // crystallizes to int
```

`infer` fails to compile on ambiguous initializers (an empty collection,
for example). See [Variables](../language/variables.md).

## Control flow

Parenthesized conditions, braced bodies, `else if` chains:

```checkmate
if (health <= 0) {
    alive = false
} else if (health < 25) {
    alive = true
} else {
    alive = true
}

while (health > 0) {
    health -= 1
}

for (int v in [10, 20, 30]) {
    total += v
}
```

## Structs and enums

Records with named fields; tagged unions with typed payloads. Fields are
newline-delimited, constructors are named, enum constructors are qualified
by their type:

```checkmate
struct vec2 {
    float x
    float y
}

enum gameEvent {
    Damage(int amount)
    Heal(int amount)
    Spawn(str enemyKind, vec2 position)
    PlayerDied()
}

vec2 pos = vec2(x: 10.0, y: 5.0)
gameEvent evt = gameEvent.Spawn("goblin", pos)
```

## `match`

Exhaustive destructuring on enums, in expression or statement form. The
wildcard `_` covers the rest:

```checkmate
str label = match (evt) {
    Damage(int amount) => "damage:" + amount
    Heal(int amount) => "heal:" + amount
    Spawn(str enemyKind, vec2 position) => $"spawn:{enemyKind}"
    PlayerDied() => "died"
}
```

Leave one variant out without a wildcard and the compiler rejects the
`match` — exhaustiveness is a compile-time guarantee, not a lint.

## `option` and `result`

Null-safety and error handling are ordinary generic enums the language
ships, with bare constructors and the `?` propagation operator:

```checkmate
option<int> findEven(int[] values) {
    for (int v in values) {
        if (v % 2 == 0) {
            return Some(v)
        }
    }
    return None()
}

result<int, str> safeDiv(int a, int b) {
    if (b == 0) {
        return Err("division by zero")
    }
    return Ok(a / b)
}

result<int, str> chain(int a, int b, int c) {
    int first = safeDiv(a, b)?      // bail out early on Err
    int second = safeDiv(first, c)?
    return Ok(first + second)
}
```

## Collections

Arrays are `T[]`, maps are `map<K, V>`. Literals may be comma- or
newline-delimited; indexing reads, writes, and inserts:

```checkmate
int[] evens = [2, 4, 6, 8]
evens.length               // 4

map<str, int> loot = {
    "gold": 120
    "gems": 3
}
loot["gold"] += 30         // compound index assignment
loot["arrows"] = 60        // insert-by-index
```

## `impl` blocks

Associated functions live in `impl` blocks, take an explicit receiver
parameter, and are called by qualified path:

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

counter c = counter(value: 41)
counter bigger = counter.bump(c)    // bigger.value == 42
// c.value is still 41 — value semantics
```

Blocks for the same target union across the whole file (and across a whole
mod), with duplicate members rejected.

## Strings

Immutable UTF-8 text. `+` concatenates, stringifying the other side
deterministically; `$"..."` interpolates:

```checkmate
str greeting = "HP: " + 100          // "HP: 100"
str report = $"score={score} pos=({pos.x},{pos.y})"
```

## Operators you should know about

Full table in [Operators](../language/operators.md); the surprises:

- **No implicit numeric coercions.** `int` and `float` never mix silently;
  `1 == "1"` is a type error, not `true`.
- **Chained comparisons and mixed `&&`/`||` are compile errors** unless you
  parenthesize. `a < b < c` is rejected outright.
- **Integer division truncates toward zero** and `a % b` takes the sign of
  `a`. Division by zero and integer overflow terminate the invocation.
- **No `++`, no ternary, no bitwise operators** in this version — see
  [Deliberately absent](../language/operators.md#deliberately-absent).

## Newlines matter

A newline ends a statement. Expressions continue across lines when the line
ends with an operator or you are inside parentheses:

```checkmate
int total = base +
    bonus              // ok: trailing operator continues

int total2 = base
    + bonus            // error: leading binary operator
```

Inside `( ... )` newlines are free; inside `[ ... ]` and `{ ... }` they are
significant — they separate collection elements and match arms. Details in
[Significant Newlines](../language/newlines.md).

## Comments

```checkmate
// line comment

/* block comment,
   /* nested not supported — ends at the first */ */
```

Block comments end at the **first** `*/`; an unterminated one swallows the
rest of the file with a single clean error.

## What the tour skipped

- [Imports](../language/imports.md) — host capability namespaces and `self.*`
  module paths.
- [Mods](../mods/mods.md) — multi-file programs with `mod.toml`.
- [Megaprogramming](../mega/overview.md) — `grammar`/`mega` declarations that
  embed entire languages (JSON, YAML, HTML, regex, …) in Checkmate source.
- [The Schema System](../schema/overview.md) — how hosts and scripts agree on
  contracts, versions, and capability gating.

Run `cargo run --features cli -- run syntax.cm` in the repository to watch
all of the above execute with `failures=0`.
