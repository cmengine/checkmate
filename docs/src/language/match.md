# Pattern Matching

`match` performs exhaustive destructuring on enum values (algebraic data
types). It works in **two positions**:

- as an **expression** — every arm yields a value and the whole `match`
  produces one;
- as a **statement** — every arm is a block of statements.

## Expression form

```checkmate
str label = match (event) {
    Damage(int amount) => "Damage"
    Heal(int amount) => "Heal"
    PlayerDied() => "Dead"
    _ => "Unknown"
}
```

- The scrutinee is parenthesized: `match (expr) { ... }`.
- Each arm is `pattern => expression` (or `pattern => { statements }`).
- Arms are newline-delimited — no commas between them.

## Statement form

```checkmate
match (event) {
    Damage(int amount) => {
        health -= amount
    }
    PlayerDied() => {
        alive = false
    }
    _ => {}
}
```

A statement-form arm whose body is a single `return` may use braces with
the return inside:

```checkmate
int classifyEvent(gameEvent evt) {
    match (evt) {
        Damage(int amount) => { return 1 }
        Heal(int amount) => { return 2 }
        Spawn(str enemyKind, vec2 position) => { return 3 }
        PlayerDied() => { return 4 }
    }
}
```

## Patterns

**Variant patterns** name the variant and declare bindings for each payload
field, **with their types**:

```checkmate
gameEvent evt = gameEvent.Spawn("goblin", vec2(x: 2.0, y: 3.0))

match (evt) {
    Spawn(str enemyKind, vec2 position) => { /* enemyKind: str, position: vec2 */ }
    _ => {}
}
```

- The binding names are yours; the types must match the variant's declared
  payload types.
- Payload-free variants match with empty parentheses: `PlayerDied()`,
  `None()`, `Clubs()`.
- **The wildcard `_`** matches anything and binds nothing. Use it for a
  catch-all arm or to ignore a specific payload:

  ```checkmate
  match (evt) {
      Damage(_) => { /* the amount does not matter */ }
      _ => {}
  }
  ```

Payload fields have camelCase names (declared in the enum); the pattern
binds them positionally in declaration order.

## Exhaustiveness

`match` must be **exhaustive**. Either every variant of the enum has an
arm, or a wildcard `_` covers the remainder — omit one variant without a
wildcard and compilation fails with an exact diagnostic. Adding a new
variant to the enum therefore turns every non-exhaustive `match` in the
codebase red until it is handled: a compile-time guarantee, not a lint.

```checkmate
// exhaustive without a wildcard:
str label = match (evt) {
    Damage(int amount) => "damage"
    Heal(int amount) => "heal"
    Spawn(str enemyKind, vec2 position) => "spawn"
    PlayerDied() => "died"
}
```

## `match` on `option` and `result`

`option`/`result` are ordinary enums, so `match` is the core way to unpack
them:

```checkmate
option<int> found = findEven(values)
match (found) {
    Some(int v) => { outcome = v }
    None() => {}
}

result<int, str> outcome2 = safeDiv(a, b)
match (outcome2) {
    Ok(int total) => { total }
    Err(str reason) => { /* handle */ }
}
```

For the `result` case specifically, the [`?` operator](../language/error-handling.md)
covers the common "propagate the error, bind the success" flow.

## Nesting and composition

The scrutinee is any expression — field access, index access, call results:

```checkmate
match (entry.event) {
    Heal(int amount) => { failures += expect(amount == 9) }
    _ => {}
}
```

Match arms are blocks, so they nest freely, and arms may bind values used
in further expressions. Because [structs and enums are compared
structurally](../language/types.md#equality-and-comparison), `match` plus
value-semantic enums give you the full data-modeling toolkit: model
variation with enums, dispatch with `match`, and let the compiler keep it
exhaustive.
