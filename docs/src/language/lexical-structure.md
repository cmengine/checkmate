# Lexical Structure

This chapter defines the raw tokens of Checkmate: source files, comments,
identifiers, literals, and how newlines behave at the lexer level.

## Source files

Source files use the `.cm` extension and are UTF-8 text. There is no
preprocessor, no conditional compilation, and no shebang handling. A file
contains a sequence of top-level declarations: `struct`, `enum`, `impl`,
`grammar`, `mega`, and function declarations. There is **no top-level
mutable state** — no global `let`, no statics, no constants at file scope.
Persistent state belongs to the host; see
[Embedding Overview](../embedding/overview.md).

Schema files (§9) are a second file kind, also `.cm`, rooted by a `schema`
declaration; they are covered in [Schema Files](../schema/schema-files.md).

## Comments

```checkmate
// A single-line comment runs to the end of the line.

/*
   A block comment. { everything } "inside" + is ignored.
*/
```

Two lexical rules about block comments are worth internalizing:

- A block comment ends at the **first** `*/`. Nesting is not supported.
- An unterminated block comment swallows the rest of the file and produces
  exactly one clean lex error — you will not get a cascade.

Inside [megaprogramming](../mega/overview.md) grammars, comments are *data*
you model explicitly (a grammar's `comment` declaration); the rules above
apply to the Checkmate source itself.

## Identifiers

Identifiers match `[A-Za-z_][A-Za-z0-9_]*`. Case is significant and carries
meaning — see [Boundary Capitalization](#boundary-capitalization) below.

Keywords cannot be identifiers. The reserved set includes the declaration
and control keywords (`struct`, `enum`, `impl`, `if`, `else`, `while`,
`for`, `in`, `match`, `return`, `infer`, `import`, `self`, `grammar`,
`mega`), the primitive type names (`int`, `float`, `bool`, `str`, `void`),
and, in the megaprogramming and schema vocabularies, words like `rule`,
`each`, `oneof`, `where`, `skip`, `comment`, `string`, `island`,
`capability`, `interface`, `since`, `requires`, `optional`, `suspend`.

## Reserved for future use

Identifiers spelled `mega` followed only by digits — `mega0`, `mega1`,
`mega42`, … — are **reserved for future use** by the megaprogramming
machinery (§8). They are rejected at lex time with a dedicated diagnostic,
in every identifier position and in schema files too. `mega` alone,
`mega0x`, `mega_0`, and `mega42a` are ordinary identifiers.

## Primitive type names

The five scalar type names are reserved:

| Type | Meaning |
| --- | --- |
| `int` | Signed 64-bit integer |
| `float` | 64-bit IEEE 754 floating point |
| `bool` | `true` or `false` |
| `str` | Immutable UTF-8 string |
| `void` | The type of "no value"; functions returning nothing |

Details in [Types](../language/types.md).

## Numeric literals

```checkmate
int decimal = 42
int negative = -17          // unary minus applied to a literal
float pi = 3.14159
float tiny = 0.5
float sci = 6.02e23
```

- Integer literals are decimal.
- Float literals require a fractional part (`0.5`, not `5.`) and may carry
  an exponent.
- Numeric conversions between `int` and `float` are strictly explicit in
  intent — there is currently no cast operator, and **implicit coercions
  are disallowed** (see [Types](../language/types.md)).
- Integer arithmetic is overflow-checked at runtime: an overflowing
  operation terminates the invocation with a clean error.

## String literals

```checkmate
str plain = "hello"
str escaped = "line1\nline2\t\"quoted\" \\ backslash"
```

Strings are double-quoted with backslash escapes (`\n`, `\t`, `\r`, `\"`,
`\\`, and friends). They are immutable UTF-8.

The **interpolated string** form prefixes the quote with `$` and embeds
expressions in `{ ... }` islands:

```checkmate
str report = $"hp={hp} pos=({p.x},{p.y}) next={fib(7)}"
```

Interpolation is covered in [Strings](../language/strings.md). Single-quoted
character literals appear only inside megaprogramming pattern code (for
character classes), not in ordinary Checkmate expressions.

## Boolean literals

`true` and `false`. They are values of type `bool` — not numbers, and not
coercible to or from `int`.

## Boundary capitalization

Identifier case encodes the boundary contract of §2.5:

| Kind | Convention | Examples |
| --- | --- | --- |
| Script-internal types, functions, variables, fields, parameters | `camelCase` | `vec2`, `spawnZombie`, `loadTexture` |
| Boundary declarations — schema types, capabilities, interfaces, interface members, impl targets | `PascalCase` | `TextureHandle`, `LoadTexture`, `engine.gamemode` |

When a [schema](../schema/overview.md) is active, this is **enforced at
compile time**: a capitalized declaration that is not part of the active
schema contract is an error, and a boundary type or function referenced in
lowercase is an error. With no schema registered the checker keeps the
pre-schema behavior and does not enforce the split — but follow the
convention; the gate will arrive with your schema.

Enum *variant* names are PascalCase payload identifiers (`Damage`,
`Some`, `Ok`) while their payload fields are camelCase; this is a
[language convention](../language/enums.md) independent of schemas.

## Newlines at the token level

Newlines are **significant**: they delimit statements and top-level
declarations. The precise statement-boundary rules — when an expression
continues, what happens inside brackets — live in
[Significant Newlines](../language/newlines.md). The one-line summary:

- A newline after a token that can end a statement ends the statement.
- A newline after a binary operator or an open parenthesis does not.
- Inside `[ ... ]` and `{ ... }` collection literals and `match` bodies,
  newlines separate elements/arms — they are delimiters, not whitespace.
