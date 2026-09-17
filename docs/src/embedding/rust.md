# Embedding in Rust

This is the complete guide to hosting Checkmate scripts from a Rust
application: adding the dependency, loading programs, invoking under
limits, unpacking values, handling errors, providing capabilities, and
running schema-gated mods.

## Adding the dependency

Hosts consume the workspace through the `cme` facade crate with the
`api` feature (and `schema-macro` for generated bindings):

```toml
[dependencies]
cme = { path = "/path/to/checkmate", features = ["api"] }
# or, once published/versioned in your workspace:
# cme = { git = "https://github.com/cmengine/checkmate", features = ["api"] }
```

The facade re-exports the whole host surface flat:

```rust
use cme::{
    Engine, ExecutionLimits, CompiledProgram, Context, Value,
    MAX_CALL_DEPTH,
};
// and error types:
use cme::api::{ErrorKind, ExecutionError};
```

> The default build of `cme` intentionally exposes **no** APIs — features
> gate everything. `api` is the host feature; `schema-macro` adds
> `cme_schema_bindings!` (see
> [Schema-Driven Embedding](schema-embedding.md)).

## Your first embedding

```rust
use cme::{Engine, ExecutionLimits, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::new();

    // 1. Load: expansion → parse → import check → type check.
    let program = engine.load_source("int main() {\nreturn 40 + 2\n}\n")?;

    // 2. Create a context under §5.5 limits.
    let context = engine.create_context(&program, ExecutionLimits {
        fuel: Some(1_000_000),      // deterministic operation budget
        deadline_ms: Some(50),      // wall-clock cap, checked at safepoints
        max_call_depth: 64,         // recursion bound
    });

    // 3. Invoke a top-level function.
    let answer = context.invoke("main", &[])?;
    assert_eq!(answer, Value::Int(42));
    Ok(())
}
```

That is the whole loop. Every step after this section is detail.

> Embedding **with a schema contract** and want all of it — bindings,
> registration, program load, context, proxies — from one invocation?
> The [`cme_schema_setup!`](schema-embedding.md#the-one-macro-quick-start-cme_schema_setup)
> quick-start macro collapses the flow; this page remains the
> advanced-user surface it automates.

## The `Engine`: compiling programs

`Engine` is the host's handle. It starts empty — no schemas, no
providers — which keeps the pre-schema behavior (host-rooted imports are
accepted as host-style paths; nothing is schema-gated).

```rust
use cme::Engine;

let mut engine = Engine::new();
```

### Three ways to load

```rust
// From a source string. Errors: CompileError (rendered diagnostics).
let program = engine.load_source(source_text)?;

// From a .cm file. I/O failures surface as LoadError::Io;
// diagnostics render as "path:line:column: message".
let program = engine.load_file("scripts/combat.cm")?;

// From a whole §10 mod (a directory holding mod.toml + src/, or the
// mod.toml path itself). Modules expand individually, link into one
// program, and every compile diagnostic re-anchors to its owning module:
let program = engine.load_mod("mods/survival_mode")?;
```

All three apply the full gate (expansion → parse → standalone-import
check → type check, plus provider presence when capabilities are called):
**a program that produced any diagnostic never becomes invocable.**

### Program metadata

```rust
// Top-level functions, in declaration order — §2.1 entry candidates.
for name in program.entry_points() {
    println!("entry: {name}");
}

// §10.4 impl targets the program implements ("engine.gamemode", ...).
for target in program.interface_targets() {
    println!("implements: {target}");
}

// Mod builds: the parsed manifest (name, version, [schemas] targets).
if let Some(manifest) = program.manifest() {
    println!("mod {} v{}", manifest.name, manifest.version);
}
```

Programs are **immutable and `Send + Sync`**: compile once, share across
threads, create as many contexts as you like.

### Registering schemas

```rust
// Parse + register a schema file (§9). The whole registered set is
// re-validated: duplicate namespaces, cross-namespace type collisions,
// and unresolved requires edges fail registration, leaving the engine
// unchanged.
engine.load_schema_file("schemas/engine.cm")?;

// Or from text:
engine.load_schema_text(schema_source, "engine.cm")?;

// After registration, every subsequent load is schema-gated:
println!("{:?}", engine.schema_namespaces());   // ["engine"]
```

Schema registration order matters: schemas gate loads that happen
**after** registration. See [The Schema System](../schema/overview.md).

## The `Context`: invoking under limits

```rust
use cme::{ExecutionLimits, MAX_CALL_DEPTH};

let limits = ExecutionLimits {
    fuel: Some(1_000_000),       // None = unmetered; Some(0) exhausts immediately
    deadline_ms: Some(50),       // None = no deadline; Some(0) expires immediately
    max_call_depth: 64,          // 0 = engine default (MAX_CALL_DEPTH = 1024)
};

let context = engine.create_context(&program, limits);
```

- The context **borrows** the program (`create_context(&program, ...)`)
  — the borrow checker enforces "program outlives contexts".
- Contexts are **cheap handles**: each invocation constructs its own
  interpreter frame, fuel cell, and deadline.
- Contexts are `Clone`; a clone is the same logical context (it shares
  the §5.7 reentrancy identity).
- The context snapshots the currently registered capability providers;
  registering providers later affects only future contexts.

### Two invocation forms

```rust
// Top-level function by name:
let result = context.invoke("fib", &[Value::Int(10)])?;

// §10.4 impl member by target path + member:
let state = context.invoke_member(
    "engine.gamemode",   // the impl target ("engine.gamemode")
    "OnTick",            // the member
    &[state_value, Value::Float(0.016)],
)?;
```

Arguments bind **by value** (cloned into the script — the host's values
are never aliased). Results come back as `Value`s.

### Interface proxies (typed host → script calls)

`has_interface` / `get_interface` support the schema-generated proxy
flow — construction fails with `UnknownEntry` when the program never
implemented the interface:

```rust
if context.has_interface("engine.gamemode") {
    // With generated bindings (see Schema-Driven Embedding):
    // let gamemode = context.get_interface::<EngineGamemodeProxy>()?;
    // let state = gamemode.init_game(config)?;   // typed args + result
}
```

## `Value`: building and unpacking

```rust
use cme::Value;

// Building — From impls cover the scalars:
let a = Value::Int(42);
let b: Value = 2.5.into();
let c: Value = "hello".into();
let d = Value::Bool(true);

// Unpacking — as_* accessors return Option:
match context.invoke("describe", &[a.clone()])? {
    Value::Str(text) => println!("{text}"),
    other => println!("CMON: {other}"),   // Display renders canonical CMON (§11.1)
}

let n = a.as_int();        // Some(42)
let f = b.as_float();      // Some(2.5)
let s = c.as_str();        // Some("hello")
let xs = some_array.as_array();   // Option<&[Value]>
let m = some_map.as_map();        // Option<&[(Value, Value)]>
```

Composite values carry structure:

```rust
// Value::Struct { name, fields: Vec<(String, Value)> }
// Value::Enum   { name, variant, payloads: Vec<Value> }
// Value::Array(Vec<Value>) / Value::Map(Vec<(Value, Value)>) / Value::Void
```

`Value` implements `PartialEq` (structural, mirroring script `==`) and
`Display` (the canonical CMON rendering — the exact text `cme run`
prints).

## Errors: two surfaces

**Compile failures** — `CompileError` (from `load_source`/`load_mod`) or
`LoadError` (from `load_file`, which adds an `Io` variant):

```rust
match engine.load_file("scripts/broken.cm") {
    Ok(program) => { /* ... */ }
    Err(err) => {
        // rendered, positioned diagnostics; prints like the CLI
        eprintln!("{err}");
    }
}
```

**Execution failures** — `ExecutionError`:

```rust
match context.invoke("main", &[]) {
    Ok(value) => { /* ... */ }
    Err(err) => {
        eprintln!("kind: {:?}", err.kind);   // ErrorKind::*
        eprintln!("message: {}", err.message);
        // Position (mod builds re-anchor to the owning module):
        if let Some(file) = &err.file {
            eprintln!("at {file}:{}:{}", err.line, err.column);
        }
        // One stable shape for logs:
        eprintln!("{}", err.render());       // "file:line:col: message"
    }
}
```

`ErrorKind` — the coarse families:

| Kind | Meaning |
| --- | --- |
| `Runtime` | Script failure: overflow, division by zero, out-of-bounds, missing key |
| `Budget` | Fuel exhausted |
| `Deadline` | Wall-clock deadline passed at a safepoint |
| `CallDepth` | Call-depth limit hit |
| `UnknownEntry` | Invoked function/impl member does not exist |
| `Reentrant` | A capability re-entered this context mid-invocation (§5.7) |

`UnknownEntry` errors carry **no position**; limit-family and runtime
errors carry the failure's span (re-anchored for mod builds).

## Providing capabilities

A capability is a function set the *script calls* and the *host
provides*. Implement the `CapabilityProvider` trait and register it per
capability path:

```rust
use std::sync::Arc;
use cme::api::CapabilityProvider;
use cme::{Engine, Value};

#[derive(Debug)]
struct Graphics {
    // your engine state (textures, device handles, ...)
}

impl CapabilityProvider for Graphics {
    // `member` is the schema member name; args are positional Values in
    // schema declaration order, cloned out of the script.
    fn call(&self, member: &str, args: &[Value]) -> Result<Value, String> {
        match member {
            "LoadTexture" => {
                let path = args.first().and_then(|v| v.as_str())
                    .ok_or("LoadTexture expects a str path")?;
                let id = self.load_texture_internal(path);
                Ok(Value::Struct {
                    name: "TextureHandle".into(),
                    fields: vec![("id".into(), Value::Int(id))],
                })
            }
            "DrawTexture" => {
                // ... draw ...
                Ok(Value::Void)
            }
            other => Err(format!("capability member `{other}` is not implemented")),
        }
    }
}

let mut engine = Engine::new();
engine.register_capability(
    "engine.graphics",                       // §9.1: exactly namespace.capability
    Arc::new(Graphics { /* ... */ }),
)?;
```

Rules the API enforces:

- `path` must be exactly two identifier segments (`namespace.capability`);
  anything else is rejected at registration.
- The provider-presence check runs at **load** time: a program that calls
  a capability with no registered provider fails the load.
- Providers must be `Send + Sync`; a provider may be called concurrently
  from independent invocations.
- A provider `Err(_)` fails the invocation with the message anchored at
  the script's call site.
- **§5.7**: a provider dispatched mid-invocation cannot
  `context.invoke(...)` back into the same context — the nested call
  fails with `ErrorKind::Reentrant`; the original invocation continues.
  Different contexts and threads are unaffected.

## Threads and concurrency

- `Engine` and `CompiledProgram`: immutable after configuration — share
  across threads freely.
- `Context`: `Send + Sync`. One context issuing invocations from many
  threads runs them **concurrently by design**; each invocation gets a
  fresh fuel cell and deadline, and script modules hold no shared mutable
  state, so there is nothing to lock.
- One `Value` handle: plain data, no interior mutability.

## A complete capability round-trip

Script (`shop.cm`):

```checkmate
import shop.store

result<int, str> buy(str sku, int qty) {
    Item item = shop.store.FetchItem(sku)
    shop.store.RecordSale(item, qty)
    return Ok(item.price * qty)
}
```

Schema (`shop.cm`):

```checkmate
schema shop 1.0.0

struct Item {
    int id
    str sku
    int price
}

since 1.0.0 capability store {
    Item FetchItem(str sku)
    void RecordSale(Item item, int qty)
}
```

Host:

```rust
use std::sync::Arc;
use cme::{Engine, ExecutionLimits, Value};
use cme::api::CapabilityProvider;

struct Store;

impl CapabilityProvider for Store {
    fn call(&self, member: &str, args: &[Value]) -> Result<Value, String> {
        match member {
            "FetchItem" => {
                let sku = args[0].as_str().ok_or("sku must be str")?;
                Ok(Value::Struct {
                    name: "Item".into(),
                    fields: vec![
                        ("id".into(), Value::Int(1)),
                        ("sku".into(), Value::Str(sku.into())),
                        ("price".into(), Value::Int(25)),
                    ],
                })
            }
            "RecordSale" => Ok(Value::Void),
            other => Err(format!("unimplemented member `{other}`")),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::new();
    engine.load_schema_file("shop.cm")?;                    // gate the load
    engine.register_capability("shop.store", Arc::new(Store))?;

    let program = engine.load_file("shop.cm")?;             // checked against schema
    let context = engine.create_context(&program, ExecutionLimits::default());

    let total = context.invoke("buy", &[Value::Str("sword".into()), Value::Int(2)])?;
    println!("{total}");                                    // Ok(50), CMON form
    Ok(())
}
```

For the *compile-time-verified* version of this same flow — where the
host's `impl` signatures are checked against the schema **when your
crate builds** — continue to
[Schema-Driven Embedding](schema-embedding.md).

## Printing results

`Value`'s `Display` renders the canonical
[CMON](../language/strings.md#cmon-checkmate-object-notation) form — the
same text `cme run` prints and the C API's `cm_value_to_string` returns.
Structs render `Name(field: value, ...)`, enums
`Name.Variant(payloads)`, arrays and maps in their literal shapes.

## Checklist for a production host

1. Register schemas **before** loading gated programs.
2. Register all capability providers **before** loading (presence is
   checked at load).
3. Give every context real limits (`fuel`/`deadline_ms`/`max_call_depth`)
   — unbounded fuel is a choice, not a default you want in production.
4. Match on `ErrorKind` for policy (budget → maybe retry with more fuel;
   deadline → shed load; unknown entry → integration bug).
5. Use `err.render()` for logs; keep `file`/`line`/`column` for editor
   jumps.
6. Keep providers free of panics — return `Err(String)` instead; a panic
   in a provider is a host bug, not a script error.
