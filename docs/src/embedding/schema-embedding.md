# Schema-Driven Embedding

The schema system's payoff: declare the host contract **once** in a
`.cm` file, and both sides get compile-time-verified bindings. This page
walks the full loop for Rust hosts (the `cme_schema_bindings!`
procedural macro) and C hosts (`cme codegen-c` generated headers).

## The workflow

```text
                schemas/engine.cm  (single source of truth)
                     │                │
      script side    │                │    host side
                     ▼                ▼
  cme check --schema …      Rust: cme_schema_bindings!   C: cme codegen-c
  gates imports, calls,     → capability trait           → typedefs + vtable
  impl completeness,        → typed proxies              → REGISTER macro with
  versions                  → boundary types               _Static_asserts
                            → runtime descriptor         → pack/unpack helpers
```

Script-side gating (imports, capability-call signatures, `impl`
completeness, version hiding) is covered in
[Capabilities and Interfaces](../schema/contracts.md); this page is the
host side.

## Rust hosts: `cme_schema_bindings!`

Enable the facade's `schema-macro` feature and point the macro at your
schema file. The macro runs the **real schema front end** at your
crate's compile time and generates:

- a **capability trait** per capability — the host implements it, and
  every signature is verified **when your crate builds**;
- **capability provider bridges** registering the implementation with
  the engine;
- **typed interface proxies** — typed host → script calls into the
  script's `impl` members;
- **native structs/enums** for the §9.3 boundary types, packing and
  unpacking script values by name;
- the runtime **descriptor** (`SchemaFile`) so the same file registers
  with the engine without re-parsing.

### The schema

```checkmate
// schemas/game.cm
schema game 1.0.0

struct Sprite {
    int id
    str name
    float scale
}

enum Event {
    Started
    Scored(int points)
}

capability window {
    since 1.0.0 Sprite OpenWindow(str title)
    since 1.0.0 void Draw(Sprite sprite)
}

interface gamemode {
    since 1.0.0 int OnEvent(Event event)
    since 1.0.0 int Tick(int frame)
}
```

### Generating and implementing

```rust
use cme::{Engine, ExecutionLimits, Value};

pub mod bindings {
    // Runs the REAL schema parser over schemas/game.cm at THIS crate's
    // compile time. Generated code references the facade's api re-export.
    cme::cme_schema_bindings!(path = "schemas/game.cm", crate = ::cme::api);
}

use bindings::{Event, Sprite};

// The host implements the generated trait — a missing member or a wrong
// signature is a compile error of the HOST crate:
struct WindowService;

impl bindings::GameWindowCapability for WindowService {
    fn open_window(&self, title: String) -> Sprite {
        Sprite { id: 7, name: title, scale: 1.0 }
    }

    fn draw(&self, sprite: Sprite) {
        // drive your renderer
    }
}
```

> Trait/method naming follows the schema (namespace `game`, capability
> `window` ⇒ `GameWindowCapability`; members map to snake_case methods).
> The macro's generated names are stable per schema path; the compiler
> will name them precisely for you when you `impl`.

### Wiring the engine

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::new();

    // Register the contract — the descriptor from the SAME file, no
    // re-parse, no drift between what the macro verified and what gates
    // the load:
    engine.register_schema(bindings::schema())?;

    // Register the provider through the generated bridge:
    bindings::register_game_window(&mut engine, std::sync::Arc::new(WindowService))?;

    // Load a schema-gated program (same gate as the CLI):
    let program = engine.load_file("scripts/hud.cm")?;
    let context = engine.create_context(&program, ExecutionLimits::default());
    // ... invoke, as in the embedding guide ...
    Ok(())
}
```

### Host → script calls through typed proxies

The script implements the interface:

```checkmate
// scripts/hud.cm (schema game registered, target 1.0.0)
impl game.gamemode {
    int OnEvent(Event event) {
        match (event) {
            Scored(int points) => { return points * 10 }
            Started() => { return 0 }
        }
    }

    int Tick(int frame) {
        return frame + 1
    }
}
```

The host calls it through the generated proxy — typed arguments, typed
result, and construction **fails** when the loaded program never
implemented the interface:

```rust
// Either shape works:
let gamemode = bindings::GameGamemodeProxy::new(&context)?;
let gamemode = context.get_interface::<bindings::GameGamemodeProxy>()?;

let next = gamemode.tick(41)?;                  // Ok(42): typed, no manual Values
let points = gamemode.on_event(Event::Scored(5))?;  // Ok(50): the enum payload
                                                 // crossed to the script and back
```

`Event::Scored(5)` is the macro-generated native enum — packed by name
into the boundary layout, matched by the script's `match (event)`,
returned payloads unpacked back into Rust types. Boundary structs work
the same way, field by name, recursively for nested types.

### Where the compile-time verification bites

| Defect | Caught |
| --- | --- |
| Host capability member missing | host crate **compile error** (unimplemented trait) |
| Host capability signature wrong | host crate **compile error** (`_Generic`-style trait check) |
| Script calls an unregistered capability | **load failure** (provider-presence check) |
| Script signature drifts from schema | **script compile error** |
| Program never implements an interface the host proxies | proxy **construction error** (`UnknownEntry`) |
| Schema file defective | host **build error** (macro runs the real parser) |

Every layer has a deterministic, positioned failure — none waits for a
mysterious runtime misbehavior.

## When your IDE caches stale bindings (`SCHEMAS_HASH`)

The macro reads `schemas/*.cm` from disk at compile time, so a plain
`cargo build` re-expands the bindings whenever the file's *content*
changes the build inputs. But some IDEs — RustRover is the reported
case — cache a procedural macro's expansion keyed on its **input
tokens**. `cme_schema_bindings!("schemas/origout.cm")` has tokens that
never change when you edit the schema file, so the IDE keeps showing the
stale expansion: members you added are "missing", types you renamed
still exist, and the errors vanish the moment a real build runs.

The fix is to make the macro's input tokens depend on the schema
contents. Pin a hash of every schema file into the invocation with a
`#[doc]` attribute:

```rust
pub mod bindings {
    #[doc = env!("SCHEMAS_HASH")]
    cme::cme_schema_bindings!(path = "schemas/origout.cm", crate = ::cme::api);
}
```

and produce that environment variable with a `build.rs` in the same
crate:

```rust
// build.rs
use std::fs;
use std::path::Path;

fn main() {
    let schema_dir = Path::new("schemas");

    let mut entries: Vec<_> = fs::read_dir(schema_dir)
        .expect("failed to read schemas/ dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "cm"))
        .map(|e| e.path())
        .collect();

    entries.sort();

    let mut hash: u64 = 0;
    for path in &entries {
        let contents = fs::read(path).unwrap_or_else(|_| panic!("failed to read {path:?}"));
        for &b in &contents {
            hash = hash.wrapping_mul(31).wrapping_add(b as u64);
        }
        for b in path.to_string_lossy().bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(b as u64);
        }

        println!("cargo:rerun-if-changed={}", path.display());
    }

    println!("cargo:rerun-if-changed={}", schema_dir.display());

    println!("cargo:rustc-env=SCHEMAS_HASH={:x}", hash);
}
```

Two things now happen on every schema edit:

1. **cargo rebuilds the host crate.** The `cargo:rerun-if-changed` lines
   make the build script (and therefore the crate) re-run when any
   `schemas/*.cm` file changes, and the changed `SCHEMAS_HASH` value
   invalidates the previous compilation.
2. **the IDE re-expands the macro.** `#[doc = env!("SCHEMAS_HASH")]`
   is part of the invocation's tokens; when the hash changes, the
   token stream the IDE feeds to its macro-expansion cache changes with
   it, and the cache misses — so the bindings are regenerated from the
   file that is now on disk.

The recipe also works for **multiple schema usages**: with several
`cme_schema_bindings!` invocations (one per namespace), add the same
`#[doc = env!("SCHEMAS_HASH")]` line on top of each. One `build.rs`
hashes the whole `schemas/` directory, so every invocation sees the same
variable and every one of them invalidates together — editing any single
schema file refreshes all of the bindings.

> The hash is not a security boundary and its algorithm is unspecified —
> it only needs to change when the schema files change. Keep the doc
> attribute on its own line directly above the macro invocation, and
> keep the `schemas/` directory layout stable so the file list (and
> therefore the hash) is deterministic. rustdoc never renders a doc
> attribute placed on a macro invocation and emits an
> `unused_doc_comments` note saying so — that is expected (the
> attribute's only job is to perturb the token stream); the note can
> only be silenced from an enclosing scope, not by a sibling attribute,
> so add `#![allow(unused_doc_comments)]` at the top of the file if your
> build treats warnings as errors. The same recipe applies unchanged to
> the [`cme_schema_setup!`](#the-one-macro-quick-start-cme_schema_setup)
> invocation.

## The one-macro quick start: `cme_schema_setup!`

The manual flow above — generate bindings, register the schema, wire
providers, load the program, create a context under limits, construct
proxies — is the right surface for advanced hosts because every step is
a decision you can steer. For a first embedding it is a lot of ceremony.
`cme_schema_setup!` does all of it in one invocation, with the manual
flow left untouched underneath:

```rust
pub mod bindings {
    cme::cme_schema_setup! {
        schema = "schemas/origout.cm",
        program = mod "checkmate",
        proxy = OrigoutTestProxy,
    }
}

fn main() {
    let host = bindings::Host::new().expect("checkmate host setup failed");
    host.run(|run| {
        println!("{}", run.origout_test.cow().unwrap());
        // `run` derefs to the context, so plain calls work too:
        let result = run.invoke("main", &[]).unwrap();
        println!("{result}");
    });
}
```

### The settings

| Setting | Value | Notes |
| --- | --- | --- |
| `schema` | `"schemas/app.cm"` | Repeatable — one bindings module is generated per namespace, and all are registered by `Host::new`. |
| `program` | `mod "checkmate"`, `file "scripts/hud.cm"`, or `source "int main() { … }"` | Required. Which `Engine::load_*` call the setup performs; relative paths resolve at RUNTIME against the process working directory. |
| `crate` | `::cme::api` | Optional; the crate the generated code references. Defaults to the facade's `api` re-export. |
| `limits` | `{ fuel: 1_000_000, deadline_ms: 50, max_call_depth: 64 }` | Optional; any subset of the keys, `none` allowed for `fuel`/`deadline_ms`. Omitted keys keep the API defaults (§5.5). |
| `proxy` | `OrigoutTestProxy` or `namespace.Proxy as field` | Repeatable. Unqualified names must be unique across the schemas. The session field is the alias, or the type's snake_case without its `Proxy` suffix. |
| `provider` | `window => MyService` or `namespace.capability => MyService` | Repeatable. The expression is wrapped in `Arc::new(...)` and handed to the generated registration bridge — pass the service itself, not an `Arc`. |

### What the expansion defines

The macro emits, at the invocation site:

- one **bindings module per schema namespace** — byte-for-byte the
  `cme_schema_bindings!` output (traits, proxies, boundary types,
  descriptor), so everything on
  [the manual page](#rust-hosts-cme_schema_bindings) still applies;
- **`Host`** — owns the engine, the compiled program, and the limits.
  `Host::new()` registers every schema, wires every `provider`, and
  loads the `program` through the same schema-gated pipeline the CLI
  uses, returning `Result<Host, HostError>`; `engine()`, `program()`,
  `limits()`, and `context()` expose the pieces for advanced use;
- **`HostError`** — the setup's failure modes (`Schema`, `Provider`,
  `Compile`, `Io`), each rendering the underlying API error;
- **`Session`** — one execution pass: the `context` field (plus every
  requested proxy as a named field). `Session` derefs to the context.

`host.run(|session| …)` builds the context, constructs every proxy, and
hands the session to your closure; a proxy whose interface the program
never implemented panics with the rendered error. `host.try_run(…)`
returns that failure as `Err(ExecutionError)` (kind `UnknownEntry`)
instead — the same host-side §10.4 guard the manual flow gives you.

### Generated bindings and the setup macro: pick one

`cme_schema_setup!` already generates the bindings for every `schema`
you list. Invoking `cme_schema_bindings!` for the same file in the same
module would define the namespace module twice — keep one macro per
schema per module, and note that the
[`SCHEMAS_HASH` recipe](#when-your-ide-caches-stale-bindings-schemas_hash)
applies to `cme_schema_setup!` invocations identically.

## C hosts: `cme codegen-c`

Generate the header once per schema (CI-friendly; the output is
byte-deterministic):

```sh
cme codegen-c schemas/engine.cm > engine_schema_gen.h
```

The generated header provides, per namespace:

1. **Function-pointer typedefs** per capability member, plus a
   **vtable** struct;
2. a **registration macro** — `CME_<NS>_<CAP>_REGISTER(engine, path, ...)`
   — whose `_Static_assert(_Generic(...))` lines verify every host
   implementation's signature **at compile time**; a missing member
   fails the preprocessor, a wrong signature fails the assert;
3. **exact-arity interface invocation helpers** for host → script calls;
4. **pack/unpack helpers** for boundary types over the `cm_value_t` ABI
   (by-name fields; `_pack` takes the host value **by address** so
   nested schema types compose recursively; `str` fields unpack to
   owned `char*` freed with `cm_string_free`; enums are tagged unions
   with per-variant payload structs dispatched through
   `cm_value_enum_variant`).

The header is multi-include-safe and several headers coexist in one
translation unit (one per namespace).

### The host implementation

```c
/* engine_schema_gen.h was generated from schemas/engine.cm */
#include "engine_schema_gen.h"

static cm_value_t* host_load_texture(void* user, cm_value_t* const* args,
                                     size_t argc, cm_error_t* err) {
    /* ... */
    return cm_value_int(1);
}

static cm_value_t* host_draw_texture(void* user, cm_value_t* const* args,
                                     size_t argc, cm_error_t* err) {
    /* ... */
    return cm_value_void();
}

/* The REGISTER macro wires the vtable AND compile-time-verifies the
   two implementations above — signatures come from the schema. */
CME_ENGINE_GRAPHICS_REGISTER(my_engine, "engine.graphics",
                             host_load_texture, host_draw_texture);
```

### Interface calls with exact arity

```c
/* The script's `impl engine.gamemode` becomes an exact-arity helper:
   arguments are plain cm_value_t*, the result is a future as usual. */
cm_future_t* f = CME_ENGINE_GAMEMODE_CALL_INITGAME(ctx, &config_value);
```

### Schema structs and enums through the typed layer

```c
/* Pack a host struct into a script value — by address, recursive: */
EngineGameState state = { .score = 100, .active = true };
cm_value_t* v = NULL;
CME_ENGINE_GAMESTATE_PACK(&state, &v);
/* ... pass v as an argument or unpack it back ... */

/* Unpack a script-built struct — by name: */
EngineGameState parsed;
CME_ENGINE_GAMESTATE_UNPACK(script_value, &parsed);

/* Enums are tagged unions with per-variant payloads: */
cm_value_enum_variant(event_value, &variant);   // variant + payload struct
```

The generated layer is not a demo: `cme-ffi`'s build generates it from
`apps/c_host/engine.cm` and compiles the C host against it, so
`cargo test` exercises the full flow (a schema defect fails the build;
a wrong-signature provider fails `_Generic`).

## Mods + schemas: narrowing the grant

A mod's `[schemas]` table narrows what it may see:

```toml
# my_mod/mod.toml
name = "my_mod"
version = "1.0.0"
checkmate_version = "0.3.0"

[schemas]
shop = "1.0.0"
```

Loading that mod grants **only** `shop` **at 1.0.0** — the host must
have registered `shop` at ≥ 1.0.0 or the load fails; members added
`since` a later version stay hidden. Loose-file loads (no manifest)
inherit everything the host registered. Full semantics:
[`requires` and Versioning](../schema/versioning.md).

## When to use generated bindings vs. raw providers

| Situation | Use |
| --- | --- |
| Stable contract, evolving team | Generated bindings — the compiler is your reviewer |
| Quick experiment / ad-hoc capability | Raw `CapabilityProvider` (Rust) or `cm_capability_member_t` (C) |
| Multiple schemas, multiple namespaces | Generated bindings per namespace; headers coexist |
| Contract changes often | Generated bindings — regeneration surfaces every drift at build time |

The raw provider surface never goes away — generated bindings are a
layer over it, not a replacement.
