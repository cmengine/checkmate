# Checkmate: A Static, Safe, Embeddable Scripting Language

**Version 0.5 — Consolidated Whitepaper**

---

## Abstract

Checkmate is a statically typed, embeddable scripting language designed for host applications that demand absolute memory safety, predictable execution characteristics, and explicit host authority.  
Scripts are written in a clean, synchronous, C-like style, yet transparently yield control when calling host-provided suspendable operations—without requiring `async`, `await`, or manual asynchronous state machines. This transparent suspension is realized at compile time via continuation splitting into ordinary native calling conventions, eliminating the need for a runtime execution scheduler.

Checkmate modules contain no mutable script-owned global state, follow strict logical value semantics backed by automatic reference counting (ARC) and copy-on-write (COW), and access external resources exclusively through capability-gated host imports. Execution proceeds either through a lightweight, portable bytecode interpreter or via ahead-of-time (AOT) native compilation through LLVM. Compiled native artifacts may be emitted ahead of distribution or compiled directly on target machines with persistent artifact caching. Checkmate deliberately contains no JIT engine, no thread pool, no event loop, and no tracing garbage collector.

The host contract is governed by versioned `.cm` schema files acting as the single source of truth for both the script toolchain and generated host Rust/C bindings. Boundary-crossing declarations follow an enforced capitalization convention checked at compile time. Scripts can be structured as multi-file mods with hierarchical directory namespaces, extensible via a declarative, pattern-based megaprogramming system. Checkmate scales from full desktop and game engine environments down to `no_std` bare-metal and microcontroller targets via an isolated runtime trait boundary.

---

## 1. Design Principles

- **Stateless script modules** – Script modules contain no ambient mutable global state. Persistent state is owned strictly by the host application and is manipulated solely through explicitly passed opaque handles and registered host capabilities. Concurrency across independent script invocations is inherently race-free.
- **Value semantics** – All script-visible values behave as independently owned data. While the underlying implementation leverages automatic reference counting (ARC), structural sharing, and copy-on-write (COW), assignment and parameter passing logically clone data.
- **Allocation bias toward compile-time resolution** – Value semantics allow aggressive escape analysis, liveness analysis, and stack allocation for non-escaping values. Dynamic allocation and ARC overhead are strictly residual costs rather than baseline overhead.
- **No tracing garbage collection** – Memory reclamation is deterministic via compiler-inserted ARC and stack reclamation. There are no stop-the-world pauses, background sweep threads, or GC spikes.
- **Compile-time continuation splitting** – Asynchronous host operations are called synchronously in script source. The compiler splits functions at suspension points into discrete, ordinary native continuations, bypassing the need for runtime frame suspension or custom call stacks.
- **Host-owned concurrency and scheduling** – Checkmate provides no built-in executor, scheduler, event loop, green threads, or task queues. All execution time and concurrency are driven entirely by the host application.
- **Static typing with visible crystallization** – Types are explicit for parameters, fields, and return values. Local variable declarations are explicitly typed by default; the `infer` keyword provides a visible, static request for type crystallization from unambiguous initializers. Silent or contextual declaration inference is prohibited.
- **Language-level capability sandbox** – Scripts cannot interact with the host OS, filesystem, network, or raw memory unless an explicit capability is granted through the schema. The sandbox is enforced by static type checking and grammar construction.
- **Single-source-of-truth host contracts** – Capabilities, interfaces, and boundary types are declared in `.cm` schema files. The schema governs script validation, host codegen (Rust traits and typed proxies), and API version negotiation.
- **Visible boundary crossing** – Top-level types and functions that cross the host/script boundary are enforced in `PascalCase`. Script-internal types and functions are enforced in `camelCase`.
- **No mod-to-mod coupling** – Multi-file mods cannot import sibling mods directly. Inter-mod communication exists only if the host explicitly exposes an intermediary bridge capability.
- **Strict Ahead-of-Time compilation** – Native artifacts are generated strictly ahead-of-time (AOT) via LLVM and loaded directly as native code. No JIT compilation is performed.

### 1.1. Non-Goals

- A general-purpose standalone application language.
- A package manager or decentralized dependency resolver.
- A sprawling standard library beyond a compact, host-neutral core.
- An internal async runtime, event loop, or thread scheduler.
- Dynamic typing, ambient global state, or hidden control-flow manipulation.

---

## 2. Syntax Overview

Checkmate’s syntax prioritizes visual clarity, regular grammatical structure, and predictability for human authors and large language models (LLMs).

### 2.1. Files and Organization

Source files use the `.cm` extension. There is no implicit global entry point (such as `main()`); host execution targets specific interface functions or exposed entry points.

### 2.2. Comments

```checkmate
// Single-line comment

/*
   Multi-line block comment
*/
```

### 2.3. Imports and Module Namespaces

Imports grant access to host-provided capability namespaces or internal files within the current mod.

```checkmate
import engine.graphics
import engine.input
import self.gamemode.rules
```

- `engine` represents a top-level host schema namespace granted by the host.
- `self` is a reserved root referencing the current mod's internal directory tree (§11.2).

### 2.4. Built-in Scalar Types

```checkmate
int          // Signed 64-bit integer
float        // 64-bit IEEE 754 floating-point number
bool         // true or false
str          // Immutable UTF-8 string
void         // Function returning no value
```

Numeric conversions are strictly explicit; implicit coercions between `int` and `float` are disallowed. Arithmetic operations are overflow-checked by default; runtime integer overflow immediately terminates the invocation.

### 2.5. Boundary Capitalization

A top-level declaration (type, capability function, or interface function) that crosses the host/script boundary is named in `PascalCase`. All script-internal top-level declarations, local variables, struct fields, and function parameters are named in `camelCase`.

```checkmate
struct vec2 {          // Internal type: camelCase
    float x
    float y
}

struct TextureHandle {  // Schema-declared host type: PascalCase
    int id
}

void spawnZombie(vec2 pos) {     // Internal function: camelCase
    engine.graphics.DrawTexture(tex, pos)   // Host capability: PascalCase
}
```

This rule is enforced at compile time against the imported schema:

- A capitalized declaration that is not part of the active schema contract produces a compile-time error.
- A boundary type or function declared or referenced in lowercase produces a compile-time error.

### 2.6. Struct Types

Structs define typed records with named fields. Field definitions and constructor invocations are newline-delimited (no commas or semicolons).

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

Instantiation uses named argument syntax:

```checkmate
vec2 pos = vec2(x: 10.0, y: 5.0)
player p = player(
    name: "Hero"
    position: pos
    health: 100
    alive: true
)
```

### 2.7. Enum Types (Algebraic Data Types)

Enums are tagged unions where variants may carry typed payloads:

```checkmate
enum gameEvent {
    Damage(int amount)
    Heal(int amount)
    Spawn(str enemyKind, vec2 position)
    PlayerDied()
}
```

Constructors are fully qualified with their enum type:

```checkmate
gameEvent evt = gameEvent.Damage(25)
```

Variant names are capitalized payload identifiers; their internal fields remain camelCase.

### 2.8. Option and Result Types

Standard generic sum types for null-safety and error handling:

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

The `?` operator provides early return propagation for `result<T, E>`. The propagated error type must match the enclosing function’s return error type exactly; implicit error transformations are not performed.

```checkmate
result<user, httpError> loadUser(int id) {
    httpResponse response = engine.http.Get($"/users/{id}")?
    return Ok(parseUser(response.body))
}
```

### 2.9. Generics

Structs and enums support generic parameterization:

```checkmate
struct pair<A, B> {
    A first
    B second
}
```

### 2.10. Variables and Mutability

Variables are declared by placing the type before the identifier. Variables are mutable by default.

```checkmate
int score = 0
float speed = 4.5
str title = "Checkmate"
bool active = true

score = score + 10
speed = 2.0
```

### 2.11. Functions

Functions declare return type, identifier, and typed parameters. Statements are newline-delimited without semicolons.

```checkmate
float distance(vec2 a, vec2 b) {
    float dx = b.x - a.x
    float dy = b.y - a.y
    return engine.math.Sqrt(dx * dx + dy * dy)
}

void logMessage(str msg) {
    engine.Log(msg)   // Return keyword optional for void
}
```

### 2.12. Function Arguments

Function calls support either positional or named argument syntax exclusively within a single invocation:

```checkmate
// Positional
user u1 = getUser(42)
movePlayer(p, position)

// Named
user u2 = getUser(id: 42)
movePlayer(
    player: p
    position: position
)
```

Mixing positional and named arguments in the same call (e.g., `getUser(42, name: "Hero")`) is a compile-time syntax error.

### 2.13. Value Semantics in Practice

Arguments and assignments follow logical value semantics. Mutating a local structure or parameter never mutates the caller's instance:

```checkmate
player damage(player p) {
    p.health = p.health - 10
    return p
}

p = damage(p) // Reassignment required to apply update
```

Under the hood, structural sharing and copy-on-write eliminate unnecessary buffer clones until mutation occurs.

### 2.14. Control Flow

```checkmate
if (health <= 0) {
    alive = false
} else {
    alive = true
}

for (enemy e in enemies) {
    updateEnemy(e)
}

while (health > 0) {
    tick()
}
```

### 2.15. Pattern Matching

`match` performs exhaustive destructuring on algebraic data types:

```checkmate
str label = match (event) {
    Damage(int amount) => "Damage"
    Heal(int amount) => "Heal"
    PlayerDied() => "Dead"
    _ => "Unknown"
}

// As a statement
match (event) {
    Damage(int amount) => {
        health = health - amount
    }
    _ => {}
}
```

### 2.16. Type Crystallization (`infer`)

Checkmate forbids implicit declaration inference. When concise local declarations are desired, the `infer` keyword serves as an explicit, visible request for static type crystallization:

```checkmate
infer wow = 10.0                          // Crystallizes to float
infer name = "Hero"                       // Crystallizes to str
infer pos = vec2(x: 10.0, y: 5.0)         // Crystallizes to vec2
```

`infer` is strictly validated:

- The expression must yield an unambiguous static type.
- Uninitialized declarations or empty collections without contextual hints (`infer items = []`) fail compilation with: `cannot infer type for 'items'; ambiguous initializer`.

#### 2.16.1. Formatter-Assisted Auto-Crystallization

The official formatter supports `--auto-crystallize`, transforming `infer` declarations into explicit types across source files:

```checkmate
infer health = 100
infer speed = 4.5
```

is rewritten to:

```checkmate
int health = 100
float speed = 4.5
```

This ensures a reversible workflow: write concise code during rapid iteration, then crystallize explicit signatures for production review.

---

## 3. Memory Management

### 3.1. Logical Value Semantics, ARC, and COW

Checkmate guarantees that values behave as if independently owned. Behind this semantic boundary, memory management is deterministic:

1. **Stack Allocation Bias** – The compiler runs aggressive escape analysis. Any struct, array, or local value that does not escape its defining lexical scope is allocated directly on the native call stack.
2. **Copy-on-Write (COW)** – Dynamic structures (strings, arrays, maps, heap-promoted records) share backing buffers via reference-counted pointers. When mutation is requested on a buffer with a reference count $> 1$, a private shallow copy is materialized prior to mutation.
3. **Automatic Reference Counting (ARC)** – When data escapes local scopes (e.g., returned or stored in long-lived state), reference count increments and decrements are automatically emitted by the compiler. Reference count elision eliminates redundant operations across linear execution paths.

There is no tracing garbage collection, no generational nursery, and no background memory compaction. Memory overhead remains strictly bounded.

### 3.2. Opaque Host Handles

Resources owned and managed by the host application (e.g., GPU textures, audio streams, ECS entities, physics bodies) are represented in scripts as opaque handles.

```checkmate
TextureHandle texture = engine.graphics.LoadTexture("hero.png")
engine.graphics.DrawTexture(texture, pos)
```

- Handles cross the boundary as `PascalCase` scalar or struct wrappers.
- Handles grant zero direct access to raw host pointers or memory.
- Copying a handle copies the identifier/reference, not the underlying host resource. Invalidation, thread safety, and resource teardown are governed entirely by the host API implementation.

---

## 4. Asynchronous Execution Model

### 4.1. The Transparent Yielding Model

Checkmate has no `async` or `await` keywords. Scripts are written in a strictly synchronous, linear style:

```checkmate
user loadUser(int id) {
    httpResponse response = engine.http.Get($"/users/{id}")
    return parseUser(response.body)
}
```

When a host function is registered in the schema with the `suspend` keyword, the compiler recognizes that calling it may pause execution. When invoked, the script yields execution back to the host. The host drives the asynchronous task (e.g., on a Tokio runtime, thread pool, or custom event loop) and resumes the script when the result is available.

### 4.2. Implementation via Compile-Time Continuation Splitting

Checkmate does not pause native call frames, walk runtime stacks, or swap stack pointers. Instead, the compiler performs **compile-time continuation splitting (CPS conversion)** at each suspendable host call.

A function containing suspendable calls is lowered into a sequence of ordinary, non-suspending native functions connected by compiler-generated **continuation structures**.

#### Single Suspension Point

Given the script function:

```checkmate
user loadUser(int id, str authHeader) {
    infer startedAt = clock.Now()
    httpResponse response = engine.http.Get($"/users/{id}", authHeader)
    infer elapsed = clock.Now() - startedAt
    return parseUser(response.body, elapsed)
}
```

The compiler computes the live-out set across `engine.http.Get`. Here, `startedAt` remains live. The compiler emits:

1. **A generated continuation struct**:
   ```checkmate
   struct loadUser$Cont0 {
       float startedAt
   }
   ```
2. **Initial function segment (`part0`)**:
   ```checkmate
   SuspendState loadUser$part0(int id, str authHeader) {
       float startedAt = clock.Now()
       return SuspendState(
           pending: engine.http.Get($"/users/{id}", authHeader),
           continuation: loadUser$part1,
           capture: loadUser$Cont0(startedAt: startedAt)
       )
   }
   ```
3. **Resumption continuation (`part1`)**:
   ```checkmate
   user loadUser$part1(loadUser$Cont0 cont, httpResponse response) {
       float elapsed = clock.Now() - cont.startedAt
       return parseUser(response.body, elapsed)
   }
   ```

`SuspendState` is a small runtime pair containing the pending host future/task handle and a function pointer to the resumption part.

#### Suspension in Control Flow

When a suspendable call occurs within loops or branches, the control flow is lowered into discrete step and continuation functions:

```checkmate
void fetchAll(str[] urls) {
    for (str url in urls) {
        engine.http.Get(url)
    }
}
```

Lowers into an index-carrying continuation:

```checkmate
struct fetchAll$Cont0 {
    str[] urls
    int index
}

SuspendState fetchAll$part0(str[] urls) {
    return fetchAll$step(fetchAll$Cont0(urls: urls, index: 0))
}

SuspendState fetchAll$step(fetchAll$Cont0 cont) {
    if (cont.index >= cont.urls.length) {
        return Done()
    }
    return SuspendState(
        pending: engine.http.Get(cont.urls[cont.index]),
        continuation: fetchAll$resume,
        capture: cont
    )
}

SuspendState fetchAll$resume(fetchAll$Cont0 cont, httpResponse _unused) {
    return fetchAll$step(fetchAll$Cont0(urls: cont.urls, index: cont.index + 1))
}
```

#### Backing Properties

- **Interpreter**: Continuations are ordinary bytecode blocks. No interpreter frames are ever parked or retained in a blocked state.
- **LLVM Native Backend**: Every split segment is an ordinary native function with standard calling conventions. No assembly frame manipulation, custom unwinding, or non-standard ABIs are required.
- **Zero Checkmate Runtime**: The host Rust binding wraps the split chain into a single generated `impl std::future::Future`. The host drives the future with zero scheduling overhead from the language layer.

### 4.3. Concurrency Limits

Scripts cannot spawn unmanaged parallel threads or tasks. Concurrency must be mediated by the host via higher-level batching capabilities (e.g., passing a list of requests to a host function that parallelizes work internally and returns a joined result).

---

## 5. Execution Model

Checkmate provides two complementary execution strategies: a lightweight bytecode interpreter and ahead-of-time (AOT) compilation via LLVM.

```
                  ┌──────────────────────┐
                  │   Checkmate Source   │
                  └──────────┬───────────┘
                             │ Parse & Megaprogram Expansion
                             ▼
                  ┌──────────────────────┐
                  │ Semantic AST / Type  │
                  └──────────┬───────────┘
                             │ Continuation Splitting & Lowering
                             ▼
                  ┌──────────────────────┐
                  │     Checkmate IR     │
                  └─────┬──────────┬─────┘
                        │          │
        Bytecode Gen    │          │ LLVM IR Gen
                        ▼          ▼
             ┌────────────┐      ┌────────────┐
             │  Bytecode  │      │  LLVM AOT  │
             └─────┬──────┘      └─────┬──────┘
                   │                   │ Native Object Emit
                   ▼                   ▼
             ┌────────────┐      ┌────────────┐
             │Interpreter │      │Native Code │
             │ Execution  │      │ (Cached)   │
             └────────────┘      └────────────┘
```

### 5.1. Bytecode Interpreter

The interpreter is a portable, register-based virtual machine:

- Enforces strict memory safety and capability boundaries.
- Executes immediately with zero compilation latency.
- Serves as the fallback engine for platforms disallowing dynamic machine-code loading (e.g., iOS, locked consoles) and for rapid inner-loop development.
- Tracks execution budgets via instruction metering and periodic deadline checks.

### 5.2. Native AOT Compilation via LLVM

The native backend compiles Checkmate IR directly into LLVM IR, applying optimization passes and emitting target-specific machine code:

- **No JIT Compilation** – Compilation occurs strictly ahead of execution (e.g., at build time, mod installation time, or during an explicit host warm-up phase).
- Emits standard object files and shared libraries dynamically linked or mapped by the host process.
- Produces native code that interacts with the host through stable, direct C-ABI function pointers and continuation structures.

### 5.3. Compilation Modes & Terminology

| Mode            | Capability Set              | Description                                                                    | LLVM Required at Runtime? |
| --------------- | --------------------------- | ------------------------------------------------------------------------------ | ------------------------- |
| **Local AOT**   | `codegen` + `artifact-load` | Script is compiled and executed on the local host machine, then cached.        | Yes (Dev / Server)        |
| **Precompiled** | `artifact-load`             | Target loads and executes pre-built native artifacts produced by CI/developer. | No (Production client)    |
| **Interpreted** | `interp`                    | Target executes bytecode directly via the virtual machine.                     | No (Universal)            |

### 5.4. Native Artifact Caching & Compatibility

Precompiled artifacts are tagged with a strict compatibility hash derived from:

- Checkmate compiler and language version
- Runtime ABI hash
- Target architecture, OS, and CPU feature flags
- Host schema hash (version and capability signatures)
- Optimization level and compiler flags

A cached artifact is loaded only when the hash matches the host environment exactly. If any mismatch is detected, the host automatically triggers recompilation or falls back to bytecode interpretation.

### 5.5. Execution Limits and Safepoints

The host can configure precise invocation constraints:

- **Fuel Metering**: A deterministic instruction counter decremented during execution.
- **Wall-Clock Deadlines**: Real-time timestamps evaluated at safepoints.
- **Call-Depth Limits**: Maximum recursion and call stack bounds.
- **Allocation Budgets**: Hard limits on memory managed by the invocation.

In native LLVM artifacts, the compiler injects lightweight cooperative safepoints at function entries, loop backedges, and continuation splits. Native execution is interrupted cooperatively without asynchronous thread termination.

### 5.6. Cooperative Cancellation

A running or suspended invocation can be cancelled by the host at any time. When cancelled:

1. The pending `SuspendState` handle is dropped.
2. The continuation structure is dropped, deterministically decrementing reference counts on all live captured values.
3. If supported by the host, cancellation signals propagate into the active host operation.

### 5.7. Reentrancy Protection

Reentrant calls into the same active Checkmate invocation are prohibited. If a host capability is called by a script, that capability cannot invoke script functions within the same execution context before the original call returns. Concurrent execution across independent invocations is fully supported.

---

## 6. Portability: The Freestanding Tier

Checkmate cleanly separates **Codegen Reach** (targets LLVM can compile to) from **Runtime Reach** (prerequisites needed to execute compiled code).

### 6.1. Hosted vs. Freestanding Targets

1. **Hosted Targets (Windows, Linux, macOS, iOS, Android, Consoles)**:
   Modern consoles and operating systems provide standard platform SDKs with system allocators, real-time clocks, and threading. The Checkmate runtime delegates to Rust’s `std` library directly.
2. **Freestanding Targets (Microcontrollers, Bare-Metal Firmware, Custom Kernels)**:
   Environments without an operating system execute within tightly constrained memory profiles (e.g., $\le 128\text{ KB}$ RAM) without access to `std`.

### 6.2. Pluggable Runtime Trait Boundary

To support freestanding targets without duplicating runtime logic, the core AOT runtime (`cme-runtime`) isolates all OS interactions behind a zero-dependency, `core`-only trait interface:

```rust
// Defined entirely over core types (no_std)
pub unsafe trait CmAlloc {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8;
    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout);
    unsafe fn realloc(&self, ptr: *mut u8, old_layout: core::alloc::Layout, new_size: usize) -> *mut u8;
}

pub trait CmClock {
    fn now_ticks(&self) -> u64;
    fn ticks_per_second(&self) -> u64;
}

pub trait CmTrap {
    fn check_safepoint(&self) -> Result<(), ExecutionError>;
}
```

- **Hosted mode**: A lightweight 10-line pass-through delegates `CmAlloc` to the global allocator and `CmClock` to `std::time::Instant`.
- **Freestanding mode**: The embedder provides static arena allocators, fixed pool allocators, and hardware timer reads (e.g., ARM Cortex-M SysTick or RISC-V CLINT).

### 6.3. Interpreter Scope Restriction

The bytecode interpreter (`cme-interp`) is strictly `std`-only.

- Interpreting code on bare metal is an anti-pattern: microcontrollers lack the storage and dynamic loading infrastructure for untrusted third-party scripts.
- Embedded devices iterate by flashing compiled binaries directly.
- Consequently, the AOT runtime supports freestanding `no_std`, while the interpreter remains focused on hosted development and sandboxed client platforms.

---

## 7. Security and Sandboxing

Checkmate provides a secure execution environment built on language-level isolation:

1. **No Ambient Authority** – Scripts have no intrinsic access to filesystems, network sockets, system calls, environment variables, or platform APIs.
2. **Strict Capability Gating** – Host APIs are accessible exclusively through explicit schema imports granted by the host application.
3. **No Pointer Arithmetic or Raw Memory Access** – The language grammar does not express raw pointers, pointer arithmetic, or unchecked type casting.
4. **No Shared Mutable Memory** – Scripts cannot hold references to raw host memory or share mutable variables across threads. Data crossing the boundary is copied or passed as opaque handles.
5. **Deterministic Denial-of-Service Defense** – Fuel meters and deadlines guarantee termination of infinite loops or recursive blowups.
6. **Honest Isolation Guarantees** – Language-level capability sandboxing prevents unauthorized API access by construction. However, when executing untrusted code under an adversarial threat model, language sandboxing should be paired with OS-level process isolation to protect against speculative hardware vulnerabilities and low-level runtime defects.

---

## 8. Megaprogramming

Checkmate supports compile-time syntax transformations (megaprograms) that allow developers to embed domain-specific grammars that expand into type-checked Checkmate code. Megaprograms are purely script-side; they cannot access host capabilities or observe ambient compiler state.

```
       Source Code with magic(...)
                   │
                   ▼
       ┌────────────────────────┐
       │ Megaprogram Expansion  │ ◄── Pure syntax matching & template expansion
       └───────────┬────────────┘
                   │
                   ▼
       Standard Checkmate AST
                   │
                   ▼
       ┌────────────────────────┐
       │ Name Resolution & Type │ ◄── Validated against Host Schema
       │        Checking        │
       └────────────────────────┘
```

### 8.1. Declaration and Invocation (`magic`)

Megaprograms are declared using the `magic` keyword inside modules, and invoked at call sites using `magic(module.name) { ... }`.

```checkmate
// Declaration inside module `agent`
magic spawn($tag model, $ident effort, $text prompt) {
    engine.SpawnAgent(model: "$model", effort: "$effort", prompt: $prompt)
}

// Invocation in consumer code
magic(agent.spawn) {
    model: claude-opus-latest
    effort: high

    Hi. Coordinate the player NPC patrol routes.
}
```

### 8.2. Syntax Patterns and Fragment Kinds

The parameter list of a `magic` declaration defines its pattern. Tokens and match fragments define the expected syntax:

```
$ident                Checkmate identifier
$expr                 Standard Checkmate expression (typechecked at call site)
$type                 Checkmate type identifier
$tt                   Single token or balanced delimiter tree
$tag                  Relaxed foreign token (letters, numbers, '-', '.')
$text                 Verbatim text block (uninterpreted)
$raw                  Verbatim text captured for late reinterpretation (§8.5)
$rawvalue<fn(...)>    Late-reinterpreted value resolved via contextual lookup
$template             Text with {{ expr }} expression interpolation islands
$path                 Route syntax (/users/{id}/profile)
$selector<css>        CSS selector supporting '&' context splicing
```

### 8.3. Schema-Validated Contextual Identifiers

Fragment kinds can validate against active host schemas or engine registries at compile time:

```checkmate
$ident<schema.table>   tableName   // Checked against live database schema
$ident<schema.column>  columnName  // Checked against table columns
$ident<css.property>   cssProp     // Checked against known CSS properties
```

### 8.4. Repetition, Optionals, and Choice

```checkmate
each { ... }                     // Match zero-or-more repetitions
each sep "," { ... }             // Repetition with required separator
optional { ... }                 // Enclosed sequence matches atomically as a unit
oneof { A => (...), B => (...) } // Ordered choice branch
```

### 8.5. Backreferences and Constraints (`where`)

Patterns can enforce syntactic match constraints via compile-time `where` clauses:

```checkmate
magic view(
    "<" $tag<HtmlTag> tagName
    ">"
    each { oneof { child => recur view, text => $template } } as children
    "</" $tag<HtmlTag> closeName ">"
) where closeName == tagName {
    engine.RenderElement(
        tag: "$tagName"
        children: [ each in children {
            match (kind) { child => $child, text => engine.TextNode($text) }
        }]
    )
}
```

If `closeName` does not equal `tagName`, compilation fails at the call site: `closing tag 'div' does not match opening tag 'p'`.

### 8.6. Context Blocks and Ancestor State

When recursive megaprograms need ambient data from parent matches (e.g., resolving `&` in nested CSS blocks), ancestor data is explicitly passed via `context { }`:

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

### 8.7. Indentation-Sensitive Matching (`indent { }`)

For embedded languages that rely on significant whitespace (e.g., Python), the `indent { }` combinator captures matching indentation blocks based on column alignment:

```checkmate
magic def("def" $ident fname "(" each sep "," { $ident param } "):" indent { each { recur stmt } as body }) {
    str $fname(each { str $param }) { each in body { $stmt } }
}
```

### 8.8. Editor & LSP Annotations

Patterns can embed metadata hints to provide rich IDE support:

```checkmate
#complete(engine.availableModels)
#hover("Target model identifier, e.g. claude-opus-latest")
$tag model
```

---

## 9. The Host Contract: Schema System

The host-script interface is declared in `.cm` schema files. A schema is the authoritative contract that configures the compiler, drives code completion, and generates Rust and C host bindings.

### 9.1. Namespace-Rooted Schema Architecture

A schema file defines a single top-level namespace root. All declarations within the file are relative to that root:

```checkmate
// File: schemas/engine.cm
schema engine v1.4.0

capability graphics {
    since 1.0.0 TextureHandle LoadTexture(str path)
    since 1.0.0 void DrawTexture(TextureHandle tex, vec2 position)
    since 1.2.0 suspend Image FetchRemoteImage(str url)
}

capability network {
    requires auth   // Capability-to-interface dependency
    since 1.0.0 httpResponse Send(httpRequest request)
}

interface auth {
    since 1.0.0 bool ValidateToken(str token)
    since 1.4.0 optional void InvalidateSession(str token)
}

interface gamemode requires core {
    since 1.0.0 GameState InitGame(GameConfig config)
    since 1.0.0 void OnTick(GameState state, float deltaTime)
}
```

### 9.2. File and Namespace Boundaries

- One schema file represents exactly one namespace root.
- A host providing multiple systems ships distinct files (`engine.cm`, `physics.cm`, `ui.cm`).
- Cross-schema interface dependencies use qualified paths: `interface hud requires ui.widgets`.

### 9.3. Schemas as Full Boundary Modules

Schema files contain boundary-visible declarations:

- All top-level types (`struct`, `enum`), capabilities, and interfaces in a schema are automatically boundary elements and must be named in `PascalCase`.
- Structs and enums declared in schemas define the shared data-interchange layouts across the FFI boundary.

### 9.4. Contract Dependencies (`requires`)

The `requires` keyword enforces contract prerequisites at compile time:

1. **Interface requires Interface** (`interface gamemode requires core`):
   A mod cannot implement `gamemode` unless it also fully implements `core`.
2. **Capability requires Interface** (`capability network requires auth`):
   A script cannot import or call `engine.network.*` unless the mod provides a complete implementation of the `auth` interface.

### 9.5. Versioning and Non-Breaking Evolution

- `since X.Y.Z` tags specify the version when a member was introduced.
- Mod manifests declare the target schema version (e.g., `host_schema_version = "1.2.0"`).
- The compiler hides all capabilities and interface members introduced in versions newer than the mod’s declared version.
- `optional` allows adding new interface functions to schemas in minor updates without breaking older mods that do not implement them.

### 9.6. Generated Host Rust Bindings

The `cme` host build pipeline translates schemas directly into native Rust traits and proxies via a procedural macro:

```rust
// Generated from schema capability `engine.graphics`
pub trait EngineGraphicsCapability {
    fn load_texture(&self, path: String) -> TextureHandle;
    fn draw_texture(&self, tex: TextureHandle, position: Vec2);
    fn fetch_remote_image(&self, url: String) -> impl std::future::Future<Output = Image> + Send;
}

// Generated from schema interface `engine.gamemode`
pub struct EngineGamemodeProxy<'a> { /* ... */ }

impl<'a> EngineGamemodeProxy<'a> {
    pub async fn init_game(&self, config: GameConfig) -> Result<GameState, ExecutionError> { /* ... */ }
    pub async fn on_tick(&self, state: &GameState, delta_time: f32) -> Result<(), ExecutionError> { /* ... */ }
}
```

---

## 10. Multi-File Mod Organization

### 10.1. Directory Structure

A mod is a self-contained directory with a manifest and a `src/` tree:

```
my_game_mod/
├── mod.toml
└── src/
    ├── main.cm
    ├── gamemode/
    │   ├── rules.cm
    │   └── events.cm
    └── ui/
        └── hud.cm
```

### 10.2. Mod Manifest (`mod.toml`)

```toml
name = "advanced_rules"
version = "1.0.0"
checkmate_version = "0.5.0"

[schemas]
engine = "1.4.0"
physics = "1.0.0"
```

### 10.3. Module Paths and File Hierarchy

File paths under `src/` map directly to internal module paths:

- `src/gamemode/rules.cm` is imported as `self.gamemode.rules`.
- `self` is the reserved root of the local mod tree.
- Cross-file imports within the same mod are statically linked during compilation.
- Because Checkmate modules contain no mutable global state, multi-file mods have no static initialization order dependencies.

### 10.4. Implementing Interfaces across Files

A mod satisfies host interfaces using `impl` blocks. Implementations can be distributed across multiple files within the mod:

```checkmate
// File: src/gamemode/rules.cm
impl engine.gamemode {
    GameState InitGame(GameConfig config) {
        return GameState(score: 0, active: true)
    }
}

// File: src/gamemode/events.cm
impl engine.gamemode {
    void OnTick(GameState state, float deltaTime) {
        state.score = state.score + 1
    }
}
```

The compiler unions all `impl engine.gamemode` blocks across the mod tree. If any required interface member is missing or implemented multiple times, compilation fails with an exact diagnostic.

### 10.5. Mod Isolation

Mods cannot import sibling mods. There is no `import other_mod.*` syntax. If inter-mod communication is necessary, the host must expose an explicit mediator capability:

```checkmate
capability engine.modBridge {
    since 1.0.0 void EmitEvent(str eventName, EventPayload payload)
    since 1.0.0 void Subscribe(str eventName)
}
```

---

## 11. Core Library and Serialization (CMON)

Checkmate includes a minimal, host-neutral standard library:

- **Primitives & Collections**: `str`, `int`, `float`, `bool`, arrays (`T[]`), maps (`map<K, V>`).
- **Control Types**: `option<T>`, `result<T, E>`.
- **String Utilities**: Formatting, UTF-8 validation, slicing, search.
- **Math Utilities**: Standard IEEE 754 floating-point operations.

### 11.1. Checkmate Object Notation (CMON)

CMON is the native, human-readable data serialization format for Checkmate structures, sharing the language's exact literal syntax:

```checkmate
Player(
    name: "Hero"
    position: Vec2(x: 100.0, y: 50.0)
    inventory: [
        Item(id: 1, count: 5)
        Item(id: 42, count: 1)
    ]
    settings: {
        "autoSave": true
        "volume": 0.8
    }
)
```

- Schema-aware: Deserializes directly into typed structs and tagged enums.
- Textual and binary representations share identical memory representations under COW buffers.

---

## 12. Crate Architecture and Workspace Layout

The Rust implementation of Checkmate is published under the crate name `cme` (Checkmate Engine). The codebase is partitioned into targeted crates within a Cargo workspace:

```
cme/                                 Workspace Root & Umbrella Crate
├── crates/
│   ├── cme-core/                    AST, IR definitions, types, schema parser
│   ├── cme-compiler/                Lexer, parser, typechecker, megaprogram expander
│   ├── cme-interp/                  Bytecode compiler and VM interpreter
│   ├── cme-codegen/                 LLVM IR lowering and AOT object emission
│   ├── cme-artifact/                Native artifact loader, cache, and validation
│   ├── cme-runtime/                 Memory core (ARC/COW), continuation ABI, no_std traits
│   ├── cme-lsp/                     Language Server Protocol daemon (`cme-lsp` binary)
│   └── cme-dap/                     Debug Adapter Protocol daemon (`cme-dap` binary)
└── Cargo.toml
```

### 12.1. Cargo Feature Matrix

The workspace enables tailored embedding profiles to minimize binary size and eliminate unnecessary dependencies:

```toml
[features]
default = []
interp        = ["dep:cme-interp"]
codegen       = ["dep:cme-codegen"]
artifact-load = ["dep:cme-artifact"]
local-aot     = ["codegen", "artifact-load"]
lsp           = ["dep:cme-lsp"]
dap           = ["dep:cme-dap"]
cli           = ["dep:clap"]

# Standard production profile: Interpreter + Precompiled native loading (No LLVM dependency)
production    = ["interp", "artifact-load"]

# Full development toolchain profile
full          = ["interp", "local-aot", "lsp", "dap", "cli"]
```

### 12.2. Embedding Scenario Matrix

| Host Deployment Target                       | Cargo Features           | LLVM Linked? | Execution Capabilities                                                   |
| -------------------------------------------- | ------------------------ | ------------ | ------------------------------------------------------------------------ |
| **Local Dev & CI Build Machines**            | `full`                   | Yes          | Bytecode, On-Device AOT Compilation, Artifact Emission, LSP, DAP         |
| **Production Game Client / Desktop**         | `production`             | **No**       | Fast Bytecode Interpreter + Direct Loading of Precompiled LLVM Artifacts |
| **Interpreter-Only Host (iOS / Strict Web)** | `interp`                 | **No**       | Bytecode Interpretation only (zero native codegen/loading)               |
| **Precompiled Native Host**                  | `artifact-load`          | **No**       | Direct Native Execution of Precompiled Artifacts only                    |
| **Freestanding Bare Metal / RTOS**           | `cme-runtime` (`no_std`) | **No**       | Embedded AOT Runtime with Custom Allocator/Clock traits                  |

---

## 13. Host Integration APIs

### 13.1. Rust Host API

Embedding Checkmate in a Rust application revolves around the `Engine` handle provided by `cme`:

```rust
use cme::{Engine, ExecutionLimits};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::new();

    // Register capability trait generated from schema
    engine.register_capability::<EngineGraphics>(graphics_service);

    // Load compiled mod artifact
    let mod_artifact = engine.load_mod("mods/survival_mode.cma")?;

    // Create execution context with resource budgets
    let mut context = engine.create_context(&mod_artifact, ExecutionLimits {
        fuel: Some(1_000_000),
        deadline_ms: Some(50),
        max_call_depth: 64,
    });

    // Invoke interface proxy
    let gamemode = context.get_interface::<EngineGamemodeProxy>()?;
    let game_state = gamemode.init_game(GameConfig::default()).await?;

    Ok(())
}
```

### 13.2. C Host API

For C and C++ hosts, `cme` exposes a stable C ABI with explicit future polling:

```c
#include "cme.h"

void run_tick(cm_engine_t* engine, cm_context_t* ctx) {
    cm_future_t* future = cm_invoke(ctx, "engine.gamemode", "OnTick", NULL, 0);

    cm_poll_result_t result;
    while ((result = cm_future_poll(future, NULL)) == CM_PENDING) {
        // Drive host async loop / work queue
        drive_host_io();
    }

    if (result == CM_ERROR) {
        cm_error_t err = cm_future_get_error(future);
        printf("Script execution failed: %s\n", err.message);
    }

    cm_future_destroy(future);
}
```

---

## 14. Tooling and Developer Experience

1. **Language Server (`cme-lsp`)**:
   Provides semantic tokenization, real-time diagnostics, schema-aware completion, hover tooltips, and megaprogram expansion previews.
2. **Debug Adapter (`cme-dap`)**:
   Supports line breakpoints, step-debugging across continuation split parts, variable inspection, and call stack reconstruction.
3. **Official Formatter**:
   Maintains canonical code formatting and handles automated type crystallization (`--auto-crystallize`).
4. **LLM-Optimized Syntax**:
   Regular C-like syntax, explicit typing, absence of complex lifetime annotations, and deterministic grammar ensure high-accuracy code generation by modern LLMs.

---

## 15. Conclusion

Checkmate establishes a balanced design space for embeddable scripting:

- It provides the safety, static typing, and algebraic data modeling of modern systems languages without the surface burden of manual lifetime annotations or complex borrow checkers.
- It delivers the linear readability of synchronous scripting while compiling to zero-overhead native continuations that compose seamlessly with host async executors.
- It guarantees robust security through capability-gated imports, value semantics, and instruction metering.
- Through modular crate architecture and an isolated `core`-only trait boundary, it spans effortlessly from high-performance game engines down to bare-metal microcontrollers.

Checkmate gives host applications complete control over execution, resources, and concurrency, delivering on a singular design mandate: **the scripting language where the host calls the shots.**
