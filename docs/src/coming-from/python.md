# Coming from Python

Checkmate trades Python's dynamism for compile-time guarantees and
host-controlled execution. The good news: control flow reads similarly,
and `match` gives you the pattern-matching you know from `match`
statements (3.10+). The big shifts: **static types everywhere**, **value
semantics** (no references!), and **no runtime environment** (the host
provides everything).

## The two-minute syntax map

| Python | Checkmate |
| --- | --- |
| `def add(a, b): return a + b` | `int add(int a, int b) { return a + b }` |
| `x = 1` | `int x = 1` / `infer x = 1` |
| `x = 1.5` (rebinding changes type) | impossible — a variable's type is fixed |
| `if x > 0:` | `if (x > 0) { }` — parens and braces required |
| `for v in xs:` | `for (int v in xs)` |
| `while cond:` | `while (cond) { }` |
| `class P:` | `struct p` + `impl p` |
| dict `{"gold": 120}` | `map<str, int> loot = {"gold": 120}` |
| list `[1, 2, 3]` | `int[] xs = [1, 2, 3]` |
| `None` | `option<T>`'s `None()` |
| `raise`/`try`/`except` | `result<T, E>` + `?` — no exceptions |
| `match ... case` (3.10) | `match` — exhaustive, on enums |
| `f"{x} and {y}"` | `$"{x} and {y}"` |
| `len(xs)` | `xs.length` |
| `dict[key]` (KeyError) | `m[key]` — missing key terminates the invocation |
| modules/`import` | [m[Mods](../mods/mods.md) + `import self.path.to.module` |
| `if __name__ == "__main__"` | host picks the entry point (`main` is a CLI convention) |
| `None` default args | named arguments; no defaults (yet) |

## Types are mandatory — and that is the deal

Python's `x = 1` becomes either `int x = 1` or the explicit
crystallization `infer x = 1`. There is no duck typing: every expression
checks against a static type, and the checker runs before *any* code
executes.

```python
# Python
def heal(player, amount):
    player["hp"] = min(100, player["hp"] + amount)
    return player
```

```checkmate
// Checkmate
player heal(player p, int amount) {
    int newHp = p.health + amount
    if (newHp > 100) {
        newHp = 100
    }
    return player(health: newHp, ...)   // fields completed with named args
}
```

Notice two shifts: **no `min` builtin yet** (write the `if`), and the
update is returned because of value semantics — next section.

Also: a variable's type never changes. `x = 1` then `x = "now a str"`
is a compile error, not a rebinding. This kills whole bug classes
(type-coercion surprises, typos creating shadow variables) at compile
time.

## Value semantics: no references, ever

Python binds names to objects; mutation through any alias is visible
everywhere. Checkmate has **no aliasing**:

```python
# Python
def bump(loot):
    loot["gold"] += 30        # caller's dict changes!

loot = {"gold": 0}
bump(loot)
print(loot)                    # {'gold': 30}
```

```checkmate
// Checkmate
void bump(map<str, int> loot) {
    loot["gold"] += 30        // mutates the local copy only
}

map<str, int> loot = {"gold": 0}
bump(loot)
loot["gold"]                   // still 0
loot["gold"] += 30             // the update pattern: mutate what you own
```

Assignment, parameters, `for` loop elements, struct fields — everything
copies (cheaply: copy-on-write under the hood). If you have been bitten by
Python's mutable default arguments or shared state across function calls,
this model is the cure.

## Errors are values

There are no exceptions and no stack unwinding:

```python
# Python
def safe_div(a, b):
    if b == 0:
        raise ValueError("division by zero")
    return a // b

try:
    total = safe_div(a, b)
except ValueError as e:
    ...
```

```checkmate
// Checkmate
result<int, str> safeDiv(int a, int b) {
    if (b == 0) {
        return Err("division by zero")
    }
    return Ok(a / b)
}

result<int, str> caller() {
    int total = safeDiv(a, b)?    // propagate like `raise`, as a value
    return Ok(total)
}
```

`?` is your `except`-free early return; `match` handles errors when you
want to inspect them. True *unexpected* failures (overflow, division by
zero, out-of-bounds index) terminate the invocation with a clean error
the host reports — they cannot be caught, by design.

## Dictionaries → maps, with caveats

```checkmate
map<str, int> loot = {"gold": 120, "gems": 3}
loot["gold"]                    // read
loot["gold"] += 30              // update
loot["arrows"] = 60             // insert
loot["missing"]                 // TERMINATES the invocation (no KeyError to catch)
```

- Keys may be any type (`map<int, str>` works).
- All values share one type — a heterogeneous dict is a struct instead:

  ```python
  # Python: config = {"retries": 3, "host": "db.local"}
  ```

  ```checkmate
  // Checkmate: heterogeneous → struct
  struct config {
      int retries
      str host
  }
  ```

- No `.get()`, no `.keys()`/`.items()` yet — iterate arrays; build the
  shapes you need. The core library is specified but not implemented
  (see [status](../status.md)).

## Classes → structs + impl

```python
# Python
class Counter:
    def __init__(self, value): self.value = value
    def bump(self):
        self.value += 1
        return self
```

```checkmate
// Checkmate
struct counter {
    int value
}

impl counter {
    counter bump(counter c) {
        c.value += 1
        return c
    }
}

counter bigger = counter.bump(counter(value: 41))
```

No methods inside the type, no `self` keyword, no inheritance, no
dunder protocols. Calls are qualified: `counter.bump(c)`. Variation that
you would model with subclasses becomes enums + `match`:

```python
# Python: isinstance checks across subclasses
```

```checkmate
// Checkmate
match (evt) {
    Damage(int amount) => { ... }
    Spawn(str kind, vec2 pos) => { ... }
    _ => { ... }
}
```

## The environment is the host's

There is no `os`, `sys`, `requests`, or `time` module — **and no import
that could provide one silently**. Anything environmental (files, HTTP,
clock, randomness) is a *capability* the host grants through a
[schema](../schema/overview.md):

```checkmate
import engine.http

result<str, str> fetch(str url) {
    return engine.http.Get(url)
}
```

If the host did not grant it, the import fails to compile. That is the
sandbox: not a lint, a gate.

## Indentation is braces, and other syntax notes

- Blocks are `{ ... }`; conditions need parentheses; `elif` is `else if`.
- Newlines *end statements* — no semicolons, and multi-line expressions
  continue only with a trailing operator or inside parentheses
  ([Significant Newlines](../language/newlines.md)).
- Integer division truncates toward zero: `7 / 2 == 3`, `-7 / 2 == -3`
  (Python floors: `-4`). `%` takes the dividend's sign: `-7 % 2 == -1`.
- No `++`; use `x += 1`. No ternary; use `if`/`else` or `match`.
- String `+` concatenates and stringifies the other side: `"HP: " + 100`
  is `"HP: 100"` — the only implicit conversion in the language.

## A tiny side-by-side

```python
# Python
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

print(fib(10))
```

```checkmate
// Checkmate
int fib(int n) {
    if (n <= 1) {
        return n
    }
    return fib(n - 1) + fib(n - 2)
}

int main() {
    return fib(10)
}
```

`cme run file.cm` prints `55`.

## Where to go next

- [A Ten-Minute Tour](../getting-started/tour.md) — the whole surface.
- [Value Semantics](../language/value-semantics.md) — the one rule that
  changes your reflexes most.
- [Pattern Matching](../language/match.md) — `match` with payloads.
- Embedding Python today? [Embedding Overview](../embedding/overview.md)
  explains where Checkmate fits.
