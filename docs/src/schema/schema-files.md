# Schema Files

A schema file is a `.cm` file rooted by a `schema` declaration. **One file
represents exactly one namespace root** — a host providing multiple systems
ships distinct files (`engine.cm`, `physics.cm`, `ui.cm`).

## Anatomy

```checkmate
// File: schemas/engine.cm
schema engine 1.4.0

struct TextureHandle {
    int id
}

enum OrderEvent {
    Started
    Checkout(int total)
}

capability graphics {
    since 1.0.0 TextureHandle LoadTexture(str path)
    since 1.0.0 void DrawTexture(TextureHandle tex, vec2 position)
}

interface gamemode requires core {
    since 1.0.0 GameState InitGame(GameConfig config)
    since 1.4.0 optional void InvalidateSession(str token)
}
```

- The header is `schema <namespace> <X.Y.Z>` — the namespace root and the
  schema's own version.
- Everything declared in the file is relative to that root: `graphics`
  here is `engine.graphics` everywhere else.
- Comments and blank lines are ordinary; declarations are
  newline-delimited like the rest of the language.

## What a schema file may declare

| Declaration | Role |
| --- | --- |
| `struct Name { fields }` | Boundary type: shared interchange layout (§9.3) |
| `enum Name { variants }` | Boundary type: tagged union with payloads |
| `capability name { members }` | Host-provided functions the script calls |
| `interface name { members }` | Script-implemented functions the host calls |

Contract members have the shape:

```text
since X.Y.Z [optional] [suspend] ReturnType MemberName(paramType paramName, ...)
```

- `since` tags the version the member was introduced (§9.5). Members
  without a `since` tag belong to the schema's floor.
- `optional` marks an **interface** member a mod may skip (§9.5).
  `optional` on a *capability* member is a schema-authoring error — the
  host must always provide capability members.
- `suspend` marks future async members; the current implementation parses
  it and rejects it with a pointed diagnostic (the §4 async system is not
  built yet).

## Boundary types

Structs and enums declared in schemas are **boundary modules**: they are
automatically boundary elements, must be PascalCase, and define the data
that crosses the FFI boundary in both directions.

- They join the script's type registry at check time, so scripts can
  declare variables of them, construct them, and match them like local
  types.
- They are **non-generic** — interchange layouts are concrete.
- The generated bindings map them to native Rust structs/enums and C
  pack/unpack helpers, by field name; nested boundary types compose.
- Boundary type names are unique **across namespaces** — the script type
  space is flat, so `engine.Sprite` and `ui.Sprite` cannot coexist.

## Option and result everywhere a type appears

The §2.8 built-in sum types are first-class in schemas: member returns,
member parameters, struct fields, enum payloads, array elements, and map
key/value types may all be `option<T>` or `result<T, E>`.

```checkmate
capability pricing {
    since 0.1.0 result<Price, str> GetPrice(str sku)
    since 0.1.0 option<Price> PeekPrice(str sku)
}
```

The arities are exact and validated at schema-parse time (`option` takes
one type argument, `result` two). The generated Rust bindings map them to
`Option<T>` / `Result<T, E>`; the generated C header ships one typed
pack/unpack helper pair per distinct shape (see
[Embedding in C](../embedding/c.md#generated-schema-headers)).

## Naming

- The namespace is lowercase (camelCase style, e.g. `engine`, `modBridge`
  for a sub-capability path).
- Types and contract members are **PascalCase**.
- Capability and interface *names* are case-free in practice (every
  whitepaper path is camelCase), while declared types and members are
  PascalCase-enforced.

## Validating and registering schemas

```sh
cme schema schemas/engine.cm        # validate one file
cme check --schema schemas/engine.cm program.cm
cme run --schema schemas/engine.cm --schema schemas/physics.cm mod_dir/
```

`--schema` is repeatable; in mod mode the manifest's `[schemas]` table
narrows the grant further (see
[Versioning](../schema/versioning.md#target-versions)).

On the host side:

- **Rust**: `engine.load_schema_file("schemas/engine.cm")`, or the
  `cme_schema_bindings!` macro embedding the same file at build time.
- **C**: `cm_schema_parse` / `cm_schema_parse_file`, then
  `cm_engine_register_schema`.

Registration re-validates the **set**: duplicate namespaces,
cross-namespace type collisions, and unresolved `requires` edges fail the
registration and leave the engine unchanged.

## Schema-authoring defects are compile errors

The schema front end rejects defective contracts, among them:

- a member tagged `since` beyond its own schema's version (it could never
  be visible — a forgotten version bump),
- `optional` on a capability member,
- an interface `requires` edge pointing at nothing,
- the retired `v1.4.0` spelling (one migration diagnostic points at it),
- duplicate member names within a contract,
- an `option` / `result` type with the wrong number of type arguments
  (`option` takes exactly one, `result` exactly two).

A defect in the schema is a *host-side* problem — it fails validation at
registration/`cme schema` time, before any script is checked against it.
