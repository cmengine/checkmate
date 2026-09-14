# Structs

Structs are Checkmate's record type: named fields, fixed layout, value
semantics.

## Declaration

```checkmate
struct vec2 {
    float x
    float y
}

struct player {
    str name
    vec2 position
    int health
    bool alive
}
```

- Fields are **newline-delimited** — no commas, no semicolons. On a single
  line inside braces, commas are accepted (`{ float x, float y }` is not
  the style; see construction below for where commas show up).
- Field types are any [type](../language/types.md): scalars, other structs,
  enums, generics, arrays (`int[]`), maps (`map<str, int>`).
- Field names are camelCase (see
  [Boundary Capitalization](../language/lexical-structure.md#boundary-capitalization)).
- There are no methods *inside* the struct — behavior lives in
  [`impl` blocks](../language/impl-blocks.md).
- There are no field defaults, no visibility modifiers, no `static`
  fields, no `nullable` fields — every field is present and initialized at
  construction.

## Construction

Construction uses **named arguments** — every field, spelled by name:

```checkmate
vec2 pos = vec2(x: 10.0, y: 5.0)

player p = player(
    name: "Hero"
    position: pos
    health: 100
    alive: true
)
```

- Newline-delimited form (above) or one-line comma form:

  ```checkmate
  vec2 flat = vec2(x: 10.0, y: 5.0)
  player hero2 = player(name: "Hero", position: vec2(x: 12.5, y: 5.0), health: 90, alive: true)
  ```

- The construction is checked for **completeness**: omit a field, supply an
  unknown one, or pass the wrong type and compilation fails.
- Field order in the literal does not matter.
- Construction is an expression: nest it, return it, pass it.

## Field access and assignment

Fields chain through any depth of nesting:

```checkmate
p.health = 90
p.position.x = 12.5
squad.scores[1] += 5
spawns["orc"].x = 40.0
```

Assignments to fields, elements, and map entries are ordinary
[assignment statements](../language/variables.md); compound assignment works
on them too.

## Equality

Struct equality is **structural** and requires the same type name:

```checkmate
vec2 a = vec2(x: 10.0, y: 5.0)
vec2 b = vec2(x: 10.0, y: 5.0)
// a == b is true
```

Two structs are equal when their type names match and every field is
equal (recursively — nested structs, arrays, maps compare structurally;
floats compare per IEEE 754).

## Generic structs

Structs take type parameters in angle brackets after the name:

```checkmate
struct pair<A, B> {
    A first
    B second
}

pair<int, str> pr = pair(first: 7, second: "seven")
pair<int, pair<str, bool>> nested = pair(first: 1, second: pair(first: "x", second: true))
```

Generic arguments crystallize from construction; nested generics close
with separate `>` tokens.

## Value semantics

The defining rule: assignment and parameter passing **clone**. Mutating a
struct — your own local or a parameter's copy — never leaks anywhere else:

```checkmate
player damage(player p, int amount) {
    p.health -= amount
    return p
}

player hurt = damage(hero, 60)   // hero.health unchanged
squad.leader.position.x = 40.0   // squad.leader is a copy of hero;
                                 // hero.position.x is unchanged
```

The implementation shares buffers behind the scenes (ARC + copy-on-write),
so clones of unmutated data stay cheap — but the *semantics* are always
"independent owned value". Details in
[Value Semantics](../language/value-semantics.md).

## Structs at the boundary

When a [schema](../schema/overview.md) is active, structs declared **in the
schema** are the shared interchange types across the host/script boundary —
PascalCase, non-generic, and mapped to native Rust structs or C pack/unpack
helpers by the generated bindings. Script-internal structs (camelCase)
never cross the boundary; only
[values](../embedding/overview.md#values-across-the-boundary) do.
