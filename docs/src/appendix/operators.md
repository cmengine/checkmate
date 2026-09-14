# Operator Reference

The condensed operator table. The working chapter is
[Operators and Expressions](../language/operators.md); the normative
source is Appendix A of the
[whitepaper](https://github.com/cmengine/checkmate/blob/mom/WHITEPAPER.md).

## Precedence (loosest → tightest)

All binary operators are left-associative; comparisons are
non-associative (chaining requires parentheses). Unary `-`/`!` are
prefix and nest freely.

| Level | Operators | Category | Associativity |
| --- | --- | --- | --- |
| 1 | `\|\|` | logical or | left |
| 2 | `&&` | logical and | left |
| 3 | `==` `!=` `<` `<=` `>` `>=` | comparison | non-associative |
| 4 | `+` `-` | additive | left |
| 5 | `*` `/` `%` | multiplicative | left |
| 6 | `-` `!` | unary (prefix) | prefix |

Postfix operations — call `f(x)`, field access `a.b`, index `a[i]` —
bind tighter than every level above. Parentheses override everything.

## Operand typing and results

| Operators | Operand types | Result |
| --- | --- | --- |
| `+` | both `int`, or both `float` | as operands |
| `+` | ≥ 1 `str`; other side `str`/`int`/`float`/`bool` | `str` |
| `-` `*` | both `int`, or both `float` | as operands |
| `/` | both `int`, or both `float` | as operands |
| `%` | both `int` | `int` |
| `<` `<=` `>` `>=` | both `int`, or both `float` | `bool` |
| `==` `!=` | both operands same type | `bool` |
| `&&` `\|\|` | both `bool` | `bool` |
| `-` (unary) | `int` or `float` | as operand |
| `!` (unary) | `bool` | `bool` |

No implicit coercions exist. `+` is string concatenation exactly when
either operand is `str`; no other operator accepts `str`.

## Mandatory parenthesization

```checkmate
a || b && c        // ERROR: mixed && and || — write a || (b && c)
a < b < c          // ERROR: chained comparison — write a < b && b < c
a == b < c         // ERROR — write a == (b < c)
```

## Evaluation rules

- `&&`/`||` short-circuit (right side evaluated only when the left does
  not decide).
- Integer division truncates toward zero: `7 / 2 == 3`, `-7 / 2 == -3`.
- Remainder takes the dividend's sign: `-7 % 2 == -1`, `7 % -2 == 1`.
- Float division is IEEE 754; `NaN != NaN`.
- Division/remainder by zero and integer overflow **terminate the
  invocation** with a clean error (never wrap, never panic the host).

## Stringification (in `+` concatenation and interpolation)

| Type | Rendered as |
| --- | --- |
| `int` | decimal, `-` when negative |
| `bool` | `true` / `false` |
| `float` | shortest round-trip decimal |
| `str` | as-is |
| struct/enum | CMON form |

Examples: `"HP: " + 100` → `"HP: 100"`; `"a" + 1 + 2` → `"a12"`;
`1 + 2 + "a"` → `"3a"`.

## Compound assignment

`+=`, `-=`, `*=`, `/=`, `%=` — statements, exactly equivalent to the
expanded form; no value, no chaining. Plain `=` is a statement too.

## Deliberately absent

Bitwise operators (`&`, `|`, `^`, `<<`, `>>`, `~`), ternary `?:`,
exponentiation, `++`/`--`, assignment-as-expression, cross-type
arithmetic/comparison/equality.
