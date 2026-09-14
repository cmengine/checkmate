# Coming from C#

Checkmate reads like a cousin of C#: braced blocks, typed signatures,
`struct`s, `enum`s. But it is closer to "C# as a pure, sandboxed expression
layer over a host" than to the full framework — no classes, no interfaces,
no LINQ, no tasks. Here is the map.

## The two-minute syntax map

| C# | Checkmate |
| --- | --- |
| `int Add(int a, int b)` | same shape, but standalone or in `impl`, never in a class |
| `var x = 1;` | `int x = 1` / `infer x = 1` — no semicolon |
| `x++;` / `x += 1;` | `x += 1` — no `++` |
| `if (x > 0) { }` | identical (condition required) |
| `foreach (var v in xs)` | `for (int v in xs)` |
| `while (cond) { }` | identical |
| `class` / `record` | `struct` — value semantics, no inheritance |
| `enum` with methods | `enum` (ADT with payloads) + `impl` |
| `switch` expressions | `match` — exhaustive, destructuring |
| `int?` / nullable reference types | `option<T>` |
| exceptions (`try`/`catch`) | `result<T, E>` + `?` — no exceptions |
| `Task`/`async`/`await` | nothing — synchronous style; host owns concurrency |
| LINQ | `for` loops + your own functions |
| `interface` | schema `interface` — a host contract, not an OOP abstraction |
| `static` classes / top-level program | no statics, no globals; host picks the entry point |
| properties | plain fields |
| `out`/`ref` | nothing — return values (value semantics) |

## Value semantics, always

C# programmers constantly ask "is this a reference or a value?". In
Checkmate the answer is always *value*:

```csharp
// C#: class instances alias; this mutates the caller's object
void Damage(Player p) { p.Health -= 10; }
```

```checkmate
// Checkmate: the parameter is a copy; mutation is local
void damage(player p) {
    p.health -= 10            // caller never sees this
}
// the update pattern:
p = damage(p)                   // damage returns the new player
```

Assignment, parameters, returns, struct fields — everything clones
(cheaply, via copy-on-write). There is no `ref`, no `out`, no boxing.

## From classes to structs + impl

A C# class becomes a Checkmate `struct` plus an
[`impl` block](../language/impl-blocks.md):

```csharp
// C#
public class Counter {
    public int Value;
    public Counter(int value) { Value = value; }
    public Counter Bump() { Value++; return this; }
}
```

```checkmate
// Checkmate
struct counter {
    int value
}

impl counter {
    counter bump(counter c) {
        c.value += 1
        return c
    }
}

counter bigger = counter.bump(counter(value: 41))
```

No constructors — construction is named-field syntax checked for
completeness. No inheritance, no virtual dispatch, no `this`. If you used
C# records + `with` expressions, Checkmate's "return the modified copy"
pattern will feel natural.

## Enums are algebraic data types

C# enums are named integers. Checkmate enums are **sum types with
payloads** — closer to a discriminated union than to C#:

```checkmate
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

The compiler enforces exhaustiveness: add a variant and every `match`
without a wildcard stops compiling. That is the safety net `switch`
expressions approximate; here it is a hard guarantee.

## Exceptions → `result<T, E>`

```csharp
// C#
try {
    var user = LoadUser(id);
} catch (HttpException e) {
    // handle
}
```

```checkmate
// Checkmate
result<user, httpError> loadUser(int id) {
    // ...
    return Ok(parsed)
}

result<int, str> caller() {
    user u = loadUser(42)?      // bail out on Err, bind on Ok
    return Ok(u.score)
}
```

There is no `throw`, no stack unwinding, no `finally`. Expected failures
are values; *unexpected* failures (overflow, division by zero, out of
bounds) terminate the invocation with a clean host-visible error — there
is no catching them, by design.

`NullReferenceException` is structurally impossible: there is no `null`.
`int?` is `option<int>`, and `match` is your `?.`/`??`.

## async/await is gone (on purpose)

Checkmate scripts are written in a strictly synchronous style. When a
host operation may take time, the *host* mediates it: you call a schema
capability, and (in the current implementation) the call is synchronous;
the planned continuation-splitting VM will let those calls yield to the
host's scheduler transparently — still no `async`/`await` keywords in
your code. Concurrency between invocations is the host's business; your
script is a pure, race-free computation.

## LINQ → loops

There is no LINQ, no iterator methods, no closures. The idioms are plain:

```csharp
// C#
var total = values.Where(v => v > 0).Sum();
```

```checkmate
// Checkmate
int total = 0
for (int v in values) {
    if (v > 0) {
        total += v
    }
}
```

Verbose? Slightly. Traceable, bounds-checked, allocation-predictable, and
LLM-friendly — that is the trade the language makes everywhere.

## Projects → mods

A C# project's `.csproj` is a mod's `mod.toml`; `src/**.cs` is
`src/**.cm` imported as `self.*` paths. NuGet has no counterpart: no
package manager, and mods cannot import sibling mods — the host mediates
any inter-mod communication. `internal`/`public` visibility does not
exist; a mod is one linked program.

## The type system's hard edges

- No implicit numeric conversions: `int` + `float` is a compile error.
  Write your conversions explicitly (an explicit cast operator is not in
  the language yet — see [status](../status.md)).
- `==` requires same types and compares **structurally** (like C# records
  with value equality, everywhere).
- Strings are immutable UTF-8; there is no `StringBuilder` yet — build
  with `$"..."` interpolation and `+`.

## A tiny side-by-side

```csharp
// C#
int Fib(int n) => n <= 1 ? n : Fib(n - 1) + Fib(n - 2);

string Greet(string name, int health) => $"hero={name} hp={health}";
```

```checkmate
// Checkmate — no ternary, no expression-bodied members:
int fib(int n) {
    if (n <= 1) {
        return n
    }
    return fib(n - 1) + fib(n - 2)
}

str greet(str name, int health) {
    return $"hero={name} hp={health}"
}
```

## Where to go next

- [Pattern Matching](../language/match.md) — your new `switch`.
- [The Schema System](../schema/overview.md) — the "interface" between
  host and scripts, with generated C#/Rust-style bindings for hosts.
- [Embedding in Rust](../embedding/rust.md) — if you host Checkmate in a
  .NET-style native host via the C ABI, see [Embedding in C](../embedding/c.md).
