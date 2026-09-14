# Hello, Checkmate

Create a file called `hello.cm`:

```checkmate
struct player {
    str name
    int health
}

int heal(player p, int amount) {
    int newHealth = p.health + amount
    if (newHealth > 100) {
        return 100
    }
    return newHealth
}

str main() {
    player hero = player(name: "Hero", health: 75)
    int healed = heal(hero, 40)
    return $"hero={hero.name} health={hero.health}->{healed}"
}
```

Run it:

```sh
cme run hello.cm
```

Output:

```text
hero=Hero health=75->100
```

Ten things are happening in this tiny program — each is a deliberate
language decision:

1. **No semicolons.** Statements are delimited by newlines. A statement ends
   when its line ends.
2. **Types come first.** `int heal(player p, int amount)` reads like the
   declaration it is: return type, name, typed parameters.
3. **Struct fields are newline-delimited** — no commas or semicolons between
   members. When you construct on one line, commas are accepted.
4. **Construction is named.** `player(name: "Hero", health: 75)` names every
   field. Field order in the literal does not matter; completeness is
   checked at compile time.
5. **Mutable by default.** `int newHealth = p.health + amount` declares a
   variable; nothing more is needed to assign it later.
6. **Parenthesized conditions, braced bodies.** `if (newHealth > 100) { ... }`
   — the condition needs parentheses, the body needs braces.
7. **`return` everywhere.** The last expression is *not* the value; only
   `return` returns.
8. **camelCase for script-internal things.** Functions, variables, fields,
   and parameters are camelCase. Names that cross the host/script boundary
   are PascalCase (you will see them when a [schema](../schema/overview.md)
   is active).
9. **Interpolation with `$"..."`.** `{expr}` islands inside an interpolated
   string evaluate any expression and stringify the result.
10. **`main` is a CLI convention, not a language concept.** The language has
    no implicit entry point ([§2.1 of the whitepaper](https://github.com/cmengine/checkmate/blob/mom/WHITEPAPER.md));
    `cme run` simply invokes the function named `main` and prints its result
    in the language's canonical CMON rendering. Real hosts invoke whatever
    function they choose — see [Embedding Overview](../embedding/overview.md).

## Value semantics, demonstrated early

Change `heal` to mutate its parameter:

```checkmate
int healInPlace(player p, int amount) {
    p.health += amount
    return p.health
}
```

Calling `healInPlace` does **not** change the caller's `hero`. The function
received a copy; the mutation is visible only inside it. To apply an update,
return the new value and reassign:

```checkmate
player healed = hero      // an independent copy
healed.health += 40       // visible only in `healed`
```

This rule has no exceptions — structs, arrays, maps, everything. It is
covered in depth in [Value Semantics](../language/value-semantics.md).

## Errors, early

What happens if you write `p.health += 40` but forget to declare `p`? Or
call `heal(hero)` with a missing argument? Or read `loot["missing"]` on a
map without inserting that key first? Compile-time mistakes stop the build
with a rendered `file:line:column` diagnostic, and `cme run` **refuses to
execute any program that produced a diagnostic** — even a runtime-bounded
one like division by zero terminates the invocation cleanly rather than
crashing the host.

## Next

Continue with [A Ten-Minute Tour](tour.md) to see the whole surface —
enums, `match`, collections, `option`/`result`, `impl` blocks, and a
glimpse of megaprogramming.
