# `requires` Edges and Versioning

Schemas evolve. This page covers the two mechanisms that keep evolution
safe: **dependency edges** (`requires`) and **version gates**
(`since`, `optional`, target versions).

## `requires`: contract prerequisites

The `requires` keyword enforces contract prerequisites at compile time,
in two forms:

**Capability requires interface:**

```checkmate
since 1.0.0 capability network {
    requires auth
    httpResponse Send(httpRequest request)
}
```

A script cannot import **or call** `engine.network.*` unless its mod
provides a complete implementation of the `auth` interface. The gate sits
on the import statement: fail `impl auth` and `import engine.network`
fails with it.

**Interface requires interface:**

```checkmate
since 1.0.0 interface gamemode requires core {
    GameState InitGame(GameConfig config)
}
```

A mod cannot implement `gamemode` unless it also fully implements
`core`. Completeness is checked for the transitive closure of
prerequisites.

**Cross-namespace prerequisites** use qualified paths:

```checkmate
interface hud requires ui.widgets {
    // ...
}
```

Both spellings of the clause are accepted — after the name, or first
inside the body:

```checkmate
since 1.0.0 interface gamemode {
    requires core
    GameState InitGame(GameConfig config)
}
```

## Versioning: `since`, `optional`, and target versions

### `since` tags

`since X.Y.Z` opens a capability or interface block and versions every
member it contains — the version when they were introduced. The same
contract may appear in several blocks (one per version band); their
members union in declaration order. The tag is load-bearing: it lets old
programs keep compiling against newer schemas by *hiding* what they
predate.

### Target versions

A program's **target version** for a namespace comes from one of:

- the mod manifest's `[schemas]` table:

  ```toml
  # mod.toml
  [schemas]
  engine = "1.4.0"
  physics = "1.0.0"
  ```

- or a host API/CLI registration when no manifest narrows it.

The compiler **hides** all capability and interface members introduced in
versions newer than the target version. Hidden members cannot be called,
and interface members hidden this way are not required — a mod targeting
`engine 1.2.0` never sees the 1.4.0 `InvalidateSession` at all.

No namespace may be targeted above the schema's own version, and the
grant is **narrowed** by the manifest: unlisted namespaces are invisible.
Registering the schema with the host without a `[schemas]` table grants
everything the host registered (pre-schema-style loose sources keep the
older behavior).

### `optional`: non-breaking interface additions

Adding a **required** interface member breaks every existing mod that
does not implement it. Marking the new member `optional` avoids the
break:

```checkmate

since 1.0.0 interface auth {
    bool ValidateToken(str token)
}

since 1.4.0 interface auth {
    optional void InvalidateSession(str token)
}
```

- Mods targeting a version before 1.4.0 never see it (hidden).
- Mods targeting ≥ 1.4.0 may implement it or skip it — completeness
  passes either way.
- Hosts calling an optional member the mod skipped get a deterministic
  "not implemented" error (an `UnknownEntry`-family failure), never a
  silent no-op.

`optional` is for **interfaces only**; on a capability it is a schema
defect.

## The forgotten-bump check

The schema set build rejects a member tagged `since` **beyond its own
schema's version** — such a member could never be visible to anyone, and
its presence means someone forgot to bump the schema header. The defect
is reported at registration (or `cme schema` validation), with a pointed
diagnostic.

## Version negotiation in practice

1. Schema author ships `engine 1.5.0` with a new capability block
   `since 1.5.0 capability engine { ... }` and a new `optional` interface
   member in a `since 1.5.0` block.
2. Old mods (target `1.4.0`) recompile unchanged: the new member is
   hidden; the optional member is invisible.
3. A mod that wants the new API bumps its `[schemas]` entry to
   `1.5.0` — the compiler now *requires* any new non-optional members
   and *allows* the optional one.
4. The host, registering `engine 1.5.0`, still serves mods targeting
   older versions: visibility is per-mod, derived from its manifest.

## Summary table

| Mechanism | Declares | Effect on scripts |
| --- | --- | --- |
| `requires` (capability → interface) | prerequisite | import + call gated on full interface implementation |
| `requires` (interface → interface) | prerequisite | implementing one demands implementing the other |
| `since X.Y.Z` | block introduction version | members of the block hidden for older targets |
| `optional` | skippable interface member | completeness passes without it |
| `[schemas]` target | per-mod visibility | grant narrowed to listed namespaces at listed versions |
