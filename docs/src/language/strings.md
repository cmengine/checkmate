# Strings

`str` is Checkmate's immutable UTF-8 text type. This page covers literals,
escapes, interpolation, concatenation, and CMON — the notation that shares
the language's literal syntax.

## Literals and escapes

```checkmate
str plain = "hello"
str escaped = "line1\nline2\t\"quoted\" \\ backslash"
```

- Double quotes only; single quotes are not string delimiters in
  expressions.
- Backslash escapes: `\n` (newline), `\t` (tab), `\r` (carriage return),
  `\"` (quote), `\\` (backslash), and other standard C-family escapes.
- Strings are **immutable** — there is no in-place mutation; you build new
  strings by concatenation or interpolation.

## Interpolation: `$"..."`

The interpolated string form prefixes the opening quote with `$`. Braces
open an **island** containing any expression; the island's value is
stringified into the string:

```checkmate
int hp = 100
vec2 p = vec2(x: 3.5, y: -1.5)
bool armed = true

str s1 = $"hp={hp}"                          // "hp=100"
str s2 = $"pos=({p.x},{p.y})"                // "pos=(3.5,-1.5)"
str s3 = $"score={hp * 2 + 1} next={fib(7)}" // "score=201 next=13"
str s4 = $"armed={armed}"                    // "armed=true"
```

- Islands accept full expressions: field chains, arithmetic, calls.
- **Stringification inside islands follows the same rules as
  concatenation** (below): ints render as decimals, bools as `true`/`false`,
  floats as their shortest round-trip form, structs/enums in CMON form.
- Escapes still decode in the literal parts:
  `$"line1\nline2 hp={hp}"`.
- Interpolation composes with concatenation:
  `"report: " + $"hp={hp}"`.

## Concatenation and stringification

`+` concatenates when at least one operand is a `str`:

```checkmate
"HP: " + 100      // "HP: 100"
"ok: " + true     // "ok: true"
1.5 + "x"         // "1.5x"
"a" + 1 + 2       // "a12"   — ("a" + 1) + 2
1 + 2 + "a"       // "3a"    — (1 + 2) + "a"
```

The canonical renderings:

| Type | Rendered as |
| --- | --- |
| `int` | decimal digits, `-` when negative |
| `bool` | `true` / `false` |
| `float` | shortest decimal form that round-trips (`3.5`, `-1.5`, `0.8`) |
| `str` | as-is |
| struct / enum | CMON form (below) |

Stringification is **not** a general coercion: a `str` never becomes a
number, and no operator except `+` accepts `str` operands
([Operators](../language/operators.md#operand-typing)).

There are **no string methods** yet (`s.length`, `s.split`, ... do not
exist) — the §11 core library is specified but not implemented. Build what
you need with concatenation, interpolation, and your own functions; see
the [status page](../status.md#small-divergences-worth-knowing).

## CMON: Checkmate Object Notation

CMON is the language's canonical, human-readable data rendering — it
shares the exact literal syntax of the language itself:

```text
Player(
    name: "Hero"
    position: Vec2(x: 100.0, y: 50.0)
    inventory: [
        Item(id: 1, count: 5)
        Item(id: 42, count: 1)
    ]
    settings: {
        "autoSave": true
        "volume": 0.8
    }
)
```

Where you meet it:

- `cme run` prints the invoked entry's result in CMON form (unless `void`).
- The C API's `cm_value_to_string` returns the canonical CMON rendering.
- Array and map *literals in source* use the same newline-or-comma shapes.
- Host logs and the [rust host app](../embedding/rust.md#printing-results)
  echo CMON text.

CMON is designed to deserialize into typed structs and enums — it is
schema-aware in the whitepaper's design; today it is the display form and
the literal-syntax twin.

## UTF-8

Strings are UTF-8 end to end. The host APIs validate UTF-8 at the boundary
(in C, `cm_value_str` rejects invalid UTF-8), and embedded NUL bytes in
script text render as U+FFFD in C-land because C strings cannot carry
them. For most embedding work this is invisible; the details live in the
[C guide](../embedding/c.md#strings-and-utf-8).
