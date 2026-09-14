# Coming from TypeScript

TypeScript gives you static types over JavaScript's runtime. Checkmate
gives you static types over *nothing* — there is no JS-like dynamic layer
underneath. If you liked TS's strict mode, enums, discriminated unions,
and exhaustiveness checking, you will feel at home. If you rely on the
flexibility underneath, this page shows what replaced it.

## The two-minute syntax map

| TypeScript | Checkmate |
| --- | --- |
| `function add(a: number, b: number): number` | `int add(int a, int b)` |
| `let x = 1;` | `int x = 1` / `infer x = 1` |
| `const x = 1;` | `int x = 1` — immutability is discipline, not a keyword |
| `if (x > 0) { }` | identical |
| `for (const v of xs) { }` | `for (int v in xs)` |
| `while (cond) { }` | identical |
| `interface` / `type` (object shapes) | `struct` |
| discriminated unions | `enum` with payloads |
| `switch` + `never` exhaustiveness | `match` — exhaustiveness guaranteed |
| `T \| null` / `undefined` | `option<T>` |
| `throw` / `try`/`catch` | `result<T, E>` + `?` — no exceptions |
| `async`/`await`/`Promise` | synchronous style; host owns concurrency |
| `Array<T>` / `T[]` | `T[]` (postfix) |
| `Map<K, V>` | `map<K, V>` |
| `class` with methods | `struct` + `impl` |
| template literals | `$"..."` interpolation |
| object literals `{ gold: 120 }` | struct construction `loot(gold: 120)` / map literals |
| `===` | `==` (strict by type system; no coercions exist) |
| union types `A \| B` | enums (closed, nominal) |
| generics on functions | generics on structs/enums only |
| `any` / `unknown` | nothing — every type is known |
| modules, npm | [m[Mods](../mods/mods.md), no package manager |

## `number` splits into `int` and `float`

There is no single `number`. `int` is a 64-bit signed integer; `float`
is IEEE 754 double. They **never mix implicitly**:

```typescript
// TS: 1 + 2.5 === 3.5
```

```checkmate
// Checkmate: 1 + 2.5 is a compile error
int a = 1
float b = 2.5
// a + b          // error: cross-type arithmetic
// an explicit cast operator does not exist yet (see Status page)
```

Integer division truncates toward zero (`7 / 2 == 3`), division by zero
and overflow terminate the invocation — no `NaN`/`Infinity` creeping into
integer logic, no silent wraparound.

## No `null`, no `undefined` — `option<T>`

```typescript
// TS
function findEven(xs: number[]): number | undefined { ... }
const v = findEven(xs) ?? -1;
```

```checkmate
// Checkmate
option<int> findEven(int[] values) {
    for (int v in values) {
        if (v % 2 == 0) {
            return Some(v)
        }
    }
    return None()
}

// unpacking is `match` (no ?? chaining, no optional chaining):
int outcome = -1
match (findEven(values)) {
    Some(int v) => { outcome = v }
    None() => {}
}
```

`option<T>` is a real enum: `match` is the only door, and exhaustiveness
means the `None` case is structurally impossible to forget. There is no
optional chaining (`?.`), no nullish coalescing (`??`), no truthiness —
conditions are `bool`, period.

## Exceptions → `result<T, E>`

```typescript
// TS
try {
    const total = safeDiv(a, b);
} catch (e) {
    // e is unknown...
}
```

```checkmate
// Checkmate
result<int, str> safeDiv(int a, int b) {
    if (b == 0) {
        return Err("division by zero")
    }
    return Ok(a / b)
}

result<int, str> chain(int a, int b, int c) {
    int first = safeDiv(a, b)?       // like `throw`, but a checked value
    int second = safeDiv(first, c)?
    return Ok(first + second)
}
```

The error type is part of the signature and must match exactly through
`?` — the "unknown" in your `catch (e)` becomes a concrete `E` the
compiler tracks. No `try`/`catch`/`finally` exist; unexpected runtime
failures (overflow, bad index, missing key) terminate the invocation and
land in the host's lap as a positioned error.

## Discriminated unions → enums

TS's `type Event = { kind: "damage", amount: number } | ...` pattern is
what Checkmate's enums *are*, natively:

```typescript
// TS
type GameEvent =
  | { kind: "Damage"; amount: number }
  | { kind: "Spawn"; enemyKind: string; position: Vec2 }
  | { kind: "PlayerDied" };

function describe(e: GameEvent): string {
  switch (e.kind) {
    case "Damage": return `damage:${e.amount}`;
    case "Spawn": return `spawn:${e.enemyKind}`;
    case "PlayerDied": return "died";
  }
}
```

```checkmate
// Checkmate
enum gameEvent {
    Damage(int amount)
    Spawn(str enemyKind, vec2 position)
    PlayerDied()
}

str describe(gameEvent evt) {
    return match (evt) {
        Damage(int amount) => $"damage:{amount}"
        Spawn(str enemyKind, vec2 position) => $"spawn:{enemyKind}"
        PlayerDied() => "died"
    }
}
```

Differences: variants are **nominal** (`gameEvent.Damage(25)`, not an
object shape), the tag and payload are one declaration, and forgetting a
variant is a compile error *without* needing the `never` trick.

## Objects → structs; references → values

TS objects are references; Checkmate structs are values:

```typescript
// TS: mutation through the reference is visible to the caller
function damage(p: Player) { p.health -= 10; }
```

```checkmate
// Checkmate: p is a copy; return and reassign instead
player damage(player p, int amount) {
    p.health -= amount
    return p
}
hero = damage(hero, 60)
```

No `readonly` gymnastics needed — aliasing does not exist, so "someone
else mutated my object" is structurally impossible. Record-style updates
(`{ ...p, health: newHealth }`) become "construct/return the new value"
and reassign.

## async/await — gone, deliberately

No `Promise`, no `async`, no event loop, no microtask queue. A script
invocation is a synchronous, bounded computation. Long-running or
environmental work (HTTP, files, timers) is a **capability call** into
the host; the planned continuation-splitting VM will make those calls
yield transparently — still with zero `async` syntax in your source. If
your TS brain reaches for `Promise.all`, hand the batch to a host
capability that parallelizes internally.

## Modules without npm

A TS project maps to a [mod](../mods/mods.md): `mod.toml` + `src/**.cm`,
imports written `import self.ui.hud`. There is no package manager and no
node_modules — and no `any`-typed escape hatch anywhere. Types come from
[structs, enums, generics](../language/types.md), and, at the host
boundary, from [schemas](../schema/overview.md) — which also generate
TS-flavored equivalents for hosts (typed proxies in Rust, C headers for
C hosts).

## Syntax quirks to unlearn

1. **No semicolons**; newlines end statements. Multi-line expressions
   continue only with a trailing operator or inside parentheses.
2. **Conditions need parentheses**; bodies need braces — always.
3. **`==` requires same types**; there are no coercions to `==` away.
   `"1" == 1` does not compile.
4. **Mixing `&&` with `||` needs parentheses**, and comparisons never
   chain (`a < b < c` is an error) — see
   [Mandatory Parenthesization](../language/operators.md#two-mandatory-parenthesization-rules).
5. **No arrow functions, no closures, no higher-order functions.** Pass
   data; model variation with enums; write plain functions.
6. **String `+` stringifies** the other side: `"HP: " + 100` works;
   everything else type-checks strictly.

## A tiny side-by-side

```typescript
// TS
function fib(n: number): number {
  if (n <= 1) return n;
  return fib(n - 1) + fib(n - 2);
}
console.log(`fib(10) = ${fib(10)}`);
```

```checkmate
// Checkmate
int fib(int n) {
    if (n <= 1) {
        return n
    }
    return fib(n - 1) + fib(n - 2)
}

str main() {
    return $"fib(10) = {fib(10)}"
}
```

`cme run file.cm` prints `fib(10) = 55`.

## Where to go next

- [Types](../language/types.md) and [Pattern Matching](../language/match.md).
- [The Schema System](../schema/overview.md) — typed contracts at the
  boundary, the "API layer" of an embedded language.
- Building a host in Rust or C? [Embedding Overview](../embedding/overview.md).
