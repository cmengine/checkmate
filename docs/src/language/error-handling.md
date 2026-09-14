# Error Handling

Checkmate has **no exceptions, no panics-as-API, and no `null`**. Failure
is a value: `option<T>` for "maybe missing", `result<T, E>` for "ok or
failed", `match` to handle, and `?` to propagate. Runtime failures that are
not values (overflow, division by zero, out-of-bounds) **terminate the
invocation** cleanly rather than unwinding into your logic.

## `option<T>`: presence

```text
option<T> = Some(T value) | None()
```

```checkmate
option<int> findEven(int[] values) {
    for (int v in values) {
        if (v % 2 == 0) {
            return Some(v)
        }
    }
    return None()
}
```

Handle it with `match`:

```checkmate
int optionOrDefault(int[] values, int fallback) {
    option<int> found = findEven(values)
    int outcome = fallback
    match (found) {
        Some(int v) => { outcome = v }
        None() => {}
    }
    return outcome
}
```

Constructors work bare (`Some(v)`, `None()`) or qualified
(`option.Some(v)`, `option.None()`). There is no `.unwrap()` — the
`match` *is* the unwrap, and it cannot forget the `None` arm if you omit
the wildcard (exhaustiveness forces the decision).

## `result<T, E>`: success or failure

```text
result<T, E> = Ok(T value) | Err(E error)
```

```checkmate
result<int, str> safeDiv(int a, int b) {
    if (b == 0) {
        return Err("division by zero")
    }
    return Ok(a / b)
}
```

- The error type `E` is whatever you declare — `str` for quick diagnostics,
  your own error enum for structured handling.
- Match to handle:

  ```checkmate
  match (safeDiv(64, 0)) {
      Ok(int total) => { /* use total */ }
      Err(str reason) => { /* report reason */ }
  }
  ```

## The `?` operator: propagation

Inside a function returning `result<T', E'>`, applying `?` to a
`result<T, E'>` expression either binds the `Ok` payload or **returns
early from the enclosing function** with the `Err` payload:

```checkmate
result<int, str> chain(int a, int b, int c) {
    int first = safeDiv(a, b)?
    int second = safeDiv(first, c)?
    return Ok(first + second)
}
```

- `?` reads as "bail out on error, otherwise give me the value".
- The propagated error type must match the enclosing function's declared
  error type **exactly** — no implicit error conversion. `safeDiv`'s
  `str` errors flow through `chain`'s `result<int, str>` because the types
  agree; a `result<int, parseError>` inside a `result<int, str>` function
  is a compile error until you `match` and translate.
- `?` composes: `deepChain` chains several fallible calls in a row and the
  first failure short-circuits the whole function.
- `?` works only where the enclosing function returns a `result` with a
  matching error type; it is not a general "unwrap or die".

## Choosing the shape

| Situation | Tool |
| --- | --- |
| Value may legitimately be absent | `option<T>` + `match` |
| Operation can fail with context | `result<T, E>` + `?` at call sites |
| Programming error / impossible state | Let it terminate: the invocation ends with a clean error the host reports |
| Many distinct failure reasons | An error `enum` as `E`, destructured with `match` |

## Runtime failures are not values

Some failures are *not* modeled as values — they are contract violations
that end the invocation:

| Failure | Trigger |
| --- | --- |
| Integer overflow | Arithmetic outside the `int` range |
| Division / remainder by zero | `a / 0`, `a % 0` |
| Array index out of bounds | `xs[i]` with `i < 0` or `i >= xs.length` |
| Missing map key | Reading `m[k]` for an absent `k` |
| Call depth exceeded | Recursion past the configured limit |
| Fuel exhausted / deadline passed | Host-imposed [limits](../appendix/limits-and-errors.md) |

Each produces a clean, positioned error (`Runtime`, `CallDepth`, `Budget`,
`Deadline`) delivered to the **host** — a CLI run prints it; an embedded
host receives it as an [`ExecutionError`](../embedding/overview.md#errors) with
kind, message, file, line, and column. Script code cannot catch these, and
that is the point: they indicate broken invariants, not expected outcomes.
Expected failure belongs in `result`.

## Structured errors with enums

For rich error reporting, declare an error enum and return it:

```checkmate
enum parseFailure {
    UnexpectedToken(str found, int line)
    UnexpectedEnd()
}

result<int, parseFailure> parseValue(str text) {
    // return Err(parseFailure.UnexpectedToken("=", 12)) ...
    return Ok(0)
}
```

Callers `match` on the failure enum and get exhaustive handling of every
reason — errors become part of the data model, visible in signatures and
checked at compile time.
