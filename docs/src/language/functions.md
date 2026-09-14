# Functions

Functions are the executable unit of Checkmate. Top-level functions are
declared at file scope; associated functions live in
[`impl` blocks](../language/impl-blocks.md). There are no nested function
declarations, no closures, and no function *values* — functions are
compile-time entities you call, not data you pass around.

## Declaration

```checkmate
returnType functionName(type param1, type param2) {
    // body: newline-delimited statements
    return value
}
```

- The **return type comes first**, then the name, then typed parameters.
- Parameters are `type name` pairs, comma-separated.
- A function with no parameters still gets its parentheses: `int ping()`.
- `void` functions return nothing; the `return` keyword is optional for
  them (a bare `return` is allowed).

```checkmate
float distance(vec2 a, vec2 b) {
    float dx = b.x - a.x
    float dy = b.y - a.y
    return dx * dx + dy * dy
}

void logMessage(str msg) {
    // return is optional for void
}

void ping() {
    return
}
```

## Calling

Positional arguments in declaration order:

```checkmate
float d = distance(a, b)
```

Or **named** arguments, in any order:

```checkmate
float d = distance(b: target, a: origin)
```

The two styles **never mix in one call**. `getUser(42, name: "Hero")` is a
compile-time error. Named arguments spanning multiple lines drop the
commas — the newline delimits (see
[Significant Newlines](../language/newlines.md)):

```checkmate
movePlayer(
    player: p
    position: position
)
```

Named arguments are checked for **completeness and exactness**: every
parameter must be supplied exactly once, and no unknown names are accepted.
For struct construction the same named-argument shape applies — see
[Structs](../language/structs.md).

## Returning

Only `return` returns a value. There is no implicit tail expression, no
`?`-less fallthrough:

```checkmate
int expect(bool ok) {
    if (ok) {
        return 0
    }
    return 1        // every path must return on a non-void function
}
```

The checker enforces that a non-`void` function returns on every control
path. `void` functions may `return` early with no value.

## Recursion

Recursion works and is bounded: the interpreter enforces a call-depth limit
(default **1024**, host-configurable per
[§5.5](../appendix/limits-and-errors.md)). Runaway recursion terminates the
invocation with a clean `CallDepth` error — never a native stack overflow.

```checkmate
int fib(int n) {
    if (n <= 1) {
        return n
    }
    return fib(n - 1) + fib(n - 2)
}
```

## Value semantics for parameters

Parameters receive **copies**. Mutating a parameter never affects the
caller's instance:

```checkmate
player damage(player p, int amount) {
    p.health -= amount
    if (p.health <= 0) {
        p.alive = false
    }
    return p
}

// caller:
player hurt = damage(hero, 60)   // hero is unchanged
hero = damage(hero, 60)          // reassign to apply the update
```

This is the language's defining discipline — see
[Value Semantics](../language/value-semantics.md).

## Entry points: there is no `main` (in the language)

§2.1 is deliberate: Checkmate has **no implicit global entry point**. The
*host* targets specific functions by name. The CLI's `cme run` follows the
convention of invoking the function named `main` (taking no arguments,
returning any type; its value prints in CMON form unless it is `void`),
and in a [mod](../mods/mods.md) `main` may live in any module. Embedded hosts
call whatever they want — `context.invoke("fib", &[...])` — and invoke
[`impl` members](../language/impl-blocks.md) by target path.

## Functions and visibility

All top-level functions in a file (or a mod's linked module set) are
visible to each other, including forward references — declaration order
does not matter for calls. Capitalization follows the
[boundary convention](../language/lexical-structure.md#boundary-capitalization):
script-internal functions are camelCase; when a schema is active,
PascalCase names belong to the contract.

## What functions do not have (yet)

- **Closures / lambdas** — not in the language. Pass data, not code; model
  variation with enums and `match`.
- **Overloading** — one name, one signature.
- **Default arguments** — call-site named arguments cover most of the same
  ground.
- **Generics on functions** — generic *types* exist ([Types](../language/types.md));
  functions are monomorphic today.
