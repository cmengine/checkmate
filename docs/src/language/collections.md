# Collections: Arrays and Maps

Checkmate ships two compound collection types: **arrays** (`T[]`) and
**maps** (`map<K, V>`). Both are value-semantic (assignment clones), both
are indexed with `[]`, and both support comma- or newline-delimited
literals.

## Arrays

An array type is the element type followed by `[]`:

```checkmate
int[] evens = [2, 4, 6, 8]
str[] names = ["amy", "bo"]
vec2[] points = [
    vec2(x: 1.0, y: 2.0)
    vec2(x: 3.0, y: 4.0)
]
int[][] grid = [
    [1, 2]
    [3, 4]
]
int[] empty = []           // element type from the declaration
```

- Literals are comma-separated on one line, or **newline-delimited** across
  lines (CMON style). Do not mix "one element per line *and* trailing
  commas"; pick a form.
- Arrays nest: `int[][]` is an array of arrays.
- Elements must all have the literal's element type — the checker enforces
  one element type per array.

### `.length`

Arrays expose a built-in `.length` member, of type `int`:

```checkmate
evens.length               // 4
grid[1].length             // 2
```

`.length` is currently the **only** collection built-in; see the
[status notes](../status.md#small-divergences-worth-knowing).

### Indexing

```checkmate
evens[0]                   // read: 2
evens[3]                   // read: 8
evens[0] = 99              // index assignment
grid[0][1] = 20            // nested index assignment
points[1] = vec2(x: 30.0, y: 40.0)
```

- Indices are `int` expressions.
- **Out-of-bounds indexing terminates the invocation** with a clean,
  positioned runtime error — it never returns a default value and never
  panics the host.
- Index assignment on a nested path (`grid[0][1] = 20`) mutates the array
  owned by the variable on the left — value semantics mean this never
  aliases anyone else's array.

### Iteration

`for`-in iterates arrays by value:

```checkmate
for (int v in evens) {
    total += v
}
```

The element is a copy; assigning to it does not write back. For
index-based mutation use a `while` loop (see
[Control Flow](../language/control-flow.md#control-flow)).

### Value semantics

```checkmate
int[] copy = evens
copy[0] = 99
evens[0]                   // still 2

void resetFirst(int[] values) {
    values[0] = 0          // mutates the parameter's copy
}
resetFirst(odds)
odds[0]                    // unchanged
```

Behind the scenes the implementation shares buffers with copy-on-write, so
the clone is cheap until a mutation actually happens — but the *observable
semantics* are always an independent value.

### Equality

Arrays compare element-wise: `[1, 2] == [1, 2]` is `true`. Same-type
requirement applies, as everywhere.

## Maps

A map type is `map<K, V>` with any key and value types:

```checkmate
map<str, int> loot = {
    "gold": 120
    "gems": 3
}

map<str, int> tally = {"a": 1, "b": 2}     // one-line form
map<int, str> names = {
    1: "one"
    2: "two"
}
map<str, vec2> spawns = {
    "goblin": vec2(x: 2.0, y: 3.0)
    "orc": vec2(x: 4.0, y: 5.0)
}
map<str, int> fresh = {}                   // empty, types from declaration
```

- Keys and values are separated by `:`; entries by commas or newlines.
- Keys may be any type (strings and ints are the common cases).
- Map literals in `{ ... }` are distinct from blocks by context — they
  appear in expression position.

### Indexing reads, writes — and inserts

```checkmate
loot["gold"]               // 120
loot["gold"] += 30         // compound index assignment
loot["arrows"] = 60        // insert: the key did not exist, now it does
spawns["orc"].x = 40.0     // mutate a struct stored in the map
```

- **Reading a missing key terminates the invocation** with a clean runtime
  error (the same determinism policy as array bounds). There is no
  `None`-returning lookup yet; if absence is expected, model it in your own
  data (e.g. store `option<int>` values) rather than probing.
- Assignment to a missing key **inserts** it.

### Ordering and equality

Maps compare **order-insensitively**: two maps are equal when they hold
the same key/value pairs, regardless of insertion order. Iteration order
is not part of the language today (there is no map iterator); collect what
you need into an array explicitly.

## Choosing shapes

| Need | Shape |
| --- | --- |
| Ordered sequence, iteration | `T[]` |
| Lookup by identifier | `map<str, V>` |
| Sparse counters | `map<str, int>` / `map<int, int>` |
| Optional presence | `option<T>` element, or absence modeled by the host |
| Multi-valued grouping | `map<str, int[]>` |

Both collections are **values** end to end: they nest, compare, clone, and
cross the host boundary as [array/map values](../embedding/overview.md#values-across-the-boundary)
— and their literal syntax is the same syntax as
[CMON](../language/strings.md#cmon) data notation.
