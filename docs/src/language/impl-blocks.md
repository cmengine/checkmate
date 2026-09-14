# `impl` Blocks

`impl` blocks attach **associated functions** to a target — a local struct,
a local enum, or a host-style dotted path. They are Checkmate's way of
grouping behavior with a type, and the mechanism mods use to satisfy host
interfaces ([§10.4](../mods/impl-union.md)).

## The shape

```checkmate
struct counter {
    int value
}

impl counter {
    int peek(counter c) {
        return c.value
    }

    counter reset() {
        return counter(value: 0)
    }
}
```

Key facts:

- Members are **plain function declarations** — the same
  [`Type name(params)` shape](../language/functions.md) as top-level
  functions, nested under the target's path.
- There is **no implicit `self`/`this`**. The receiver is an explicit,
  ordinary first parameter (`counter c`). Value semantics apply to it like
  any parameter.
- A member with no receiver (`reset`) acts as a constructor-style
  associated function.
- Return types are mandatory (except `void`), exactly like top-level
  functions.

## Calling members

Members are called by **qualified path** — `target.member(...)` — passing
the receiver explicitly:

```checkmate
counter c = counter(value: 41)

counter.peek(c)          // 41
counter.reset()          // counter(value: 0)
```

There is no method-call syntax (`c.peek()` does not exist). The qualified
form makes every call site visually explicit about which target's member
runs — consistent with the language's "visible boundary" philosophy.

Member calls resolve inside member bodies too, including across `impl`
blocks, with forward references working like top-level functions:

```checkmate
impl counter {
    int peekTwice(counter c) {
        return counter.peek(c) + counter.peek(c)
    }
}
```

## Multiple blocks union

A target may have several `impl` blocks, in the same file or spread across
a whole [mod](../mods/impl-union.md). The compiler **unions** every block for
the same target; implementing the same member twice is a compile error.

```checkmate
impl counter {
    counter bump(counter c) {
        c.value += 1
        return c
    }
}

// a second block, unioned with the first:
impl counter {
    int peek(counter c) {
        return c.value
    }
}
```

## Value semantics through members

Members follow the same discipline as everything else: the receiver is a
copy, so mutations inside the member are visible only through its return
value. Reassign to apply:

```checkmate
counter bumped = counter.bump(c)
// bumped.value == 42, c.value still 41
```

A mutating "method" is by convention a function that takes and returns the
type. (This is deliberate: with no aliasing, there are no borrowed
receivers to reason about.)

## `impl` on enums

Same rules, on enum targets:

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

suit.label(suit.Hearts())    // "hearts"
```

## Host-style dotted-path targets

An `impl` target may be a **dotted path** — typically
`namespace.interface`, the shape of a schema contract:

```checkmate
struct GameConfig {
    int startingScore
    bool active
}

struct GameState {
    int score
    bool active
}

impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: config.startingScore, active: config.active)
    }

    GameState OnTick(GameState state, float deltaTime) {
        state.score += 1
        return state
    }
}
```

- Without a schema, these members still type-check and run; the host
  invokes them by their path (`engine.gamemode.InitGame(config)` from
  script code, or `context.invoke_member("engine.gamemode", "InitGame", ...)`
  from a host).
- With a [schema](../schema/overview.md) active, `impl <namespace>.<interface>`
  becomes a **contract satisfaction**: every required visible member must be
  implemented with the exact signature, `optional` members may be skipped,
  and version gating applies. Members declared PascalCase — they are
  boundary declarations.
- Implementing an unknown or ungranted namespace is an error, not a silent
  skip: an impl is a claim, and claims get validated.

The cross-file version of this (a whole mod implementing an interface in
pieces) is [Implementing Interfaces Across Files](../mods/impl-union.md).

## What `impl` is not

- **Not inheritance.** There is no subtyping, no overriding, no dynamic
  dispatch between targets.
- **Not namespaced free functions for organization only** — the target must
  be a declared local type or a valid dotted path; you cannot `impl` a
  builtin type (`int`), and generic targets are not supported yet.
- **Not traits** — a schema `interface` (§9) is the contract concept, and
  satisfaction is checked structurally by the compiler.
