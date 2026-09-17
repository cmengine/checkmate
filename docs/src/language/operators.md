# Operators and Expressions

This page is the working reference for Checkmate's expression language. The
normative source is Appendix A of the
[whitepaper](https://github.com/cmengine/checkmate/blob/mom/WHITEPAPER.md);
a condensed table also lives in [Appendix: Operator Reference](../appendix/operators.md).

## Precedence and associativity

From loosest to tightest. All binary operators are **left-associative**,
except comparisons, which are **non-associative** (see below).

| Level | Operators | Category | Associativity |
| --- | --- | --- | --- |
| 1 | `\|\|` | logical or | left |
| 2 | `&&` | logical and | left |
| 3 | `==` `!=` `<` `<=` `>` `>=` | comparison | non-associative |
| 4 | `+` `-` | additive | left |
| 5 | `*` `/` `%` | multiplicative | left |
| 6 | `-` `!` (prefix) | unary | prefix |

Parenthesized expressions override precedence and bind tighter than
everything. Unary operators nest freely: `!!flag` is fine, and since `--`
is not a token, `--x` is `-(-x)`.

```checkmate
1 + 2 * 3        // 7
10 - 4 - 3       // 3  (left-associative)
-x * y           // (-x) * y
```

## Two mandatory-parenthesization rules

Two constructs are **compile-time errors** unless explicitly parenthesized.
Both rules exist because the unparenthesized form is a well-known source of
silent mistakes.

**Rule 1 — mixing logical operators.** An expression containing both `&&`
and `||` must parenthesize the mix:

```checkmate
a || b && c        // error: mixed && and ||
a || (b && c)      // ok
(a || b) && c      // ok
a && b && c        // ok: same operator, left-associative
```

**Rule 2 — comparisons do not chain.** An operand of a comparison may not
itself be a comparison expression unless parenthesized:

```checkmate
a < b < c          // error: chained comparison; write a < b && b < c
a == b < c         // error; write a == (b < c)
(a < b) == c       // ok (c must be bool)
```

## Operand typing

Operators are strict. There are **no implicit coercions**, including
through operators.

| Operators | Operand types | Result |
| --- | --- | --- |
| `+` | both `int`, or both `float` | as operands |
| `+` | at least one `str`; other side `str`, `int`, `float`, or `bool` | `str` (concatenation) |
| `-` `*` | both `int`, or both `float` | as operands |
| `/` | both `int`, or both `float` | as operands |
| `%` | both `int` | `int` |
| `<` `<=` `>` `>=` | both `int`, or both `float` | `bool` |
| `==` `!=` | both operands of the same type | `bool` |
| `&&` `\|\|` | both `bool` | `bool` |
| `-` (unary) | `int` or `float` | as operand |
| `!` (unary) | `bool` | `bool` |

- `+` means numeric addition only when **both** operands are numeric. If
  either operand is `str`, `+` is string concatenation. The meaning of each
  `+` node is determined entirely by its operand types — never by context.
- No operator other than `+` accepts a `str` operand: `"ab" * 3` is a type
  error. There is no string repetition.
- Cross-type equality is a type error: `1 == "1"` does not compile.

## Evaluation semantics

**Short-circuiting.** `lhs && rhs` evaluates `rhs` only when `lhs` is
`true`; `lhs || rhs` evaluates `rhs` only when `lhs` is `false`. The
right-hand side is never evaluated when the left decides:

```checkmate
bool safe = f && 1 / 0 == 1      // false, and no division ever runs
```

**Integer division truncates toward zero** and yields an `int`. The
remainder takes the sign of the dividend:

```checkmate
7 / 2     // 3
-7 / 2    // -3
-7 % 2    // -1
7 % -2    // 1
```

**Float division** is ordinary IEEE 754 division.

**Division, remainder by zero, and integer overflow** terminate the
invocation at runtime with a clean, positioned error — they never panic
the host, wrap silently, or produce `inf`. This is part of the language's
determinism guarantee; see
[Execution Limits and Errors](../appendix/limits-and-errors.md).

## String concatenation and stringification

When either operand of `+` is a `str`, the other operand converts to its
canonical string form:

| Type | Rendered as |
| --- | --- |
| `int` | decimal digits, `-` prefix when negative |
| `bool` | `true` / `false` |
| `float` | shortest decimal representation that round-trips |
| `str` | as-is |

```checkmate
"HP: " + 100      // "HP: 100"
"ok: " + true     // "ok: true"
1.5 + "x"         // "1.5x"
"a" + 1 + 2       // "a12"   — parsed as ("a" + 1) + 2
1 + 2 + "a"       // "3a"    — parsed as (1 + 2) + "a"
```

The last two are consequences of left-associativity: deterministic, but
mixing numeric and string operands across a chain is discouraged style.
Stringification applies **only** within concatenation — it is not a general
coercion, and a `str` never becomes a number. For richer formatting use
[interpolation](../language/strings.md).

## Compound assignment

The compound assignment operators are `+=`, `-=`, `*=`, `/=`, and `%=`.
Each is a **statement**, exactly equivalent to expanding the operator:

```checkmate
x += 10           // identical to: x = x + 10
s += "!"          // identical to: s = s + "!"
s += 100          // identical to: s = s + 100 (uses stringification)
```

- Compound assignment yields no value; it cannot be chained or embedded in
  an expression: `x += y += 1` and `a = (b += 1)` are compile-time errors.
- Plain `=` is a statement too: `a = b = c` is a compile-time error.
- The target is evaluated exactly once. Index targets like `scores[0] += 5`
  and `loot["gold"] += 30` are supported (see
  [Collections](../language/collections.md)).

## What you can build expressions from

Beyond the operator grammar, expression positions accept:

- literals (int, float, bool, byte, string, interpolated string),
- identifiers,
- struct construction and enum construction (named or positional args),
- calls (top-level functions, `impl` members by qualified path),
- field access (`p.position.x`), index access (`scores[0]`, `loot["k"]`),
- `match (...) { ... }` in expression position,
- the `?` operator on `result` expressions,
- parenthesized expressions.

Precedence of member/index/call postfix operations: they bind tighter than
every operator in the table above. `evens[0] + 1`, `p.x * p.y`,
`counter.bump(c).value` all do what you expect.

## Deliberately absent

The following constructs do not exist in this version — each is a decision,
not an omission:

- **Bitwise operators** (`&`, `|`, `^`, `<<`, `>>`, `~`) — reserved for a
  future appendix.
- **Ternary conditional** (`?:`) — use `if`/`else`.
- **Exponentiation** — expected to arrive as a host math capability, not an
  operator.
- **Increment/decrement** (`++`, `--`) — value semantics make them
  pointless; write `x += 1`.
- **Assignment as an expression** — assignment yields no value and cannot
  chain.
- **Cross-type arithmetic, comparison, or equality** — never permitted.
