# Expansion Templates

The template is the target code with **holes**. Holes are typed by capture
kind and syntactic position; elaboration splices the pattern's capture
tree into them and produces Checkmate code — which is then parsed and
type-checked like handwritten code, with spans pointing back at the
embedded-language source.

## A complete example

```checkmate
mega def(py.def as d) {
    $d.ret $d.fname(each in d.params {
        [when present($ptype) { $ptype $param } else { infer $param }]
    }) {
        each in d.body {
            match ($item) {
                ifStmt  => if ($cond) { @py.emitBody($body) }
                return  => return $value
                call    => $callee(each in $args { $arg })
            }
        }
    }
}
```

Invoked as:

```checkmate
mega(def) {
    def clamp(v: int, lo: int, hi: int) -> int:
        if v < lo:
            return lo
        if v > hi:
            return hi
        return v
}
```

Expanding to ordinary, fully type-checked Checkmate:

```checkmate
int clamp(int v, int lo, int hi) {
    if (v < lo) {
        return lo
    }
    if (v > hi) {
        return hi
    }
    return v
}
```

## Hole types by position

| Hole position | ident/text/numeric capture | list capture | tagged record | code capture | optional capture |
| --- | --- | --- | --- | --- | --- |
| name (function, field, param) | splices the identifier | — | — | parsed as an identifier | — |
| type | — | — | — | parsed as a type | — |
| expression | splices as a literal value | array literal | must be `match`ed | parsed as an expression | — |
| element lists (params, args, statements, fields) | single element | repeats elements | — | repeats elements | single element if present |

An `optional { p } as x` capture holds `some(value)` or `none`;
`present($x)` tests it in `where` conditions and `[when]` guards, and
splicing an absent optional is a template compile error.

## Template constructs

| Construct | Meaning |
| --- | --- |
| `$cap` | splice a capture |
| `$"text {cap} more"` | interpolate captures into a string |
| `[each in xs { ... }]` | repeat per element (optional `where` filter) |
| `[when cond { ... } else { ... }]` | select a branch |
| `match ($cap) { label => ... }` | dispatch on a `oneof` tag; exhaustiveness checked |
| `let name = value` | bind a template-local value |
| `require(cond, "message")` | emit a compile-time error anchored at a capture's span |
| `@fn(args)` | invoke a compile-time function |

### `require`

```checkmate
mega value(toml.document as doc) {
    require(@tablesConsistent($doc), "table redefined or reopened with a conflicting type")
    @toValue($doc)
}
```

`require` is the post-match validator: it runs after matching, may inspect
the *whole* capture tree, and on failure reports at the offending node
rather than at a backtracked position. Prefer it whenever rejecting
suffices; use `where` only when the check must steer matching. The full
decision table is in the whitepaper's §8.3.4.

### Name resolution inside iteration

Inside `match ($item) { label => ... }` arms and `each in xs { ... }`
bodies, a bare `$field` resolves to the current element's field; the
qualified form (`$item.field`) always works and means the same thing.

## Spans: diagnostics point at embedded source

Every generated AST node carries the span of the template element and the
capture that produced it. A type error in generated code — say, a string
where an `int` is expected inside an HTML-derived call — is reported at
the embedded language's source position in your file, not at the macro
definition. Through recursive `@`-functions, spans thread along
([Compile-Time Functions](../mega/compile-time.md)), so even
machine-generated code keeps pointing at the user's text.

## Invocation positions

Megaprogram invocations are allowed in **declaration, statement,
expression, and type** positions — macros can generate anything the
language can declare. `mega(def) { ... }` above generates a top-level
function; `mega(json.value) { ... }` generates an expression;
`mega(yaml.value) { ... }` can generate a whole configuration struct.

## Determinism

Templates are pure splices: no side effects, no randomness, no clock.
Together with pure `@`-functions and a fixed traversal order, the same
source and grammars expand byte-identically on every platform — which is
what will keep future native-artifact hashes reproducible.
