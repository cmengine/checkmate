# Capabilities and Interfaces

Two contract directions, two gate sets. This page gives each side's
exact rules as the compiler enforces them.

## Capabilities: script calls, host provides

```checkmate

since 1.0.0 capability graphics {
    TextureHandle LoadTexture(str path)
    void DrawTexture(TextureHandle tex, vec2 position)
}
```

A script that wants to call `engine.graphics.LoadTexture` must:

1. **Import the capability**:

   ```checkmate
   import engine.graphics
   ```

2. **Call through the full path** with the schema's parameter names
   available as named arguments:

   ```checkmate
   TextureHandle tex = engine.graphics.LoadTexture(path: "hero.png")
   ```

3. **Have a provider registered on the host** for `engine.graphics`. The
   provider-presence check runs at **load time**: a program calling a
   capability with no registered provider fails the load. Wiring mistakes
   are deterministic before any invocation.

Capability calls type-check against the schema signatures — wrong
argument types, wrong counts, or unknown members are compile errors.
Calls dispatch to the host provider at runtime through an
engine-agnostic seam ([Embedding Overview](../embedding/overview.md)).

### Capability members are never optional

`optional` is defined for *interface* members a mod may skip. A
capability member declared `optional` is a schema defect: the host
promises its capabilities whole.

## Interfaces: script implements, host calls

```checkmate

since 1.0.0 interface gamemode {
    GameState InitGame(GameConfig config)
    void OnTick(GameState state, float deltaTime)
}
```

A script satisfies an interface with an
[`impl` block](../mods/impl-union.md):

```checkmate
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }

    void OnTick(GameState state, float deltaTime) {
        // per-tick logic
    }
}
```

The compiler enforces:

- **Completeness** — every required visible member must be implemented.
  `optional` members may be skipped without breaking the implementation.
- **Exact signatures** — parameter types and return types must match the
  schema exactly. Parameter *names* are free (they are call-site sugar,
  and capability-side named arguments bind against the schema's names).
- **PascalCase members** — interface members are boundary declarations.
- **Visibility gating** — members introduced `since` a version newer than
  the program's target version are hidden and not required (§9.5).
- **No silent skips** — an impl target naming an **ungranted** or
  **unknown** namespace is an error, anchored at the impl site. An impl
  is a claim; claims get validated.

## The two gates on capability *imports*

§9.4's "cannot import or call" rule gives interfaces teeth over
capabilities:

```checkmate
capability network {
    requires auth
}

since 1.0.0 capability network {
    httpResponse Send(httpRequest request)
}

since 1.0.0 interface auth {
    bool ValidateToken(str token)
}
```

1. **The capability–import gate**: `import engine.network` compiles only
   if the mod completely implements the `auth` interface. A partial or
   missing `impl auth` makes the *import itself* fail.
2. **The capability–call gate**: `engine.network.Send(...)` additionally
   requires the import (as always).

Interface-to-interface prerequisites work the same way one level up:
`interface gamemode requires core` means a mod cannot implement
`gamemode` unless it also fully implements `core`.

Full details and version interactions:
[`requires` Edges and Versioning](../schema/versioning.md).

## The three-gate checklist

For any boundary call in your program, three gates must pass at compile
time:

| Gate | Checked at | Failure mode |
| --- | --- | --- |
| Import gate | compile (per import statement) | missing/failed `requires` prerequisites, ungranted namespace |
| Call gate | compile (per call site) | signature mismatch, unknown member, version-hidden member, missing import |
| Provider gate | **load** (per engine) | no registered provider for the capability path |

Script-side errors stop the build; provider-side errors stop the load;
neither ever waits for an invocation to discover them.
