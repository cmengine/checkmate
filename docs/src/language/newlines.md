# Significant Newlines

Checkmate is newline-sensitive by design: a line break usually ends a
statement, the way a semicolon would in C. This page gives the exact rules,
because they have a few deliberate corners.

## The core rule

> A line break is significant after a token that can end a statement, and
> insignificant otherwise.

Practically:

- Finish a statement on its line. `int x = 1` then newline — done.
- An expression **continues** across a newline when the line ends with a
  binary operator or an open parenthesis:

  ```checkmate
  int total = base +
      bonus              // ok: trailing operator continues the expression
  ```

- A line that **begins** with a binary operator is a compile error:

  ```checkmate
  int total = base
      + bonus            // error: leading binary operator
  ```

- After a trailing operator, a continuation line may begin with a **unary**
  operator — it binds to the operand, not the statement stream:

  ```checkmate
  int d = a +
      -b                 // ok: a + (-b)
  ```

## Inside parentheses: newlines are free

Newlines are insignificant inside the innermost **parentheses** — call
arguments, construction arguments, parenthesized expressions:

```checkmate
vec2 pos = vec2(
    x: 10.0
    y: 5.0
)

clamp(
    value: -5
    low: 0
    high: 10
)

int total = (base +
    bonus)
```

Named arguments across lines drop the commas entirely — the newline
delimits. This is the language's preferred multi-line call style.

## Inside brackets and braces: newlines are delimiters

Inside `[ ... ]` (arrays) and `{ ... }` (maps, `match` bodies) newlines
**stay significant** — they separate elements, entries, and arms:

```checkmate
int[] odds = [
    1
    3
    5
]

map<str, int> loot = {
    "gold": 120
    "gems": 3
}
```

So a newline is not whitespace there; it is a separator (the comma form
also works: `[1, 3, 5]`, `{"a": 1, "b": 2}`). Match arms are
newline-delimited the same way (see
[Pattern Matching](../language/match.md)).

Struct *declarations* follow the same shape: fields are
newline-delimited ([Structs](../language/structs.md)).

## Why significant newlines

Three reasons, all deliberate:

1. **One statement per line reads cleanly** — the overwhelming majority of
   code in any C-family language puts one statement per line anyway; the
   newline rules make the formatter's job trivial and diff noise low.
2. **No ASI ambiguity.** JavaScript's automatic semicolon insertion is a
   famous source of subtle bugs. Checkmate's rule is total: a newline
   after a statement-ending token *is* the statement end — no fallback
   parsing to second-guess.
3. **LLM- and human-regular grammar.** The whole surface is designed so a
   line's role is locally decidable.

## Style rules of thumb

- Put one statement per line; break expressions **after** operators.
- Multi-line calls and constructions: inside the parentheses, one argument
  per line, no commas.
- Multi-line collections: one element per line (newline form) or one line
  with commas — do not mix.
- Never start a line with a binary operator.

These match what the checker accepts; the future formatter will enforce
the same shapes.
