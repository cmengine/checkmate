# Checkmate: A Static, Safe, Embeddable Scripting Language

**Version 0.4 — Consolidated Whitepaper**

> **A note on this revision.** Version 0.3 was published as a base document plus four appendices of draft override notes, each one superseding parts of everything before it. That was a useful way to iterate quickly, but it is not a useful way to *read* the language. This revision folds every override into the section it modifies, so the documents below describe one consistent language with no diff-reading required. Nothing here reverses a v0.3 decision — this is purely consolidation, plus one new section (§11, Modules) covering multi-file mods and the host/script contract, which was designed after v0.3 and did not previously appear in the whitepaper at all.

---

## Abstract

Checkmate is a statically typed, embeddable scripting language for host applications that demand safety, predictable performance, and explicit host control.
Scripts are written in a synchronous, C-like style, yet can transparently suspend when calling host-provided suspendable functions — without `async` or `await`.
Checkmate modules contain no mutable script-owned global state, use value semantics with automatic reference counting (ARC) and copy-on-write (COW), and access host resources solely through explicitly granted capabilities.
Execution can proceed through a fast bytecode interpreter or via optional native compilation through LLVM — ahead of distribution or on the target machine with persistent caching. Compilation is strictly ahead-of-time; Checkmate does not use JIT compilation.
The language provides no executor, scheduler, event loop, thread pool, or task system. All concurrency and scheduling remain under host control.
Boundary-crossing declarations — anything a script exposes to the host, or the host exposes to a script — are marked by a capitalization convention, checked at compile time against a versioned schema file that is the single source of truth for both sides of the boundary.
Scripts may be organized as multi-file, multi-directory mods with an explicit module system, and may define their own expressive syntax via a pattern-based megaprogramming system.
This whitepaper describes the language design, execution model, host integration, memory management, module system, security guarantees, and planned tooling.

---

## 1. Design Principles

- **Stateless script modules** – Checkmate modules contain no mutable script-owned global state. Persistent state is owned by the host and may only be accessed through explicitly granted host functions. Script functions are not necessarily pure: host APIs may read or modify host state.
- **Value semantics** – Checkmate values behave as independently owned values. The implementation may use automatic reference counting (ARC), structural sharing, and copy-on-write (COW), but assignment and mutation follow logical value semantics.
- **No direct shared host state** – Scripts never receive mutable references to host memory or shared host variables. Data crosses the host boundary through typed function arguments and return values. Host state is accessed explicitly through registered functions.
- **Independent invocations** – Independent Checkmate invocations do not share mutable script-owned state. Concurrency behavior for host resources is defined by the host API. Hosts are responsible for declaring whether individual capabilities are thread-safe, thread-affine, serialized, or otherwise restricted.
- **Static typing with no declaration inference** – Types are explicit for variables, parameters, fields, and return values by default. A single, visible opt-in form (type crystallization, §2.16) allows a local declaration's type to be determined by its initializer — but the language never *silently* infers the type of a declared binding.
- **Simplicity** – No semicolons, no implicit exceptions, and no hidden arbitrary control flow. The syntax stays close to C's declarative style but without historical pitfalls. Transparent suspension is host-declared behavior, not invisible magic.
- **Host-owned execution** – Checkmate never owns threads, an event loop, or an async runtime. The host decides when and how code runs.
- **Capability-based imports** – A script can only access host APIs that are explicitly granted, forming a natural sandbox.
- **Explicit, versioned host contracts** – What a script may call, and what a host requires a script to implement, is defined in one schema file that is the compiled source of truth for both the script toolchain and the host's Rust bindings (§10).
- **Boundary visibility** – Whether a declaration crosses the host/script boundary is visible at its name, not just at its import (§2.5).
- **No mod-to-mod coupling** – Mods cannot import one another. Cross-mod interaction exists only if the host deliberately designs and exposes a bridge capability (§11.5).
- **Performance** – Scripts can execute through a fast bytecode interpreter or be compiled ahead-of-time to native code through LLVM. Native artifacts may be produced ahead of deployment or compiled on the target machine and persistently cached for later reuse.

### 1.1. Non‑goals

- A general‑purpose application language.
- A package manager, or a resolver for mod-to-mod or mod-to-registry dependencies.
- A standard library beyond a small, host‑neutral core (§9).
- An async runtime or scheduler — this is entirely the host's responsibility.
- Just-in-time compilation.

---

## 2. Syntax Overview

Checkmate's syntax is deliberately familiar, borrowing from C, Rust, and a touch of modern scripting languages.

### 2.1. Files

Source files use the `.cm` extension. A file may contain multiple functions and type definitions. There is no main entry point; the host calls specific functions, or a script implements interface functions the host calls (§10, §11).

### 2.2. Comments

```checkmate
// Single‑line comment

/*
   Multi‑line comment
*/
```

### 2.3. Imports

Imports grant access to host‑provided APIs, or to other files within the same mod. They are capability gates; if a host does not expose a module, it cannot be imported, and a mod can never import another mod directly (§11.5).

```checkmate
import engine.graphics
import engine.input
import self.gamemode.rules
```

`engine` here is simply the name the host chose for its top-level module — hosts may name their bridge module anything (`engine`, `game`, `foo`); there is no reserved `host` keyword. `self` is reserved, and refers to the current mod's own file tree (§11.2).

### 2.4. Built‑in Scalar Types

```
int          // signed 64‑bit integer
float        // 64‑bit IEEE 754 floating‑point number
bool         // true or false
str          // immutable UTF‑8 string
void         // function returns no value
```

Fixed‑width integer types (`i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`) may be introduced later. For the initial specification, `int` is a 64‑bit signed integer.

No implicit conversions exist between numeric types. Explicit casts will be defined in a future revision.

#### 2.4.1 Overflow behavior

Integer arithmetic is checked by default. Overflow causes the current invocation to terminate with a runtime error. The core library may later provide explicit wrapping or saturating operations.

### 2.5. Boundary Capitalization

Any top-level declaration — a type, a `capability` function, or an `interface` function (§10) — that crosses the host/script boundary is named in `PascalCase`. Every other top-level declaration, and all fields, locals, and enum variants regardless of level, are named in `camelCase`. This is a hard compile-time rule, not a style convention:

- A capitalized top-level declaration that does not appear in the mod's derived boundary set (§10.4) is a compile error: `'SpawnZombie' is capitalized but implements no host contract member`.
- A type or function that *does* cross the boundary but is declared or used lowercase is the mirror error.

This makes the boundary visible at every use site, not only at the `import` line — reading a call, you can tell whether it crosses into host territory without checking imports. It applies only to top-level names; internal struct fields, enum variant names, and local variables keep ordinary `camelCase` regardless of whether their enclosing type is a boundary type.

```checkmate
struct vec2 {          // internal type, never crosses the boundary: lowercase
    float x
    float y
}

struct TextureHandle {  // appears in a capability signature: boundary type, capitalized
    int id
}

void spawnZombie(vec2 pos) {     // internal function: lowercase
    engine.graphics.DrawTexture(tex, pos)   // DrawTexture is a capability member: capitalized
}
```

### 2.6. Struct Types

Structs define records with named fields. Fields are separated by newlines; no commas or semicolons are used.

```checkmate
struct vec2 {
    float x
    float y
}

struct player {
    str name
    vec2 position
    int health
    bool alive
}
```

Construction uses named arguments:

```checkmate
vec2 pos = vec2(x: 10.0, y: 5.0)
player p = player(
    name: "Hero"
    position: pos
    health: 100
    alive: true
)
```

### 2.7. Enum Types

Enums are algebraic data types (tagged unions). Each variant may carry payload data.

```checkmate
enum gameEvent {
    Damage(int amount)
    Heal(int amount)
    Spawn(str enemyKind, vec2 position)
    PlayerDied()
}
```

Variant names are qualified with the enum type during construction:

```checkmate
gameEvent evt = gameEvent.Damage(25)
```

This avoids global name collisions and makes the constructed type explicit. Variant *names* keep their capitalization as shown above regardless of whether the enclosing enum is a boundary type — variant capitalization is a payload-tag convention, unrelated to §2.5's boundary rule.

### 2.8. Option and Result

The core library provides standard generic control types:

```checkmate
enum option<T> {
    Some(T value)
    None()
}

enum result<T, E> {
    Ok(T value)
    Err(E error)
}
```

A `?` propagation operator may be used for concise error handling. It requires the propagated error type to match exactly the enclosing function's error type; implicit error conversion is not performed.

```checkmate
result<user, httpError> loadUser(int id) {
    httpResponse response = engine.http.Get($"/users/{id}")?
    return Ok(parseUser(response.body))
}
```

Explicit error mapping can be achieved through core‑library operations, e.g. `engine.http.Get(id).mapError(...)`.

### 2.9. Generics

Structs and enums support generics with angle brackets:

```checkmate
struct pair<A, B> {
    A first
    B second
}
```

Generic functions are planned for a later release.

### 2.10. Variables and Assignment

Variables are declared with a type followed by a name and an optional initializer. Mutability is the default: any variable may be reassigned.

```checkmate
int score = 0
float speed = 4.5
str title = "Checkmate"
bool active = true

score = score + 10
speed = 2.0
```

There is no `let` or `var` keyword; the type name serves as the introducer. See §2.16 for the `infer` keyword, an explicit alternative to writing the type by hand.

### 2.11. Functions

Function signatures are C‑style: return type, name, parameter list (type name pairs). The body uses braces.

```checkmate
float distance(vec2 a, vec2 b) {
    float dx = b.x - a.x
    float dy = b.y - a.y
    return engine.math.Sqrt(dx * dx + dy * dy)
}
```

Non‑void functions must explicitly return a value. `void` functions may complete by reaching the end of their body; an explicit `return` is optional.

```checkmate
void log(str message) {
    engine.Log(message)   // return is optional here
}
```

### 2.12. Function‑call Arguments

Function calls support both positional and named arguments, but a single call must use one style exclusively.

Positional:

```checkmate
user u = getUser(42)
movePlayer(p, position)
```

Named:

```checkmate
user u = getUser(id: 42)
movePlayer(
    player: p
    position: position
)
```

Mixing `getUser(42, name: "Matin")` is not permitted.

### 2.13. Value Semantics in Practice

Variables and parameters follow value semantics. Assignments and argument passing logically copy the value. Mutating a parameter or local variable does not affect the caller's original.

```checkmate
void damage(player p) {
    p.health = p.health - 10   // modifies local copy
}
```

To update the original, the modified value must be returned and explicitly reassigned:

```checkmate
player damage(player p) {
    p.health = p.health - 10
    return p
}

p = damage(p)
```

Collections follow the same value semantics. Under the hood, copy‑on‑write avoids unnecessary duplication.

### 2.14. Control Flow

If statements use parentheses and braces:

```checkmate
if (health <= 0) {
    alive = false
}
```

For loops support iteration over host‑provided collections:

```checkmate
for (enemy e in enemies) {
    updateEnemy(e)
}
```

C‑style for loops may be added later.

While loops are planned:

```checkmate
while (health > 0) {
    tick()
}
```

### 2.15. Pattern Matching

`match` is the primary way to destructure enums. It is exhaustive: every variant must be handled. A wildcard `_` may be used to match any remaining cases.

`match` is an expression that may also be used as a statement.

```checkmate
str label = match (event) {
    Damage(int amount) => "Damage"
    Heal(int amount) => "Heal"
    PlayerDied() => "Dead"
    _ => "Unknown"
}

// Statement usage
match (event) {
    Damage(int amount) => {
        health = health - amount
    }
    _ => {}
}
```

Inside `match`, variant names are resolved using the statically known type of the matched value.

### 2.16. Type Crystallization

Type crystallization is an explicit shorthand for local variable declarations whose types are determined from their initializers. The keyword is `infer`:

```checkmate
infer wow = 10.0        // float wow = 10.0
infer name = "Hero"     // str name = "Hero"
infer pos = vec2(x: 10.0, y: 5.0)
```

is equivalent to:

```checkmate
float wow = 10.0
str name = "Hero"
vec2 pos = vec2(x: 10.0, y: 5.0)
```

The inferred type is fixed at the declaration and participates in ordinary static type checking exactly as though it had been written explicitly. `infer` does not create dynamically typed variables, permit implicit type changes, or introduce unrestricted type inference — it applies only when the compiler can determine one unambiguous static type.

```checkmate
int health = 100     // explicit type
infer health2 = 100  // explicit request for type crystallization
```

Declarations without an initializer, or expressions whose type cannot be determined unambiguously, require an explicit type:

```checkmate
infer items = []          // invalid — element type is ambiguous
int[] items = []          // required instead
```

Diagnostics reuse the same word: `cannot infer type for 'items'; ambiguous initializer`.

Numeric literals retain their ordinary literal types: `infer count = 10` gives `int count`; `infer speed = 10.0` gives `float speed`.

#### 2.16.1. Formatter-Assisted Crystallization

The official Checkmate formatter supports an optional auto-crystallize mode. When enabled, the formatter rewrites `infer`-based declarations into equivalent explicit type declarations:

```checkmate
infer health = 100
infer speed = 4.5
infer name = "Hero"
```

becomes:

```checkmate
int health = 100
float speed = 4.5
str name = "Hero"
```

This is a source transformation performed by the formatter — it does not change program behavior or alter the type selected by the compiler. It lets developers write concise code while retaining the option to produce fully explicit source for review, publication, generated output, long-term maintenance, or projects that prefer visible types everywhere.

A formatter implementation may expose this behavior through a command-line option such as `checkmate fmt --auto-crystallize`. Type crystallization therefore provides a reversible workflow: concise declaration → compiler determines the static type → formatter writes the type explicitly, on request.

### 2.17. Closures (Planned)

Closures will capture variables explicitly. The exact syntax is under design, but the intent is to require listing captured variables for clarity and memory‑management transparency.

---

## 3. Memory Management

### 3.1. Value Semantics and ARC/COW

Checkmate provides logical value semantics. Assignment behaves as though the assigned value becomes independently owned. The runtime may use automatic reference counting (ARC), structural sharing, and copy‑on‑write (COW) internally to avoid unnecessary copying.

When a shared value is mutated, the runtime creates a private representation before applying the mutation. Unmodified values may continue to share storage.

The compiler may use escape analysis, liveness analysis, ownership‑flow analysis, stack allocation, and reference‑count elision to reduce allocation and ARC overhead. Values that do not escape their defining scope may be stack‑allocated.

Developers never manually increment or decrement reference counts. COW and ARC are implementation strategies, not user‑visible ownership rules.

### 3.2. Opaque Host Handles

Some host resources are too large, expensive, or externally managed to be transferred as ordinary Checkmate values. Hosts may expose opaque handle types for resources such as textures, windows, database connections, ECS entities, audio streams, or UI objects. Handle types cross the boundary and are therefore capitalized per §2.5.

```checkmate
TextureHandle texture = engine.LoadTexture("hero.png")
engine.DrawTexture(texture)
```

A host handle does not expose raw memory or direct mutable access. Scripts may only pass handles to host functions that explicitly accept them.

Host handles are values that refer to host‑owned resources. Copying a handle does not copy the underlying resource and does not grant direct memory access. Their lifetime, thread affinity, cloning behavior, and invalidation rules are defined by the host API.

---

## 4. Asynchronous Execution

### 4.1. The Model: Transparent Yielding

Checkmate has no `async` or `await` keywords. Scripts are written as synchronous, blocking code:

```checkmate
user loadUser(int id) {
    httpResponse response = engine.http.Get($"/users/{id}")
    return parseUser(response.body)
}
```

When the host registers a function as suspendable (asynchronous) — marked with `suspend` in the schema file, §10.2 — the Checkmate engine notes that calls to it may suspend. At such a call, the engine transparently pauses script execution, saves the execution context, and returns control to the host (as a future/pollable object). The host's async runtime (e.g. Tokio) drives the I/O. Once the I/O completes, the engine resumes the script immediately after the call, passing the result.

From the script's perspective, `engine.http.Get` simply returned a value after some time. The host developer sees a standard Rust `Future` that can be `.await`ed.

### 4.2. Internal Implementation

Checkmate preserves synchronous source‑level control flow. Calls to host functions declared suspendable may transparently suspend execution and later resume at the point following the call.

The interpreter and native compiler may internally transform suspendable functions into resumable execution states. This transformation is an implementation detail. Script authors do not write state machines, callbacks, futures, `async`, or `await`.

- The interpreter represents suspended execution using resumable interpreter frames containing the current instruction position, local values, operand state, and pending host operation.
- The native backend compiles functions that may reach suspendable host calls using a resumable calling convention. Values that remain live across a suspension point are stored in compiler‑generated invocation state rather than relying on preservation of an ordinary native call stack.

### 4.3. No Checkmate Runtime

Checkmate provides asynchronous language semantics but does not provide an asynchronous runtime, executor, scheduler, event loop, thread pool, or task system. All asynchronous execution is driven by the embedding host.

### 4.4. Suspendable Host Functions

Host functions are declared as either synchronous or suspendable in the schema file (§10.2), using the `suspend` keyword. The Checkmate compiler and runtime use this information to determine whether a call may suspend — a host function can never unexpectedly suspend, since suspendability is part of the compiled schema and known before script execution.

Conceptual Rust API, generated from the schema (§10.5):

```rust
// generated from: since 1.0.0 float Sqrt(float x)
engine.register_fn("math.Sqrt", |x: f64| x.sqrt());

// generated from: since 1.2.0 suspend HttpResponse Get(str url)
engine.register_suspendable_fn("http.Get", |url: String| async move {
    fetch(url).await
});
```

Calls to suspendable host functions may be used in ordinary Checkmate functions. The possibility of suspension is handled transparently by the runtime.

### 4.5. Concurrency

A script cannot spawn parallel tasks. If concurrent host operations are required, the host may expose higher‑level capability functions that perform concurrency internally and return a combined result. This keeps the language free of concurrency primitives.

---

## 5. Execution Model

### 5.1. Bytecode Interpreter

For development and platforms that disallow native code generation (e.g. iOS), Checkmate can be executed via a register‑based bytecode interpreter. The interpreter is simple, portable, and preserves Checkmate's language‑level memory‑safety and capability restrictions. It enforces execution limits through instruction metering and periodic deadline checks.

### 5.2. Native Compilation via LLVM

Checkmate's native backend uses **LLVM**, targeting strictly **ahead-of-time (AOT)** compilation — Checkmate does not use just-in-time (JIT) compilation anywhere in its execution model.

Checkmate source is parsed, type‑checked, and lowered through a backend-independent Checkmate intermediate representation before being translated into LLVM IR. LLVM performs target-specific optimization and generates native object artifacts.

The compiler is embedded in the host application, not distributed with individual scripts. Compilation may happen:

- **Ahead of distribution** – plugin developer or CI compiles before deployment.
- **On the target machine** – the host compiles source or bytecode after installation, during loading, or at another host-controlled preparation phase, then persistently caches the resulting artifact.
- **Interpreter only** – no native compilation, used as a fallback.

LLVM is an implementation detail of the native backend. Checkmate's language semantics, capability model, host API, suspension behavior, memory model, and execution limits are defined independently of LLVM. The bytecode interpreter does not use LLVM; it executes Checkmate bytecode directly and remains the portable execution mode for development, restricted platforms, and hosts that do not enable native compilation.

Cranelift may be revisited later as an optional faster-iteration backend for local development builds, but it is not part of the current design and is not the native backend.

### 5.3. Native Artifact Caching & Compatibility

Cached native artifacts are associated with the following compatibility information:

- Checkmate language version
- Checkmate compiler version
- Checkmate runtime ABI version
- target architecture
- target operating system
- target ABI
- CPU feature requirements
- host API schema version (§10.3)
- optimization level
- relevant compiler configuration

A cached artifact is loaded only when all required compatibility conditions are satisfied. Otherwise, the host recompiles the script or falls back to the bytecode interpreter. Native artifacts must not be treated as portable across operating systems, architectures, ABI versions, or incompatible host APIs.

### 5.4. Hybrid Mode

The engine can always fall back to the interpreter when:

- running on a platform that forbids native code loading (e.g. iOS)
- the script is untrusted and the host chooses not to compile
- during development, to avoid compilation latency

### 5.5. Execution Limits

The host may configure execution limits for each Checkmate invocation. Supported limits may include:

- instruction or fuel budget
- elapsed‑time deadline
- allocation budget
- maximum recursion depth
- maximum call depth
- maximum collection size
- maximum number of suspended invocations
- maximum native compilation time

The bytecode interpreter enforces execution limits through instruction metering and periodic deadline checks. The native backend enforces limits through compiler‑generated safepoints, which may be inserted at loop backedges, function calls, host calls, and other suitable locations. Native execution is interrupted cooperatively at safepoints, not by asynchronously terminating arbitrary machine instructions. Safepoint and fuel/deadline enforcement apply identically to LLVM-generated native code as described above.

Fuel provides deterministic operation limits; deadlines provide responsiveness limits. Hosts may use either or both.

### 5.6. Cancellation

A pending Checkmate invocation may be cancelled by the host. Cancellation releases the suspended script execution state and drops values held exclusively by that invocation.

When supported by the host API, cancellation is propagated to outstanding host operations. Hosts are not required to make every operation cancellable.

Cancellation is cooperative. A host operation that cannot be cancelled may continue independently after the Checkmate invocation has been discarded. Cancellation must correctly release ARC‑managed values, pending interpreter frames, native resumable frames, and host‑operation handles.

### 5.7. Reentrancy

Reentrant execution on the same Checkmate invocation is prohibited. A host function called by Checkmate must not call back into the same engine invocation before the original call completes. Independent invocations may execute concurrently when the host and engine configuration permit it.

---

## 6. Host Integration

### 6.1. Rust Host API

The Rust API provides an `Engine` struct:

- `register_fn(name, func)` – synchronous host function
- `register_suspendable_fn(name, func)` – suspendable (async) host function
- `call<T>(name, args)` – always returns a Rust `Future`. A fully synchronous Checkmate function completes during its first poll. A function that reaches a suspendable host call remains pending until the host operation becomes ready.
- `call_blocking<T>(name, args)` – a convenience API for hosts that explicitly permit blocking. Checkmate does not create or own an async executor to implement this operation.

Type marshalling is automatic for built‑in types and user‑defined structs/enums (via a derive macro or manual implementation). As of §10.5, both the trait definitions and the marshalling code for schema-declared members are generated directly from the schema file rather than written by hand.

```rust
let user: User = engine
    .call("load_user", (42,))
    .await?;
```

### 6.2. C Host API

A minimal C API exposes an opaque future handle:

```c
cm_future_t* future = cm_call(engine, "load_user", args, arg_count);
cm_poll_result_t result = cm_future_poll(future, &output);
```

Poll states:

- `CM_PENDING` – the invocation is still running/suspended
- `CM_READY` – the invocation completed; output contains the result
- `CM_ERROR` – the invocation terminated with an error
- `CM_CANCELLED` – the invocation was cancelled

The C host owns polling and scheduling. Checkmate does not create threads or run an event loop. A blocking convenience API may be provided only if its waiting behavior is explicitly supplied or approved by the host.

### 6.3. Plugin Lifecycle (Planned)

A formal lifecycle will govern:

- loading a mod
- compiling or interpreting a mod
- creating invocation state
- suspending an invocation
- cancelling an invocation
- unloading a mod
- invalidating native artifacts

A mod cannot be unloaded while it has active or suspended invocations unless the host first cancels or completes them.

---

## 7. Security & Sandboxing

Checkmate's security model relies on:

1. **No ambient authority** – Scripts have no access to filesystem, network, or OS resources unless explicitly provided by the host through imported modules.
2. **Capability‑based imports** – Only explicitly granted host modules may be imported.
3. **No shared mutable state** – Scripts do not hold direct mutable references to host variables. Host state is accessed only through function calls.
4. **Language‑level memory safety** – No raw pointers, no system‑call interface, no unrestricted FFI.
5. **Execution and resource limits** – Fuel, deadlines, allocation limits, and depth limits prevent denial‑of‑service.
6. **No mod-to-mod authority** – A mod cannot observe, call, or otherwise gain authority over another mod except through a capability the host deliberately designed for that purpose (§11.5). There is no ambient cross-mod channel to lock down, because none exists by default.
7. **Native code safety** – Generated code interacts with the host through compiler‑controlled runtime operations and explicitly registered host capabilities. Checkmate's capability model is a language‑level isolation mechanism, not a substitute for process isolation when executing hostile code under a strong adversarial threat model.

---

## 8. Megaprogramming

Checkmate supports megaprogramming, a compile-time syntax transformation system that allows scripts to define expressive domain-specific syntax and generate ordinary Checkmate code. Megaprograms are entirely script-side: they do not invoke host functions, access host capabilities, observe host state, or require host participation. The embedding host receives only the fully expanded Checkmate module, after megaprogram expansion, name resolution, type checking, host API validation, and execution-limit instrumentation have already run.

### 8.1. Invocation and Declaration: `magic`

A megaprogram's name is a dotted path, exactly like a host capability or interface name — the leading segment is the owning module, since a megaprogram is not a free-form macro identifier system but a named, importable, module-scoped declaration.

Declaration (inside the owning module's own file, so no module prefix is needed, the same way a file inside `engine` doesn't write `engine.` to refer to its own siblings):

```checkmate
magic spawn(...) { ... }        // declared inside module `agent`
```

Invocation (always fully qualified, since a call site needs to know which module it's pulling a megaprogram from, same as any other import):

```checkmate
magic(agent.spawn) {
    kill @e[type=zombie]
}
```

### 8.2. Pattern = Declaration Parameter List

There are no separate `pattern { }` / `expand { }` blocks. The `magic name(...)` parameter list *is* the match pattern; the function body is the expansion template. Literal tokens (`"<"`, `"SELECT"`, `":"`, etc.) may appear directly in the parameter list alongside captures — this is the one place Checkmate's grammar allows a string literal to mean "match this syntax" rather than "this is a string value."

### 8.3. Fragment Kinds

```
$ident   name     -- a Checkmate identifier
$expr    name     -- a full expression, resolved/typechecked at the call site
$type    name     -- a Checkmate type
$tt      name     -- one token or one balanced-delimiter group
$tag     name     -- relaxed identifier: letters, digits, `-`, `.`
                     (does not resolve against scope; for foreign tokens
                     — model names, CSS classes, version strings)
$text    name     -- verbatim text, no interpretation
$raw     name     -- verbatim text captured now, reinterpreted later against
                     a fragment kind chosen at match time (§8.6)
$rawvalue<fn(...)> name
                  -- like $raw, but the reinterpretation function is given
                     explicitly and depends on an earlier capture (§8.6)
$template name    -- verbatim text; `{{ expr }}` islands parse as live
                     Checkmate expressions and splice in at expansion
$path    name     -- route-pattern sub-grammar: literal segments, `{name}`
                     capture, `{*name}` rest capture
$selector<css> name
                  -- CSS-selector sub-grammar; recognizes `&` as a splice
                     point for an inherited `context` value (§8.8)
```

**Contextual/checked fragment kinds:** any fragment kind may carry a type argument that is not a closed enum but a live lookup against the host API schema (§10) or another schema-like registry, resolved at engine build time, not hardcoded per macro:

```
$ident<HtmlTag>          name   -- checked against an enum
$ident<schema.table>     name   -- checked against live DB schema
$ident<schema.column>    name   -- ditto, columns
$ident<css.property>     name   -- checked against known CSS props
```

This is what makes the SQL and CSS examples in §8.11 possible — the macro doesn't hardcode a token list, it defers validity to a registered lookup function, so it stays correct as the underlying schema changes.

### 8.4. Repetition, Grouping, Choice

```
each { ... }                 -- repeat zero-or-more; usable in both pattern
                                 and expansion template (in the template,
                                 re-emits per match)
each sep "," { ... }         -- repeat with a required separator token
                                 between matches, no trailing separator
each in X { ... }            -- in expansion, iterate a specific earlier
                                 capture list X (disambiguates when
                                 multiple `each` groups exist)
optional { ... }             -- the whole enclosed subsequence is present
                                 or absent as a unit (not per-token — this
                                 is what prevents a guard-without-role
                                 mismatch when two optional tokens appear
                                 in sequence)
$x?                          -- shorthand for a single optional capture,
                                 still valid for one-token cases
oneof { A => (...), B => (...) }
                              -- ordered choice; the matched branch's tag
                                 (A/B/...) is available in the expansion
                                 as `kind` for a `match`
```

### 8.5. Backreferences (`where`)

A pattern may close with a `where` clause constraining captured values against each other, checked at match time, failing the match — with a real diagnostic — rather than deferring to runtime:

```checkmate
magic view(
    "<" $tag<HtmlTag> tagName ... "</" $tag<HtmlTag> closeName ">"
) where closeName == tagName {
    ...
}
```

Failure message pattern: `closing tag 'div' does not match opening tag 'p'`. `where` may reference any capture in scope, including inside `each`/`oneof` (e.g. `where each col in columns: col.alias in [...]`).

A `where` clause referencing a capture inside an absent `optional { }` block is vacuously true, rather than an error.

### 8.6. Late Reinterpretation (`$raw` / `$rawvalue`)

Some grammars need a captured slot's *kind* to depend on another slot captured earlier in the same match — an HTML attribute's value is a CSS block if the attribute name is `style`, an expression if the name starts with `@on`, else a plain string; a CSS declaration's value shape depends on the property name.

`$raw` captures unparsed text; the expansion template reinterprets it explicitly via a `match`/lookup on the earlier capture:

```checkmate
each { $ident name "=" "$raw value" } as attrs
...
match (name) {
    "style" => css.parse($raw(value))
    _       => $raw(value)
}
```

`$rawvalue<fn(...)>` is the same mechanism with the lookup function named directly in the pattern instead of the template:

```checkmate
$ident<css.property> prop ":" $rawvalue<css.valueOf(prop)> val ";"
```

This is the single mechanism that makes conditional-fragment-kind situations work, instead of needing several hardcoded fragment kinds per use site.

### 8.7. Recursion (`recur`)

A megaprogram may reference itself inside its own pattern and expansion, enabling nested nesting — HTML elements inside elements, CSS rules inside rules, statements inside statements:

```checkmate
each { recur view } as children
each { recur stmt } as body
```

This is ordinary recursive-descent semantics — no special runtime support; it expands at compile time the same as any other megaprogram call, just self-referentially.

### 8.8. Context Blocks (Ancestor Data for `recur`)

A nested `recur` call sometimes needs data from its *parent* match — the enclosing selector, the enclosing indent column — which plain recursion doesn't expose, since recursion only flows arguments *down* what's explicitly passed, and an implicit parent-scope lookup would violate hygiene. `context { }` is the explicit, opt-in fix for this "descendant needs ambient ancestor value" pattern (the same problem, and the same style of solution, that UI frameworks with implicit context providers solve):

```checkmate
magic style(
    context { $selector<css> parentSel }
    ...
    each { recur style with context { parentSel: sel } } as nested
) { ... }
```

`$selector<css>` fragments recognize `&` as "splice `context.parentSel` here" — a fragment-kind-local parsing rule, the same pattern as `$template`'s `{{ }}` handling. A top-level (non-nested) `magic(css.style)` call supplies no context; nested `recur` calls pass it explicitly.

### 8.9. Indentation-Sensitive Matching (`indent { }`)

Checkmate's own grammar is whitespace-insignificant (§2), and that is unchanged everywhere outside a `magic` pattern. `indent { }` is a combinator legal only inside a pattern, for matching indentation-sensitive *embedded* grammars — Python being the motivating case:

```
indent { pattern }
```

The column of the first line matched inside the block becomes its reference column. The block keeps matching lines at exactly that column, and ends at the first line whose leading whitespace is `<=` the reference column, or at EOF. A nested `indent { }` inside it establishes its own deeper reference column and recurses independently — dedent detection falls out of one column comparison, with no per-construct special casing needed (`if`/`else`, loops, nested defs all behave identically).

### 8.10. LSP Hints

Captures may carry editor hints via `#` (`@` is reserved for literal tokens being matched, e.g. `@onClick` in `html.view`, and the two must never collide):

```checkmate
#complete(engine.availableModels)
#hover("Model identifier, e.g. claude-opus-latest")
$tag model
```

`#complete(...)` supplies autocomplete, and may reference earlier captures for contextual completion (`#complete(schema.columnsOf(alias.table))` in the SQL example below — completion for a column name depends on which table alias was captured earlier in the same match). `#hover(...)` supplies hover text. `#highlight(kind)` forces semantic-token classification. These compile into the host API schema (§10) alongside the megaprogram definition, so an LSP gets them without re-parsing macro internals per edit.

### 8.11. Worked Examples

**agent.spawn** — hyphenated foreign token (`$tag`), verbatim prompt body:

```checkmate
magic spawn($tag model, $ident effort, $text prompt) {
    engine.SpawnAgent(model: "$model", effort: "$effort", prompt: $prompt)
}

magic(agent.spawn) {
    model: claude-opus-latest
    effort: high

    Hi. Do this for me:
    blah blah blah
}
```

**html.view** — recursion, backreference, late reinterpretation, `$template`:

```checkmate
magic view(
    "<" $tag<HtmlTag> tagName
    each { $ident name "=" "$raw value" } as attrs
    ">"
    each { oneof { child => recur view, text => $template } } as children
    "</" $tag<HtmlTag> closeName ">"
) where closeName == tagName {
    engine.RenderElement(
        tag: "$tagName"
        attrs: [ each in attrs where not name.startsWith("@") {
            (name: "$name", value: match (name) {
                "style" => css.parse($raw(value))
                _       => $raw(value)
            })
        }]
        events: [ each in attrs where name.startsWith("@on") {
            (kind: eventKind.from(name.after("@on")), handler: $expr(value))
        }]
        children: [ each in children {
            match (kind) { child => $child, text => engine.TextNode($text) }
        }]
    )
}
```

**router.pathList** — grouped optionals:

```checkmate
magic pathList(
    each {
        optional { $tag guard ":" $tag role }
        $path route "=>" $expr handler
    }
) {
    each { engine.RegisterRoute(guard: "$guard", role: "$role", path: $route, handler: $handler) }
}
```

**db.query** — schema-checked idents, contextual completion, synthesized return type:

```checkmate
magic query(
    "SELECT"
    each sep "," {
        #complete(schema.columnsOf(alias.table))
        $ident alias "." $ident<schema.column> col
        optional { "AS" $ident asName }
    } as columns
    "FROM" $ident<schema.table> fromTable "AS" $ident fromAlias
    each { "JOIN" $ident<schema.table> joinTable "AS" $ident joinAlias "ON" $expr onCond } as joins
    optional { "WHERE" $expr whereCond }
) where each col in columns: col.alias in [fromAlias, joins.each.joinAlias] {
    engine.DbQuery(
        sql: sqlgen.select(
            columns: [ each in columns { (alias: "$alias", column: "$col", as: "$asName") } ]
            from: (table: "$fromTable", alias: "$fromAlias")
            joins: [ each in joins { (table: "$joinTable", alias: "$joinAlias", on: $onCond) } ]
            where: $whereCond
        )
        resultType: schema.rowTypeOf(columns)
    )
}
```

**css.style** — nested recursion, `&` via context, sibling-dependent value kind:

```checkmate
magic style(
    context { $selector<css> parentSel }
    each {
        oneof {
            rule => (
                $selector<css> sel "{"
                    each { $ident<css.property> prop ":" $rawvalue<css.valueOf(prop)> val ";" } as decls
                    each { recur style with context { parentSel: sel } } as nested
                "}"
            )
            atrule => ("@media" $text query "{" each { recur style } as mediaBody "}")
        }
    } as rules
) {
    engine.CompileStyle(rules: [ each in rules { match (kind) {
        rule   => css.rule(selector: sel.resolve(parentSel), decls: [each in decls { (prop: "$prop", value: css.coerce(prop, val)) }], nested: [each in nested { $nested }])
        atrule => css.media(query: "$query", body: [each in mediaBody { $mediaBody }])
    }}])
}
```

**py.def** — indentation-sensitive embedded grammar:

```checkmate
magic def("def" $ident fname "(" each sep "," { $ident param } "):" indent { each { recur stmt } as body }) {
    str $fname(each { str $param }) { each in body { $stmt } }
}

magic stmt(
    oneof {
        ifStmt     => ("if" $expr cond ":" indent { each { recur stmt } as thenBody } optional { "else" ":" indent { each { recur stmt } as elseBody } })
        returnStmt => ("return" $expr val)
        printStmt  => ("print" "(" $expr val ")")
        assignStmt => ($ident target "=" $expr val)
    }
) {
    match (kind) {
        ifStmt     => { if ($cond) { each in thenBody { $stmt } } else { each in elseBody { $stmt } } }
        returnStmt => { return $val }
        printStmt  => { engine.Print($val) }
        assignStmt => { infer $target = $val }
    }
}
```

### 8.12. Open Questions

- Escaping literal `{{` inside `$template` output is not yet decided.
- The fragment-kind set is fixed for v0.4. A user-extensible set edges toward procedural megaprogramming, which is deferred (§8.13).
- Formatter behavior for `indent { }`-matched embedded content — how the pretty-printer preserves or reformats foreign indentation — is deferred to the formatter spec.

### 8.13. Procedural Megaprograms

Procedural megaprograms are not required for the initial language and are deferred. They may be considered in a future revision if declarative transformations prove insufficient for advanced syntax analysis or code generation.

### 8.14. Tooling Support

The language server may analyze megaprogram definitions and expansions to provide completion within megaprogram invocations, validation of generated host calls, diagnostics mapped to the original invocation, navigation between generated code and its source, and expansion previews.

### 8.15. Compilation Pipeline

The complete compilation model:

```
Checkmate source
       ↓
Parse
       ↓
Megaprogram expansion
       ↓
Name resolution and type checking
       ↓
Checkmate intermediate representation
       ├── Bytecode generation
       │        ↓
       │    Interpreter
       │
       └── LLVM IR generation
                ↓
          LLVM optimization
                ↓
       Native AOT artifact
```

Megaprogram expansion occurs before ordinary semantic analysis. The host API is validated against the expanded Checkmate program, and the host never receives unexpanded megaprogram syntax.

---

## 9. Core Library

Checkmate provides a deliberately small, host‑neutral core library. The initial core library includes:

- scalar types
- strings
- arrays/lists
- maps
- `option<T>`
- `result<T, E>`
- equality and ordering
- iteration
- basic string operations
- basic collection operations
- numeric utilities
- formatting support

The following are intentionally kept outside the core language and belong to host‑provided capabilities:

- HTTP clients
- database implementations
- filesystem access
- graphics APIs
- UI frameworks
- networking runtimes
- async executors
- application frameworks

---

## 10. The Host Contract: Schema Files

Everything a script may call on the host, and everything the host requires a script to implement, is defined in one `.cm` schema file per host module. This file is the single source of truth on both sides of the boundary: the script toolchain consumes it natively as part of ordinary compilation, and the host's Rust bindings are generated from it via macro rather than hand-written and kept in sync by hand (§10.5). This plays the same role a `.proto` file plays for a gRPC service — a contract compiled into both client and server — while remaining an ordinary Checkmate source file rather than a separate IDL.

### 10.1. `capability`: What a Script May Call

```checkmate
schema engine v1.4.0

capability engine.graphics {
    since 1.0.0 TextureHandle LoadTexture(str path)
    since 1.0.0 void DrawTexture(TextureHandle tex, vec2 position)
    since 1.2.0 suspend Image FetchRemoteThumbnail(str url)
}
```

A `capability` block is a set of functions the host provides and a script may call, exactly the role `import`ed host modules already played in §2.3/§6.1 — this section gives that contract a concrete file format. Every member is a boundary declaration and is named per §2.5.

### 10.2. `interface`: What a Script Must Provide

```checkmate
interface engine.gamemode requires engine.core {
    since 1.0.0 GameState InitGame(GameConfig config)
    since 1.0.0 void OnTick(GameState state, float deltaTime)
    since 1.0.0 void OnPlayerJoin(GameState state, PlayerId id)
    since 1.4.0 optional void OnPlayerLeave(GameState state, PlayerId id)
}
```

An `interface` block is a set of functions the host requires a script to implement, and will call into. A script satisfies an interface with one or more `impl` blocks (§11.4), which the compiler checks for completeness against the interface at compile time — a missing required member is a compile error naming exactly which function is missing, not a runtime failure discovered when the host tries to call it.

`requires` declares that implementing this interface obligates the mod to also fully implement another named interface. This is how "no new-game-mode module without a main module" is enforced: `engine.gamemode requires engine.core` means a mod with an `impl engine.gamemode` block but no complete `impl engine.core` fails to compile, with a diagnostic identifying the missing prerequisite interface. One relation — `requires` — covers both "this is a mandatory baseline" and "that other module is an optional extension built on it," since whether a given interface is optional from the *host's* perspective is just a question of whether anything requires it.

`optional` marks a single interface member as not required for completeness — a mod may implement `engine.gamemode` without providing `OnPlayerLeave` and still compile cleanly. This exists specifically so that adding a new required interface member in a later schema version doesn't retroactively break every mod that predates it (§10.3).

### 10.3. Versioning

The schema file carries a single semver at the top (`schema engine v1.4.0`), and each individual member is tagged with the version it was introduced in via `since`. A mod's manifest (§11.1) declares the schema version it was built against; the compiler filters the schema down to members with `since <=` that declared version, and rejects any call to, or requirement to implement, a member newer than what the mod declares — with the specific version named in the error.

This handles minor-version compatibility automatically and entirely within the one file, with no external diffing tool and no snapshot of prior schema versions to maintain:

- Adding a new `capability` member is non-breaking — older mods simply don't call it, and never see it in their filtered view of the schema.
- Adding a new *required* `interface` member is breaking — it demands an implementation that doesn't exist in mods written before it existed. This is why `optional` (§10.2) exists on the interface side specifically; there is no equivalent concern on the capability side.

Major-version bumps are not automatically handled, because there is no automatic handling of a genuine breaking change — that is what "breaking" means. What tooling can enforce is honesty about it: a `since` tag introduced at or below the current major version, attached to a change that alters an existing required member's signature rather than adding a new one, is rejected until the schema's leading major version is incremented. The break still has to happen manually; the tooling's job is only to stop it from being labeled as a minor bump.

### 10.4. Derived Boundary Set

The set of names subject to the capitalization rule in §2.5 is derived, not separately declared: it is exactly the set of types and functions that appear anywhere in a `capability` or `interface` block, filtered to the members visible under the mod's declared schema version (§10.3). A type used only in `capability`/`interface` signatures is a boundary type even if never explicitly labeled as one; a type never mentioned in either is never a boundary type, however it's named.

### 10.5. Generated Rust Bindings

The schema file is the codegen input for the host side, not a design document that Rust bindings are separately hand-maintained against:

- Each `capability` block generates a Rust trait the host implements, replacing hand-written `register_fn`/`register_suspendable_fn` calls (§6.1) with a derive macro. `suspend` members generate an `async fn` in the trait.
- Each `interface` block generates a typed proxy the host calls into — e.g. `gamemode.on_tick(state, dt).await` — instead of the stringly-typed `engine.call("onTick", args)` form, though it rides on the same `Future`-based `call()` primitive underneath (§6.1).
- `since`/`optional` metadata generates the same version-filtered view on the Rust side that the script compiler enforces on the script side, so a host binary built against schema 1.2 cannot accidentally call a 1.4-only capability member either.

This is the concrete mechanism behind the "generated Rust bindings" and "generated C descriptors" already named as a goal for the host API schema in §5.3/§13 — the schema file described here is that schema, given an actual file format.

---

## 11. Modules: Multi-File Mods

A single script file is enough for a small tool, but a game mod is rarely one file — it wants a natural tree of source across multiple concerns (rules, UI, events) without flattening everything into one script or inventing an ad hoc split mechanism per mod.

### 11.1. A Mod Is a Directory

```
my_mod/
    mod.toml
    src/
        main.cm
        gamemode/
            rules.cm
            events.cm
        ui/
            panel.cm
```

`mod.toml` is deliberately static, boring metadata — name, version, `checkmate_version`, and the `host_schema_version` a launcher or package host can read without invoking the Checkmate compiler at all: a subset of the same compatibility categories §5.3 already tracks for native artifacts, reused here for a lighter-weight, pre-compile check. It is a manifest, not a build system or resolver — consistent with the non-goals in §1.1.

### 11.2. Module Paths Mirror the File Tree

A file's path under `src/` is its module path, with no separate mapping to keep in sync: `gamemode/rules.cm` is `self.gamemode.rules`. `self` is the reserved root referring to the current mod's own tree (§2.3); it is never a stand-in for a host module name, and a host module is never referred to as `self`.

```checkmate
import self.gamemode.rules
import engine.gamemode
```

By the time source reaches §5 (compilation), a multi-file mod is still one compiled unit — the module system is source organization, not a new execution concept, and it introduces no new runtime behavior. Because script modules have no mutable global state (§1), splitting a mod across files sidesteps the static-initialization-order problem multi-file setups usually have to solve — there is no init order to define, because there is nothing that needs one.

### 11.3. Capabilities and Interfaces Are Imported Like Any Host Module

A schema file (§10) is authored and shipped by the host, and imported the same way any other host module is:

```checkmate
import engine.gamemode
import engine.core
```

### 11.4. `impl` Blocks Satisfy Interfaces

```checkmate
impl engine.gamemode {
    GameState InitGame(GameConfig config) { ... }
    void OnTick(GameState state, float deltaTime) { ... }
    void OnPlayerJoin(GameState state, PlayerId id) { ... }
}
```

The compiler collects every `impl engine.gamemode` block across the entire mod tree — `impl` blocks for the same interface may be split across multiple files, which is exactly what a tree-structured mod wants for something like gamemode logic living across `rules.cm` and `events.cm` — and checks the union against the interface. A required member missing from every block in the mod is a compile error naming that member. Two identical members implemented in two different files is also a compile error (`duplicate implementation of 'OnTick'`), since splitting is meant to divide responsibility, not duplicate it.

`requires` (§10.2) is checked the same way: implementing `engine.gamemode` without a complete `impl engine.core` somewhere in the mod is a compile-time error identifying the missing prerequisite interface, not a failure discovered when the host tries to load the mod.

At load time, the host can check which `interface` blocks a mod completely implements — a mod with a complete `impl engine.gamemode` is eligible to appear in a mode-select menu; one without simply isn't offered. The `impl` blocks themselves are the declaration of what's implemented; nothing further needs to be duplicated into the manifest.

### 11.5. No Mod-to-Mod Imports

A mod's only import targets are `self.*` (its own tree) and host-provided modules (§2.3). There is no syntax for importing another mod, so there is nothing to explicitly forbid — attempting it is simply an unresolved import, reported as `cannot import 'other_mod' — mods may only import 'self' and host-provided modules`.

If a host wants mods to interact, it designs and exposes a bridge capability, at which point it is governed by exactly the same schema and versioning machinery as any other capability:

```checkmate
capability engine.modRegistry {
    since 1.0.0 void RegisterHandler(str eventName, HandlerRef handler)
}
```

No separate mechanism is introduced for this case; a bridge capability is not a special kind of thing, just an ordinary capability the host chose to write.

### 11.6. Autocomplete and Tooling

Because the schema file (§10) is itself Checkmate source with LSP hints available (§8.10), the same language server that completes ordinary script code completes capability calls, interface signatures, and `impl` block members with no separate tooling path. An IDE can additionally offer an "implement `engine.gamemode`" quick action that scaffolds an empty `impl` block with every required member stubbed, since the interface is a real, complete, named declaration rather than a convention inferred from usage.

### 11.7. Open Questions

- Mod-to-mod dependency declarations beyond a host-provided bridge (e.g. "this mod expects mod X's registry to exist") are unaddressed and out of scope for the current design.
- Whether `mod.toml` should itself become an all-Checkmate manifest, rather than a separate static format, is unresolved. The current choice favors a format tooling can read without a Checkmate parser at all, over having a single source-language for every file in a mod.

---

## 12. Tooling & Ecosystem (Planned)

- Language server (LSP) for IDE support, including megaprogram-aware completion and diagnostics (§8.14) and schema/`impl`-aware completion (§11.6).
- Official formatter for consistent code style, including auto-crystallization (§2.16.1) and (deferred) formatting rules for `indent { }`-matched embedded content (§8.12).
- Debugger protocol that maps generated code back to Checkmate source, including megaprogram expansion sites.
- LLM‑friendly design – explicit types, explicit (never silent) inference, no semicolons, and familiar syntax make Checkmate easy for large language models to generate correctly.

---

## 13. Conclusion

Checkmate is a statically typed, embeddable scripting language built around explicit host control. Script modules contain no mutable script‑owned global state, use logical value semantics backed by ARC and copy‑on‑write, and access host capabilities only through explicitly registered, versioned functions.

Checkmate preserves a simple synchronous programming model while allowing host functions to suspend execution transparently. The language provides no executor, scheduler, event loop, thread pool, or task system — the embedding application owns all asynchronous execution and may integrate Checkmate with Rust futures, C event loops, game‑engine schedulers, or custom runtimes.

Checkmate supports a portable bytecode interpreter and optional ahead-of-time native compilation through LLVM with persistent artifact caching. Native artifacts may be produced ahead of deployment or compiled on the target machine; the interpreter remains available for rapid development, restricted platforms, and fallback execution.

Scripts may define their own expressive, host-independent syntax via a pattern-based megaprogramming system, and may be organized as multi-file, multi-directory mods whose relationship to the host — what they may call, what they must implement, and which versions of each — is defined by a single versioned schema file acting as the compiled source of truth for both the script toolchain and the host's generated Rust bindings.

Checkmate provides language‑level memory safety and capability isolation, while hosts retain control over permissions, scheduling, resource limits, host state, mod-to-mod boundaries, and deployment policy.

The goal is not to replace Rust, C, or general‑purpose application languages. Checkmate is designed to give host applications a small, predictable, statically typed extension language whose behavior remains explicit at the host boundary.

Checkmate — the scripting language where the host calls the shots.
