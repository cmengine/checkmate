# Control Flow

Checkmate has three control-flow statements: `if`/`else`, `while`, and
`for`-in. All conditions are parenthesized; all bodies are braced blocks.
There is no `do`/`while`, no labeled loops, no `break`/`continue` in this
version — structure your loops with conditions and early `return`.

## `if` / `else if` / `else`

```checkmate
if (health <= 0) {
    alive = false
} else if (health < 25) {
    alive = true
} else {
    alive = true
}
```

- The condition must be `bool` — no truthiness, no implicit conversions
  from `int` or pointers.
- Parentheses around the condition are **required**.
- Braces around the body are **required**, even for a single statement.
- `if` is a *statement*, not an expression. It yields no value; compute the
  value before, or use [`match`](../language/match.md) when you need an
  expression form:

  ```checkmate
  str grade(int score) {
      if (score >= 90) {
          return "A"
      } else if (score >= 80) {
          return "B"
      } else if (score >= 70) {
          return "C"
      } else {
          return "F"
      }
  }
  ```

## `while`

```checkmate
int count = 0
int total = 0
while (count < 5) {
    total += fib(count)
    count += 1
}
```

The condition is `bool`, parenthesized, re-checked before each iteration.
Because there is no `break`, a `while` exits only through its condition or
by returning from the enclosing function. Hosts can always bound runaway
loops with [fuel or a deadline](../appendix/limits-and-errors.md).

## `for`-in

```checkmate
int sumAll(int[] values) {
    int total = 0
    for (int v in values) {
        total += v
    }
    return total
}
```

- The header is `for (Type element in collection)`.
- `collection` must be an **array** (`T[]`) — iteration over maps and other
  shapes is not part of the language today; collect the shape you need into
  an array first.
- The element variable is declared inline with its type. The element is a
  **copy** (value semantics): assigning to it inside the body does not
  write back into the array.
- There is no index form. Track the index yourself if you need it:

  ```checkmate
  int i = 0
  while (i < evens.length) {
      evens[i] = evens[i] * 2
      i += 1
  }
  ```

## Statements and blocks

A block `{ ... }` is a sequence of newline-delimited statements. Blocks
introduce scopes; a variable declared inside a block is gone when the block
ends. Match-arm blocks, function bodies, and branch bodies all follow this
one rule.

## Control transfer

- `return value` — exits the current function. In a `void` function, bare
  `return` exits.
- The [`?` operator](../language/error-handling.md) — an early return of an
  `Err` payload from the current function.
- There is no `break`, `continue`, `goto`, or exceptions. Error flow is a
  value flow: `result<T, E>` and `match`/`?` — see
  [Error Handling](../language/error-handling.md).

## A complete pattern

```checkmate
int countNegative(int[] values) {
    int negatives = 0
    for (int v in values) {
        if (v < 0) {
            negatives += 1
        }
    }
    return negatives
}
```

Everything composes: loops nest, `if`/`else` chains nest, and the checker
verifies that every non-void path returns before any of it runs.
