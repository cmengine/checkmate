# Enums

Enums are Checkmate's algebraic data types: a closed set of **variants**,
each optionally carrying typed payloads. Together with `match` they are
the language's tool for modeling "one of".

## Declaration

```checkmate
enum gameEvent {
    Damage(int amount)
    Heal(int amount)
    Spawn(str enemyKind, vec2 position)
    PlayerDied()
}
```

- Variants are newline-delimited inside the braces.
- A variant may carry zero or more typed payload fields:
  `Damage(int amount)` has one payload field named `amount` of type `int`;
  `PlayerDied()` carries nothing.
- **Variant names are PascalCase** payload identifiers (`Damage`, `Some`,
  `Ok`); their payload field names are camelCase. This convention holds
  regardless of schemas.
- Payload field lists are comma-separated within the variant's parentheses.

## Construction

Variant constructors are **fully qualified** with their enum type:

```checkmate
gameEvent evt = gameEvent.Damage(25)
gameEvent spawn = gameEvent.Spawn("goblin", vec2(x: 2.0, y: 3.0))
```

- Payload arguments are positional, in declaration order.
- Payload-free variants still take their parentheses: `gameEvent.PlayerDied()`.
- Construction is an ordinary expression; nested generics crystallize from
  the payload:

  ```checkmate
  enum maybe<T> {
      Just(T value)
      Nothing()
  }

  maybe<vec2> spot = maybe.Just(vec2(x: 8.0, y: 9.0))
  maybe<int> absent = maybe.Nothing()   // T pinned by the declared type
  ```

## Destructuring with `match`

See [Pattern Matching](../language/match.md) for the full chapter; the enum
essentials:

```checkmate
str describe(gameEvent evt) {
    return match (evt) {
        Damage(int amount) => "damage:" + amount
        Spawn(str enemyKind, vec2 position) => $"spawn:{enemyKind}"
        PlayerDied() => "died"
        _ => "other"
    }
}
```

Patterns declare bindings with their types; the wildcard `_` covers the
rest; exhaustiveness is enforced.

## Structural equality

Enum values compare structurally — same type, same variant, equal
payloads:

```checkmate
gameEvent.Damage(5) == gameEvent.Damage(5)     // true
gameEvent.Damage(5) != gameEvent.Damage(6)     // true
```

## The built-in enums: `option` and `result`

Two generic enums are built into the checker and available everywhere:

```text
option<T>  = Some(T value) | None()
result<T, E> = Ok(T value) | Err(E error)
```

They construct bare (`Some(v)`, `None()`, `Ok(x)`, `Err(e)`) or qualified
(`option.Some(v)`, `result.Ok(x)`), match like any enum, and `result` adds
the [`?` operator](../language/error-handling.md). They are *the* null-safety
and error-handling story of the language — there is no `null`.

## `impl` on enums

Enums can carry [`impl` blocks](../language/impl-blocks.md) alongside their
variants — associated functions with an explicit receiver parameter:

```checkmate
enum suit {
    Clubs()
    Diamonds()
    Hearts()
    Spades()
}

impl suit {
    str label(suit s) {
        return match (s) {
            Clubs() => "clubs"
            Diamonds() => "diamonds"
            Hearts() => "hearts"
            Spades() => "spades"
        }
    }
}

// call by qualified path:
str name = suit.label(suit.Hearts())
```

## When to reach for an enum

- **Closed sets of outcomes**: `gameEvent`, parse results, AI states.
- **"Missing" values**: `option<T>` instead of sentinel values.
- **"Failed" values**: `result<T, E>` instead of exceptions.
- **Data with exactly-one-of payloads**: the variant payload *is* the
  state, and `match` forces every consumer to handle every case.

For modeling "several optional capabilities at once", prefer a struct of
`option` fields or split enums — enums are a sum type, not a bag of flags.
