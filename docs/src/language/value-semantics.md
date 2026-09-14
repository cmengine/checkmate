# Value Semantics

Value semantics is Checkmate's defining discipline: **every value behaves
as independently owned data**. Assignment clones, parameters clone, struct
fields clone on write-out. Mutating a value is visible only through the
variable you mutated.

This chapter states the rule precisely, shows its consequences, and
explains what the implementation does under the hood.

## The rule

- **Assignment copies**: `b = a` gives `b` its own `a`.
- **Parameter passing copies**: a function receives independent values.
- **Return copies out**: returning a struct hands the caller a new value.
- **Fields and elements copy on read-out**: `squad.leader` is a copy of the
  leader, `evens` copied into `copy` is a copy.

The consequence: **mutation never escapes**. There is no aliasing, no
shared mutable state, no "who else holds this?" — ever.

```checkmate
player damage(player p, int amount) {
    p.health -= amount          // mutates the local copy
    if (p.health <= 0) {
        p.alive = false
    }
    return p                    // hand the updated copy back
}
```

```checkmate
player hurt = damage(hero, 60)  // hero unchanged
hero = damage(hero, 60)         // reassignment applies the update

squad.leader.position.x = 40.0  // squad.leader is a copy;
                                // hero.position.x unchanged
```

## Reassignment as the update pattern

Because mutation is local, "update" means "compute the new value, then
assign it":

```checkmate
state = engine.gamemode.OnTick(state, deltaTime)
hero = heal(hero, 40)
scores[0] = scores[0] * 2
```

This is not boilerplate to work around — it is the model. Data flow is
visible in the source: if a variable changed, its assignment (or a field
assignment rooted at it) is in the code you are reading.

## Copying is cheap: ARC + COW

The language guarantees the *semantics* of copies; the implementation
guarantees their *cost profile*:

1. **Stack allocation bias** — values that do not escape their scope live
   on the stack (planned aggressively in the AOT backend; the tree walker
   allocates on the Rust heap with the same escape discipline).
2. **Copy-on-write (COW)** — strings, arrays, maps, and heap-promoted
   records share refcounted backing buffers. A "copy" increments a count;
   only a *mutation* on a shared buffer materializes a private copy first.
3. **Automatic reference counting (ARC)** — when data escapes, the
   compiler emits balanced increments/decrements. Reclamation is
   deterministic: no tracing GC, no stop-the-world pauses, no background
   threads.

Net effect: the semantics are "everything clones", the behavior of clones
of unmutated data is O(1), and memory reclamation is deterministic — which
is exactly what a host embedding untrusted scripts wants.

## What this buys you

- **Race-free concurrency.** Independent invocations share nothing mutable;
  the host can run a [context](../embedding/overview.md#contexts) from many
  threads without locks around script data.
- **No defensive copies in your head.** You never think about aliasing,
  because there is none.
- **Deterministic reclamation.** Dropping an invocation drops its values
  now, not "eventually".
- **Simple equality reasoning.** `==` compares values, not identities;
  there *is* no identity.

## Contrast with other languages

If you are coming from:

- **Rust**: there is no borrow checker, because there are no borrows. Think
  of every type as `Clone`-by-value with implicit cheap clones. No `&`,
  no `&mut`, no lifetimes.
- **Go/C#/Java**: no reference types *as a language concept*. A struct
  parameter is not a pointer to the caller's instance — ever. Maps and
  arrays are values too (contrast Go's maps and slices).
- **Python**: no reference semantics. `b = a` never aliases; there is no
  `is` distinction to worry about, and no shared-mutable-default pitfalls.
- **TypeScript/JavaScript**: no objects-by-reference; a struct is a value
  like a JS *primitive*, not like a JS object.

The [Coming from ...](../coming-from/go.md) guides expand these contrasts
per language.

## Where the boundary lives

Value semantics governs **script-visible values**. Host-owned resources —
textures, entities, sockets — are represented as **opaque handles** that
cross the boundary by identifier, not by copying the resource:

```checkmate
TextureHandle texture = engine.graphics.LoadTexture("hero.png")
engine.graphics.DrawTexture(texture, pos)
```

Copying a handle copies the identifier; resource lifetime and thread rules
belong to the host API. See
[Embedding Overview → Opaque handles](../embedding/overview.md#opaque-host-handles).
