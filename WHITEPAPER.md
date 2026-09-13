# Checkmate: A Static, Safe, Embeddable Scripting Language

**Version 0.6**

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

A running or suspended invocation can be canceled by the host at any time. When canceled:

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

- Interpreting code on bare metal is an antipattern: microcontrollers lack the storage and dynamic loading infrastructure for untrusted third-party scripts.
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

Checkmate's megaprogramming system is a declarative macro facility designed around one ambition: **any formal language — HTML, CSS, JavaScript, TypeScript, JSON, YAML, TOML, regular expressions — must be embeddable as a megaprogram, and the definition of that embedding must itself be pleasant to read.**

The acid test is not "can macros generate boilerplate" but "can a macro author write a grammar for a real-world language, with every feature, in a page of pattern code, and get precise error messages pointing into that language's source." Checkmate passes by construction: the standard library ships `std.json`, `std.yaml`, `std.toml`, `std.re`, `std.html`, `std.css`, `std.js`, and `std.ts` — none of which use a private compiler hook. They are ordinary megaprograms.

Megaprograms are purely script-side: no host capabilities, no ambient compiler state. Expansion happens after parsing and before name resolution, so generated code is type-checked against the host schema exactly like hand-written code (§5):

```text
        mega(name) { …region… }
                     │
                     ▼
        ┌──────────────────────────┐
        │ Region Scan & Normalize   │  brace balancing under the composed
        │ (string/comment/island    │  profile of every grammar the entry
        │  forms of every grammar)  │  pattern touches; edge trim (§8.6)
        └────────────┬─────────────┘
                     ▼
        ┌──────────────────────────┐
        │ Packrat Pattern Match     │  grammar rules → capture tree
        │ (deterministic, memoized) │  every capture carries source spans
        └────────────┬─────────────┘
                     ▼
        ┌──────────────────────────┐
        │ Template Elaboration      │  capture tree + template → Checkmate AST
        │ (@-calls run in the       │  spans remapped to the call site
        │  sandboxed interpreter)   │
        └────────────┬─────────────┘
                     ▼
        repeat until no mega() invocation remains
                     ▼
        Name Resolution & Type Checking  (validated against host schema)
```

Design tenets:

1. **The read-aloud test.** Patterns are built from keywords (`each`, `optional`, `oneof`, `where`, `indent`, `soft`), never sigil soup.
2. **Character-precise, line-honest grammars.** Matching operates on raw UTF-8 text; character classes, maximal-munch scans, verbatim runs, and indentation blocks are first-class — and line boundaries remain _visible_ to a grammar until it explicitly softens them. Languages whose semantics are defined over line boundaries (JavaScript's ASI, TOML's tables, YAML's blocks) must be able to see those boundaries.
3. **Zero private hooks.** The shipped grammars use only the public system.
4. **Deterministic and terminating.** Ordered choice plus packrat memoization; compile-time computation under §5.5 budgets, which are operation counts, never wall-clock time.
5. **Diagnostics are first-class.** Errors inside embedded languages point at the embedded source.
6. **Purity.** No host access, no I/O — identical expansion on every platform, keeping §5.4 artifact hashes reproducible.

Existing macro systems offer these powers separately (token-level patterns, unrestricted compile-time languages, external grammar frameworks, template languages); Checkmate unifies all four in one declarative, span-faithful surface.

### 8.1. The Three Artifacts

| Artifact              | Role                                                      | Analogy             |
| --------------------- | --------------------------------------------------------- | ------------------- |
| `grammar`             | A named library of matching rules                         | The lexer + parser  |
| `mega`               | An entry point binding a pattern to an expansion template | The semantic action |
| Compile-time function | An ordinary pure Checkmate function invoked with `@`      | The code generator  |

Simple megaprograms need only a `mega` declaration — pattern in the parentheses, template in the braces:

```checkmate
mega agent.spawn(
    #complete(engine.availableModels)
    #hover("Target model identifier, e.g. claude-opus-latest")
    "model:" $tag model
    "effort:" $word effort

    $text prompt
) {
    engine.SpawnAgent(
        model: $"{$model}"
        effort: $"{$effort}"
        prompt: $prompt
    )
}

// Consumer code:
mega(agent.spawn) {
    model: claude-opus-latest
    effort: high

    Hi. Coordinate the player NPC patrol routes.
}
```

`#complete` / `#hover` are inert editor metadata consumed by `cme-lsp` (§8.9); they never affect matching, and they are the _only_ place host registries are visible (§8.5). Invocation is `mega(name) { … }`; the declaration form `mega name(…) { … }` is distinguished by the identifier between `mega` and `(`.

**Names.** megas are module-scope declarations, qualified by their module: the `value` macro of `std.json` is invoked as `mega(json.value)`. Grammar rules are qualified by their grammar: `json.value`. The two namespaces never meet — `mega(…)` resolves macro names, pattern positions resolve rule names — and the standard library names each grammar after its module, so the spellings coincide in §8.8's examples. Which is intended is decided by syntactic position, never by search. Invocation names resolve at parse time (§8.6): a macro must be imported, or declared earlier in the same file, before it is invoked.

### 8.2. Grammars

A grammar is a named, importable library of rules. Grammars and rules are script-internal and follow the `camelCase` enforcement of §2.5. Five lexical declarations define a grammar's **profile**:

```checkmate
// File: std/css.cm  (excerpt — full treatment in §8.8)
import std.re

grammar css {
    skip    [ ' ', '\t', '\r', '\n' ]     // the skip set
    comment ( "/*" until "*/" )            // a comment form
    string  ( '"' )                        // a string form: opens and closes on
    string  ( "'" )                        // the delimiter, backslash escapes
    island  ( "${" "}" )                   // honored, single-line by default
                                           // an interpolation island
    rule styleRule(context { selector parent = none }) {
        selector sel "{"
        each { declaration } as decls
        each { styleRule with context { parent: sel } } as nested
        "}"
    }
    // rule selector, rule declaration, rule value, …
}
```

**Line-oriented vs. flow-oriented.** If the skip set contains a line terminator, the grammar is _flow-oriented_: newlines are layout, skipped between elements, and `eol`/`line`/`indent` are unavailable. Otherwise it is _line-oriented_: the skipper never crosses a line boundary, line structure is visible, and `eol`, `line`, and `indent` are first-class. This is the most consequential choice a grammar author makes:

- JSON, CSS, and HTML are flow-oriented.
- YAML, TOML, Python — and, necessarily, JavaScript — are line-oriented. ASI and restricted productions are _defined_ over line boundaries; a grammar that cannot see them cannot express them. Inside a line-oriented grammar, `soft { p }` (§8.3.2) grants newline-skipping exactly where the language allows it — inside brackets, after operators — so line-visibility costs nothing where newlines are free (§8.8 demonstrates the full JS line grammar in this style).

**Comments.** In flow grammars, comment forms are consumed by the skipper. In line grammars they are _not_ auto-skipped; instead:

- a **transparent line** — a line containing only skip-set characters, or only skip-set characters plus one or more comment forms — is skipped by `eol`, `eof`, and the `indent` block protocol (§8.3.5), and
- `eol` itself consumes the line terminator and the entire following **transparent tail** (skip-set characters and zero or more consecutive comment forms) (§8.3.2).

Interior comments are matched explicitly, as pattern alternatives. The consequence: a line-oriented grammar never strands a comment on a line boundary, and a comment line — at _any_ column — can neither open, close, nor be swallowed by an indented block. §8.3.1's worked TOML example demonstrates the split.

**String forms** are consumed by the profile's consumers: the invocation-region scanner of §8.6 and `$tt` balancing. They never run in the skipper; in-pattern matching uses fragments and explicit rules, so `"a # not a comment"` in TOML and `content: "/* keep */"` in CSS are settled by atomicity alone (§8.3.1).

**Islands.** String forms may declare an **island** (e.g., `island ( "${" "}" )`). The region scanner (§8.6) treats the island delimiters as transparent to brace balancing, allowing nested macro invocations inside interpolations to be discovered and expanded.

- **Rules** reference each other by bare name (recursion is a self-reference) or qualified name across grammars. `recur` is sugar for the innermost enclosing rule.
- Grammars **extend** others: `grammar ts extends js { rule type { … } }` — overriding or adding rules. This is how user megaprograms patch the shipped grammars. **Lexical profile inheritance:** a child grammar inherits the parent's `skip`, `comment`, `string`, and `island` declarations. The child may override them or append to them.
- **Profile inheritance:** a mega whose entry pattern is a rule reference inherits that grammar's lexical profile for matching _and_ region scanning; a mega with an inline pattern uses the default profile (horizontal and newline skipping, `"` strings, no comments). `indent`, `eol`, and `line` are reachable only through line-oriented grammars, and the compiler rejects them in flow-oriented contexts and inside `soft`.

### 8.3. The Pattern Language

#### 8.3.1. Match Domain, Skipping, and Atomicity

Matching operates on a cursor over raw region text. Three rules govern the skipper, and the first is exhaustive — the grammar's correctness must never depend on an unstated case:

1. The skipper runs **immediately before exactly these elements**: literals and `i"…"` literals, classes, fragments, rule references, groups, `oneof` branches, `each` iterations, `optional` attempts (`optional` ≡ `each [0,1]`), `soft` regions, and `indent` starts. It **never** runs before `eol`, `eof`, `line`, `where`, `label`, or `peek`/`not` — those observe the cursor as it stands (`peek`/`not` apply the enclosing mode's skipper tentatively _within_ their subpattern and then restore it). `raw { p }` suspends the skipper entirely within `p`; a rule referenced inside `p` applies its own grammar's skipper within its own match — the delegation boundary is exempt.
2. **Atomic matchers never skip internally.** `$str`, `$word`, `$tag`, `$int`, `$float`, `$ident`, `$text`, `$raw`, `$expr`, `$type`, `$block`, `scan`, `until`, `lineRest`, `$tt`, `$template`, and `raw { … }` regions consume their extent in one step; skip-set characters and comment forms inside that extent are data, not syntax. `soft { p }` is likewise atomic: if `p` fails, all of its skips are undone.
3. Zero-width assertions (`peek`, `not`, `line`) restore the cursor entirely.

Rule 2 settles the comment-in-string question by construction: the skipper only ever inspects text at element boundaries, and by the time the boundary after a string is reached, the string — `#` and all — has already been consumed atomically:

```toml
motto = "a # not a comment"   # but this is
```

```text
keyval → dottedKey(motto) → "=" → value → string → $str
    [atomic: consumes "a # not a comment", '#' included]
→ eol    [remainder is one comment form; consumes the line end]
```

The same invariant covers JSON (`"http://x#frag"`), CSS (`content: "/* keep */"`), and the region scanner of §8.6. In line-oriented grammars the trailing comment is consumed by `eol` itself, and comment-only lines are transparent to the line machinery — there is no stranding hazard and no comment that can eat a block terminator.

The remaining hazard is **loose string matching** — building strings from element sequences, where rule 1 lets the skipper eat string-internal content. Loose matching must be atomic or sealed:

```checkmate
// wrong: the skipper runs before `any`, eating spaces inside the string
rule badString { "\"" each { not { "\"" } any } as chars "\"" }

// right: an atomic fragment, or a raw region
rule goodString { $str text }
rule alsoGood   { "\"" raw { each { not { "\"" } any } as chars } "\"" }
```

#### 8.3.2. Combinators

| Form                                                                      | Matches                                                                                                                                                                        | Capture              |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------- |
| `"lit"` / `i"lit"`                                                        | exact character sequence (`i`: case-insensitive)                                                                                                                               | —                    |
| `[a-z0-9_]` / `[^…]`                                                      | exactly **one** character in / not in the set                                                                                                                                  | `as name`            |
| `any`                                                                     | any single character                                                                                                                                                           | `as name`            |
| `scan […]`                                                                | maximal run of ≥ 1 characters from the set                                                                                                                                     | text                 |
| `until "lit"` / `until { p }`                                             | verbatim run stopping at the first position where `lit` / `p` matches **as a whole**; the stop condition consumes nothing                                                      | text                 |
| `lineRest`                                                                | verbatim run to end of line (terminator excluded)                                                                                                                              | text                 |
| `eol`                                                                     | _line mode only._ Consumes the terminator (or matches at region end) and the entire following **transparent tail** (skip-set characters and zero or more comment forms)        | —                    |
| `line`                                                                    | _line mode only, zero-width._ A non-skip-set character remains before the line's terminator                                                                                    | —                    |
| `eof`                                                                     | only skip-set characters and transparent lines remain to region end; consumed                                                                                                  | —                    |
| `soft { p }`                                                              | _line mode only._ Newlines join the skip set within `p`; atomic (failed skips are undone)                                                                                      | —                    |
| `optional { p }`                                                          | `p` or nothing, atomically                                                                                                                                                     | `opt` capture (§8.4) |
| `each { p }`, `each+`, `each sep pattern trailing { p }`, `each [n, m]`   | repetition; separator (literal or pattern); trailing separator; bounds                                                                                                         | list `as xs`         |
| `oneof { label => ( p ) … }`                                              | first matching branch, in declaration order                                                                                                                                    | tagged record        |
| `peek { p }` / `not { p }`                                                | zero-width positive / negative lookahead                                                                                                                                       | —                    |
| `( p )`                                                                   | grouping                                                                                                                                                                       | `as name`            |
| `ruleName`, `grammar.ruleName`                                            | rule reference; recursion; delegation                                                                                                                                          | `as name`            |
| `recur`                                                                   | innermost enclosing rule                                                                                                                                                       | —                    |
| `indent { p }` / `indent verbatim`                                        | indentation-delimited block (§8.3.5)                                                                                                                                           | record / text        |
| `raw { p }`                                                               | `p` with the skipper suspended                                                                                                                                                 | —                    |
| `where cond`                                                              | validates captures in scope (§8.3.4)                                                                                                                                           | —                    |
| `context { … }` / `with context { … }`                                    | ancestor data (§8.3.7)                                                                                                                                                         | —                    |
| `label "msg" { p }`                                                       | diagnostic context for failures inside `p` (§8.3.9)                                                                                                                            | —                    |

**Ordered choice with complete fall-through.** A `oneof` tries branches in declaration order; a branch fails if _any_ element of its sequence fails — a literal, a fragment, a `where`, anything — and the next alternative is tried. **There is no point in the system at which entering a branch becomes irreversible.** Indented-block failures that "look" committed (§8.3.5) are backtrackable like any other; commitment is a diagnostic annotation, not control flow. Prefix-colliding alternatives (`(?<!`, `(?<=`, `(?<name`) are written longest-prefix-first, which is precisely how ordered choice stays deterministic.

#### 8.3.3. Fragments

Fragments are pre-parameterized matchers that bind a capture name directly: `$tag name`, `$str key`, `$raw value`. Any word/tag/ident fragment accepts an `i` prefix — `i$tag name` — which matches case-insensitively and folds the capture to lowercase; this is how HTML's case-insensitive tag names stay first-class (§8.8).

| Fragment                                   | Extent / what it matches                                                                                                                            | Capture kind  |
| ------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------- | ------------- |
| `$ident`                                   | a Checkmate-valid identifier token                                                                                                                  | ident         |
| `$word`                                    | foreign identifier: `[A-Za-z_$][0-9A-Za-z_$]*`                                                                                                      | text          |
| `$tag`                                     | relaxed foreign token: letters, digits, `-`, `.`, `_`                                                                                               | text          |
| `$int` / `$float`                          | numeric literal forms (decimal, fractional, exponent; `0x`/`0o`/`0b` prefixes)                                                                      | int / float   |
| `$str`                                     | double-quoted string with backslash escapes                                                                                                         | str           |
| `$tt` / `$tt<"{{" "}}">`                   | a single token or balanced delimiter tree, honoring the grammar's profile string forms                                                              | text          |
| `$text`                                    | effective tail if one exists, else remainder of region                                                                                              | text          |
| `$template` / `$template<open close rule>` | same extent, split at island delimiters; islands parsed by `rule` (default `{{ }}` + Checkmate expression); `\{{` / `\}}` escape literal delimiters | tagged parts  |
| `$raw` / `$raw<grammar.rule>`              | effective tail (§8.3.6), parsed as Checkmate code / by the referenced rule — boundary search is parse-integrated                                    | code / record |
| `$expr` / `$type` / `$block`               | a live Checkmate island: the same tail-bounded, parse-integrated extent, parsed by the real Checkmate parser                                        | code          |

Any fragment accepts a **validator** — `$ident<self.notReserved>`, `$word<std.html.voidTag>` — naming a grammar rule (the matched text must match it) or a pure function from the same module or core library. Validators never touch host schemas: §8.5's purity rule is absolute, and host registries are reachable only through inert `#complete` metadata.

Live islands make embedded data contain real, type-checked Checkmate expressions:

```checkmate
mega ui.banner($template body) {
    ui.compound([each in body {
        match ($item) {
            text => ui.label($"{$item.text}")
            expr => ui.live($item.value)
        }
    }])
}

ui.banner {
    Welcome back, {{ playerName }}!
    You have {{ player.score }} points.
}
```

#### 8.3.4. Constraints: `where` and `require`

**`where` is an ordinary pattern element.** It consumes no input; it succeeds if its condition — a compile-time expression over captures — is truthy, and fails otherwise. A failing element fails its enclosing sequence. Inside `oneof`, a branch whose sequence fails falls through to the next alternative **for any reason, including a failed `where`**.

The scope of a `where` condition is:

- every capture bound anywhere within the **current rule invocation** — earlier in the same sequence, in enclosing groups, in the `oneof` branch currently being attempted, or in earlier iterations of an enclosing repetition; and
- the rule's declared `context` (§8.3.7).

Captures from the _calling_ rule are deliberately out of scope. This restriction makes a rule's result a function of (position, context, skip mode) alone, which is what keeps packrat memoization sound (§8.7).

Conditions may use equality and comparison, `&&`/`||`/`!` (Appendix A), `some x in xs { … }` / `all x in xs { … }`, `present(x)` for optional captures (§8.4), capture accessors (`.line`, `.col`, `.span`), and calls to pure functions (§8.5).

Worked example — the HTML `element` rule of §8.8 against four inputs:

| Input        | Behavior                                                                                                                                                                                      |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `<br>`       | `styleEl`/`scriptEl` fail their `where name == …`; `selfClose` fails its literal `"/>"`; `voidEl`'s `where @isVoid(name)` succeeds and `">"` matches. Match.                                  |
| `<br/>`      | Same fall-through; `selfClose` matches `"/>"`. Match.                                                                                                                                         |
| `<p>…</div>` | Earlier branches fail; `normal` matches through `…`, then `where close == name` fails with no alternatives left. Whole-match failure; the constraint is the furthest failure and is reported. |
| `<div/>`     | Every branch fails; the furthest failure is the literal `">"` at the `/`, with the `isVoid` constraint also recorded.                                                                         |

The first three rows are why `where` must backtrack out of branches: the void-element checks are _branch selectors_, not validators.

**Division of labor.** Templates may call `require(cond, "message")` (§8.4), which emits a compile-time error anchored at a capture's span. The decision procedure is mechanical:

| The check…                                                                                                                    | Mechanism                                                              |
| ----------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| can be decided from the current rule instance's captures (+ context), and must be able to steer which alternative matches     | `where`                                                                |
| needs captures from any other rule instance — siblings, ancestors, the whole tree — and only needs to reject the final result | template `require`                                                     |
| is cross-instance but must steer matching                                                                                     | accumulate a list downward through `context` and check it with `where` |

The third row's syntax: `context { str[] open }`, extended at each recursion via

```checkmate
recur with context { open: append(open, name) }
```

Binding values in `with context` are captures or compile-time expressions over in-scope captures and the current context — that expression form is what makes accumulation possible.

The underlying invariant: **matching is stateless across rule instances.** No rule observes another instance's captures; everything cross-instance is either explicit downward context or post-match validation. This is the same property that keeps memoization sound, so the division is load-bearing, not stylistic.

Under this procedure: HTML's closing-tag check is a `where` (single instance, and it selects branches); TOML's table-reopening check is a `require` (needs every header in the document); YAML's alias resolution is a `require` (needs the whole tree); an "unclosed ancestor tag" checker is either a `require` (if rejecting suffices) or the context-carried open-tag list above (if it must steer). Prefer `require` when rejecting suffices — it reports at the offending node instead of at a backtracked position.

#### 8.3.5. Indentation

`indent` maintains a **block stack** of the base columns of all currently open indented blocks. Column is measured in visual spaces: tab characters (`\t`) advance the column to the next multiple of 8. However, if a line's indentation prefix mixes tabs and spaces, it triggers a committed-block failure ("mixed tabs and spaces in indentation").

The protocol for `indent { p }`:

1. **Start.** If the cursor is mid-line, `B` is the current column and matching begins at the cursor. Otherwise the cursor must be at end of line; `indent` consumes the newline, skips **transparent lines** (§8.2), and `B` is the column of the next non-transparent line (none remaining → the block matches empty).
2. **Depth.** If a block is open, `B` must be **strictly greater** than its base column; otherwise this element fails — an ordinary, backtrackable failure. This rule is what separates a nested block from a following sibling.
3. **Iteration.** The cursor is placed at the start of the next non-transparent line, whose column must equal `B`; `p` is matched; only skip-set characters or one comment form may remain on the line, and the protocol then consumes the terminator and any transparent lines. A failing iteration is discarded and backtracked.
4. **Termination.** Let `L` be the next non-transparent line after the last completed iteration (or none):
   - **No `L`, or column(`L`) < `B`** — clean end, cursor at the start of `L`. Every open block whose base exceeds column(`L`) also ends cleanly at `L`; the innermost block whose base equals column(`L`) claims `L` as its next iteration. A dedent is never an error, and one dedented line may close several nested blocks at once. Comment-only lines are transparent: they close nothing, open nothing, and are skipped wherever `L` is sought — so the Python idiom of an outdented comment inside a block, and the YAML idiom of a comment at any column between entries, both parse.
   - **column(`L`) ≥ `B` with ≥ 1 completed iteration** — the block fails. The failure is backtrackable like any element failure, but it records a **committed-block diagnostic** — "line at column c belonged to this block and could not be parsed" — which §8.3.9's reporter prefers over ordinary furthest failures. Commitment is a report, not control flow: `oneof` fall-through, `peek`, and `$raw`'s speculative tails all observe an ordinary failure. This keeps the system's backtracking story uniform (no exceptions) _and_ keeps the error quality the commitment was invented for.
   - **column(`L`) ≥ `B` but does not match the base column of any currently open block** — committed-block failure: "indentation level does not match any open block". This prevents silent misalignment in No-Man's Land.
   - **column(`L`) ≥ `B` with 0 iterations** — ordinary failure: the block could not start; the enclosing sequence backtracks and may try other alternatives.

`indent verbatim as name` follows steps 1–2 restricted to end-of-line starts (empty match permitted only at region end), then captures verbatim every line — blank lines _and comment lines_ included — with column ≥ `B`, ending at the first non-transparent line with column < `B`. Comment lines are content inside a verbatim block; this covers YAML block scalars and Python-style raw bodies.

Static checks: rules containing `indent` are memoized with the enclosing base column in the key (§8.7); `indent`/`eol`/`line` require line mode; and `indent` may not directly follow `eol` in a sequence — `indent` performs its own line advancement, and the check catches the double-advance mistake at compile time.

Worked example (`std.yaml`):

```yaml
title: Checkmate
limits:
  fuel: 1000000
deadlineMs: 50
```

The outer mapping opens at the document root with a mid-line start, `B` = 0. `title`'s value takes the same-line branch (`peek { line }` — content remains on the line). `limits`'s value: the same-line branch fails at end of line, the block branch opens a nested map with `B` = 4 — strictly deeper than 0. After `fuel`, the next line `deadlineMs` is at column 0 < 4, so the nested block ends cleanly and the outer map (base 0) claims it. Had `limits` been followed directly by a sibling at column 0, the nested map's `indent` would compute `B` = 0, violating step 2, and the value would fall back to empty (null) — **instead of swallowing the rest of the document.** That is the ambiguity step 2 exists to kill.

#### 8.3.6. Verbatim Capture and `$raw`

`$raw` looks like the most novel primitive in the system; it is sugar over `until`, plus a parse:

```text
( … e ($raw x) t1 t2 … )   ≡   ( … e (until { t1 t2 … } as x) … ) + parse x
```

**Effective tail.** The tail is the sequence of siblings following the `$raw`, extended outward through enclosing groups and repetition bodies until non-empty. Inside `each sep "," { … $raw x … } as xs ")"`, the tail is "the separator-continued repetition, or `")"`". A `$raw` with an empty tail (nothing follows it anywhere in the pattern) is a compile-time error: `raw capture requires a following terminator; use $text or until`. `$text` and `$template` use the same extent computation but fall back to the region remainder when no tail exists.

**Boundary semantics — parse-integrated.** The scan is lazy and first-boundary, but a candidate boundary is accepted only when _both_ hold: the tail matches **as a whole** at that position, **and** the captured text parses. This is what makes nested structures work. In `f(g(x), y)`, the boundary at the inner comma is rejected — the tail matches, but `g(x` does not parse — and the scan continues to the outer comma, where `g(x)` does. A tail `":" indent { … }` does not stop at a `:` that is not followed by end-of-line-plus-deeper-block; in §8.4's grammar, `if a ? b : c:` therefore captures `a ? b : c`, not `a ? b`.

If no boundary is accepted, the element fails at the furthest position where the tail matched, reporting the parse error there — the boundary the input most nearly reached.

**Parsing.** Captured text is parsed at capture time. `$raw` parses as Checkmate code; `$raw<grammar.rule>` delegates to the referenced rule. `$expr`, `$type`, and `$block` are live islands with the same extent rule and the real Checkmate parser as the parse step. **For Checkmate expressions, prefer `$expr`:** the real parser's delimiter tracking is exact where `$raw`'s boundary search is a heuristic — a heuristic that handles nesting correctly (above) but at cost.

**Speculation and the cycle cut.** Tail matching is speculative — it succeeds or fails without consuming. If speculative evaluation reaches the same `$raw`/`until` instance at the same position — possible when a separator-less repetition body begins with a `$raw` — the re-entrant attempt fails. This keeps `$raw` total. The documented consequence: a separator-less `each { $raw x }` merges greedily into one capture; use `sep` to separate items.

**Cost.** `until` is O(characters × tail cost) before memoization — and with parse integration, × the parse cost of each candidate — so packrat memoization of tail attempts is what bounds the total; the worst case remains quadratic in region size. Because speculative tail matching can be expensive, the compiler statically warns if a `$raw` tail contains unbounded repetitions or complex recursive rules. If a `$raw` exhausts the §5.5 compile-time fuel limit during boundary search, it terminates with a budget error rather than hanging, and the diagnostic explicitly names the `$raw` instance and the furthest boundary attempted.

#### 8.3.7. Ancestor Context

Recursive rules needing ambient data — the parent selector when resolving `&` in nested CSS — declare it explicitly:

```checkmate
rule styleRule(context { selector parent = none }) {
    selector sel "{"
    each { declaration } as decls
    each { styleRule with context { parent: sel } } as nested
    "}"
}
```

Context flows only downward, is always explicit, and is visible to `where` clauses and templates. Fields may declare defaults (`= none`); a call without `with context` uses them — so a top-level `styleRule` (no parent) and a delegated one compose without special cases. Binding values are captures or compile-time expressions over in-scope captures and the current context — the sanctioned mechanism for the third row of §8.3.4's decision table.

#### 8.3.8. Delegation and Composition

A qualified rule reference inside a pattern invokes another grammar inline:

```checkmate
import std.json

grammar conf {
    skip    [ ' ', '\t' ]
    comment ( "#" )

    rule file {
        each { oneof { setting => setting, include => include } } as entries
    }

    rule setting {
        $word key "=" json.value as value
        eol
    }

    rule include {
        "include" $str path
        eol
    }
}
```

Delegation relies on **grammar-local termination**: a sub-grammar's top-level repetition stops when the next characters cannot start another rule of that grammar — a JSON value ends exactly where JSON's structure closes, so control returns precisely at conf's line end. The sub-grammar's own skipper applies within its match (json's is flow-oriented, so a value may legally span lines); the host's line discipline resumes at the boundary. A line-oriented host and a flow sub-language compose without friction, and both directions are used by the standard library: the JavaScript grammar delegates to `re.literal` for regex literals; the conf grammar above delegates to `json.value`.

**Inline vs. late delegation.** `$raw<grammar.rule>` and `cm.parse(grammar.rule, text, span)` (§8.5) perform the same delegation on _captured_ text — late, after an `until` has fixed the extent. The distinction matters when the embedded language's termination rule disagrees with its own lexer: HTML's raw-text elements end at the first matching end tag **lexically**, regardless of strings inside the embedded CSS or JavaScript, so `std.html` captures `<script>` bodies with `until` and parses them late (§8.8). When termination is grammar-local — JSON in conf, regex literals in JS — inline delegation is exact. The choice of strategy belongs to the macro author, like every other strategy in this system.

#### 8.3.9. Failure Semantics and Diagnostics

Matching failures are reported at the **furthest position reached**, with the set of alternatives expected there, contextualized by enclosing `label` blocks and rule names. `where` failures participate like any element. Committed-block failures (§8.3.5) are reported in preference to ordinary furthest failures — "this line belonged to this block" is a better diagnosis than whatever far-away alternative happened to be tried last. If a match later succeeds through another branch, all recorded failures are discarded:

```text
error[mega]: mods/hud/src/hud.cm:17:5
    constraint failed: close == name  ('div' ≠ 'p')
    element opened at mods/hud/src/hud.cm:15:5
    ┆ <p class="hud">
    ┆     <span>HP</span>
    ┆ </div>
    ┆  ^^^ while matching 'html.element' → branch 'normal' → 'close'
```

A mega invocation's pattern must consume the entire region; leftover content is reported the same way, along with any region-scan hint from §8.6.

#### 8.3.10. Pattern Grammar (Condensed)

```ebnf
pattern    → { term }
term       → literal | iliteral | class | fragment | ruleref | group
           | repetition | optional | choice | look
           | verbatim | rawregion | softregion | indentBlock
           | constraint | label | lineend | lineassert | eof
literal    → STRING ;  iliteral → "i" STRING
class      → "[" items "]" [ "as" BIND ]              // exactly one character
fragment   → [ "i" ] "$" IDENT [ "<" ( validator | ruleref | templateSpec ) ">" ] [ "as" BIND ]
ruleref    → qualified [ "with" "context" "{" { IDENT [ ":" cexpr ] } "}" ] [ "as" BIND ]
           | "recur"
repetition → "each" [ "+" ] [ "sep" pattern ] [ "trailing" ] [ bounds ]
             "{" pattern "}" [ "as" BIND ]
optional   → "optional" "{" pattern "}" [ "as" BIND ]        // ≡ each [0,1], atomic
choice     → "oneof" "{" { IDENT "=>" [ "(" pattern ")" | pattern ] } "}"
look       → ("peek" | "not") "{" pattern "}"
verbatim   → ("until" ( literal | "{" pattern "}" ) | "lineRest") [ "as" BIND ]
rawregion  → "raw" "{" pattern "}"
softregion → "soft" "{" pattern "}"
indentBlock→ "indent" ( "{" pattern "}" | "verbatim" [ "as" BIND ] )
constraint → "where" cexpr
label      → "label" STRING "{" pattern "}"
lineend    → "eol" ;  lineassert → "line" ;  eof → "eof"
annotation → ("#complete" "(" expr ")" | "#hover" "(" STRING ")" | "#token" "(" STRING ")")
```

Annotations may appear between any terms; they are inert (§8.1, §8.9).

### 8.4. Expansion Templates

The template is the target code with holes. Holes are typed by capture kind and syntactic position:

| Hole position                                    | ident / text / numeric capture | list capture     | tagged record     | code capture            | optional capture          |
| ------------------------------------------------ | ------------------------------ | ---------------- | ----------------- | ----------------------- | ------------------------- |
| name (function, field, param)                    | splices the identifier         | —                | —                 | parsed as an identifier | —                         |
| type                                             | —                              | —                | —                 | parsed as a type        | —                         |
| expression                                       | splices as a literal value     | array literal    | must be `match`ed | parsed as an expression | —                         |
| element lists (params, args, statements, fields) | single element                 | repeats elements | —                 | repeats elements        | single element if present |

An `optional { p } as x` capture holds `some(value)` or `none`; `present($x)` tests it in `where` conditions and template `[when]` guards, and splicing an absent optional is a template compile error.

**Name resolution in templates.** Inside `match ($item) { label => … }` arms and inside `each in xs { … }` bodies, a bare `$field` resolves to the current element's field; the qualified form (`$item.field`) is always available and means the same thing. A `match` must list every branch label of the tagged capture; exhaustiveness is checked.

Template constructs: `$cap` splices; `$"…{cap}…"` interpolates into strings; `[each in xs { … }]` repeats (with optional `where` filters); `[when cond { … } else { … }]` selects; `match ($cap) { label => … }` dispatches on `oneof` tags; `let` binds; `require(cond, "message")` emits a compile-time error anchored at a capture's span; `@fn(…)` invokes a compile-time function (§8.5).

Every generated AST node carries the span of the template element and capture that produced it, so **type errors in generated code point at the embedded-language source**. A complete example — a Python-flavored `def` generating a real Checkmate function:

```checkmate
// File: src/bridge.cm
grammar py {
    skip    [ ' ' ]
    comment ( "#" )

    rule def {
        "def" $word fname
        "(" soft { each sep "," { $word param optional { ":" $type ptype } } as params ")" }
        "->" $type ret ":"
        indent { each { recur } as body }
    }

    rule stmt {
        oneof {
            ifStmt => (
                "if" $expr cond ":"
                indent { each { recur } as body }
            )
            return => ( "return" optional { $expr value } eol )
            call   => ( $word callee "(" soft { each sep "," { $expr arg } as args ")" } eol )
        }
    }
}

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

expands to ordinary, fully type-checked Checkmate:

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

`$expr cond` is a live island whose boundary is the tail `":" indent { … }`, parse-integrated (§8.3.6) — so `if a ? b : c:` captures `a ? b : c`, and `if clamp(v, lo) > hi:` works because the inner commas fail the Checkmate parse and the scan continues. `$type ptype` is a live island type-checked after expansion; the template's `[when present($ptype) … else { infer $param }]` handles untyped parameters (a typed function is generated either way). `soft` around the parameter and argument lists matches Python's bracket rule: newlines are free inside `(`…`)`, significant outside. The nested `if` bodies dedent from column 12 to 8, closing the inner block cleanly and continuing the outer one (§8.3.5); `@py.emitBody` (§8.5) handles recursive statement codegen. Invocation positions: **declaration, statement, expression, and type** — macros can generate anything the language can declare.

### 8.5. Compile-Time Computation

Templates can call any pure Checkmate function with the `@` prefix. Unmarked calls are ordinary runtime code emitted into the output; `@`-marked calls execute during expansion, in the sandboxed bytecode interpreter (§5.1), under §5.5 limits. §5.5 fuel is an **operation count**, never wall-clock time — compilation is deterministic and byte-reproducible across platforms. Compile-time code may import only `self` modules and the core library; schema imports inside compile-time-evaluated code are compile errors, and there are no exceptions — validators included (§8.3.3).

Compile-time functions receive capture values (records, lists, texts, numbers, spans) and return values or `code` — a compile-time-only syntax-fragment type, constructed by the parsers (`$raw`, `$expr`, `$type`), by a builder API (`cm.code.call`, `cm.code.fn`, …), or by parsing text. **Parsing API:**

- `cm.parseExpr(text, span)` / `cm.parseStmts(text, span)` — Checkmate expressions and statements; the text may contain `mega(…) { … }` invocations, which enter the expansion queue of §8.6 like any other.
- `cm.parse(grammar.rule, text, span)` — delegate to any grammar rule (late delegation, §8.3.8).

The `span` argument threads provenance: every node parsed from the text carries it. Nodes built by `cm.code.*` inherit the span of the `@`-call's template element unless given one explicitly. Diagnostics for generated code therefore keep pointing at embedded-language source even through recursive generators — `@py.emitBody`, `@std.html.emitElement`, `@std.re.emitMatcher` are ordinary recursive Checkmate functions. `code` exists only during compilation: §5.2 is strict AOT, so this is metaprogramming, not dynamic code execution.

### 8.6. Invocation and Post-Expansion

**Region location.** `mega(name) { …region… }` — the parser resolves `name` in the macro namespace (imports precede use; unresolved names are parse errors), then locates the region by brace balancing under the **composed profile**: the comment, string, and island forms of the entry grammar _and of every grammar its entry pattern references_, transitively. At each position the scanner tries comment forms longest-first, then string forms longest-first. If a string form declares an **island** (e.g., `${` to `}`), the scanner recursively balances braces inside the island, allowing nested macro invocations to be discovered and expanded. A single-line string form that does not close on the same line is treated as ordinary text (an apostrophe in prose cannot swallow the file); forms declared `multiline` may span. Everything else counts braces.

Composition is what makes nesting Just Work: an HTML region containing `console.log("}")` (js strings composed in), `// it's fine` (js comments, matched before strings), `` `a ${b} c` `` (js multiline strings), or `console.log(\`val: ${ mega(json.value) { 1 } }\`)` (js island composed in, balancing the inner `{`) all balance correctly — no single profile could know all three, but the pattern's own reference graph does.

**Region normalization.** The region excludes one line terminator immediately after `{`, one immediately before `}`, and horizontal whitespace at the region's start and end. Line-oriented grammars therefore begin matching at the first content character — `mega(def) {⏎    def clamp(…` matches `def` directly, and `mega(re.compile) {⏎    ^[\w.…` does not silently absorb the indentation into the regex (std.re's skip set is empty). Flow-oriented grammars skip the trimmed whitespace anyway; the rule is uniform and harmless to them.

**No speculative extension.** If the pattern fails at the region's last character, the diagnostic reports the furthest failure and adds the scan hint where relevant: _an inner `}` invisible to every composed profile — a brace inside an embedded regex literal, say — may have closed the region early; the heredoc form is exact._ The scanner never guesses a larger extent: a wrong guess can silently absorb host code into the macro, and no syntactic signal distinguishes that case from a genuine truncation. Heredocs are the zero-approximation escape hatch:

```checkmate
mega(name) <<tag … tag
```

— the region extends verbatim to the first line whose content is exactly `tag`; only the edge trims of normalization apply.

A mega invocation's pattern must consume the entire region; leftover content is reported with the furthest-failure diagnostics.

**One AST, one queue.** Expansion is a worklist over a single AST representation. `mega()` invocation nodes enter that tree from exactly three places, and all three are processed identically:

1. literal template text;
2. text parsed at compile time — `$raw` captures, `$expr`/`$type`/`$block` islands, and `cm.parse*` results;
3. `code` values returned by `@`-functions.

After each pass, the expander sweeps the entire tree in fixed depth-first source order and enqueues every remaining invocation. The sweep repeats until none remain, bounded by an **expansion-tree depth cap of 64, counting all origins** — nesting depth, not pass count: sibling invocations at the same depth expand in the same pass, so breadth is unbounded and legitimate wide generation never hits the cap. Exceeding it reports the full expansion stack — each macro, span, and pass.

**Provenance never affects processing.** A node returned by an `@`-function is expanded in the same pass as a node spliced from a template or parsed from an island; origin is recorded only for diagnostics (§8.9). Islands are type-checked after the fixpoint together with all other generated code: there is exactly one name-resolution and type-checking pass over the final tree (§5), regardless of how each node arrived. Because `@`-functions are pure and the traversal order is fixed, the fixpoint is deterministic — which is what keeps §5.4 artifact hashes reproducible.

### 8.7. Guarantees

1. **Determinism.** Ordered choice, no ambiguity, stateless matching, pure `@`-functions, deterministic operation-count fuel, and a fixed traversal order in the expansion fixpoint: the same source and the same grammars produce byte-identical expansion on every platform.
2. **Complexity, honestly stated.** Packrat memoization bounds matching time by O(region × rules) **per environment**, where an environment is a (context bindings, enclosing block column, skip mode) tuple. Environments are few in practice — contexts are small and indentation is shallow — but neither factor is _structurally_ bounded: an accumulated context list grows with input depth, and distinct indentation columns grow with input width; in the worst case total time is O(region² × rules). Memo storage is O(environments × positions) and is the practical memory bound. `until`/`$raw` add a quadratic worst case (§8.3.6). All matching and compile-time evaluation run under §5.5 fuel, so pathological grammars terminate with a budget error rather than hanging. Catastrophic _regex-style_ backtracking is impossible by construction — at compile time; the runtime complexity of matchers _generated_ by macros (e.g. `std.re`'s backtracking path) is the macro author's responsibility, stated in that macro's documentation.
3. **Termination.** Fuel-metered compile-time evaluation; expansion depth capped across all origins (§8.6); left recursion — including **nullable-prefix cycles**, a rule reaching itself while consuming nothing through `optional`, empty iterations, `peek`/`not`, `where`, or `label` — statically rejected with a rewrite suggestion; as a backstop, a re-entrant rule invocation against an in-progress memo entry fails immediately. Speculative tails are cycle-cut (§8.3.6).
4. **Purity.** No host capabilities, no I/O, no clock, no ambient compiler state — enforced by the import checker, not convention, with no exceptions for validators or annotations.
5. **Span faithfulness.** From furthest-failure parse errors to final type errors, diagnostics point at the embedded language's source, in the user's file — including code built by `@`-functions (§8.5's span threading).
6. **Uniform provenance.** One expansion mechanism and one AST; how a node arrived never affects which passes process it (§8.6).

### 8.8. The Standard Grammar Library

Shipped under the `std` root alongside the §11 core. Deleting them and re-implementing them in user space yields identical behavior. Coverage is stated exactly; where a construct is out of scope, the grammar rejects it with a diagnostic rather than silently mis-parsing it.

| Grammar             | Coverage                                                                                                                                                                 | Key mechanisms exercised                                             |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------- |
| `std.json`          | RFC 8259, strict (leading zeros rejected)                                                                                                                                | flow skipper, `oneof`, recursion, `scan`, `sep`                      |
| `std.toml`          | TOML 1.0 incl. multi-line arrays and dotted keys                                                                                                                         | line mode + `soft`, explicit key charset, `require`                  |
| `std.yaml`          | YAML 1.2 core: block/flow (multi-line flow included), anchors, tags, multi-doc, block scalars, merge keys, plain multi-line scalars                                      | `indent`, `indent verbatim`, transparent comments, `peek`, `require` |
| `std.re`            | ECMAScript regex plus PCRE lookbehind, named groups, possessive/atomic groups, inline flags; POSIX classes; conditionals, subroutines, `\Q…\E` rejected with diagnostics | guarded classes, ordered prefix choice, `until`, `@` codegen         |
| `std.html`          | HTML5 syntax minus foreign content; raw-text termination per spec                                                                                                        | `i$tag`, whole-tail `until`, late parsing, delegation                |
| `std.css`           | selectors incl. nesting and `&`, `:has()`/`:is()`, at-rules, custom properties, `!important`                                                                             | `context`, `$tt`, `scan`, `string` forms                             |
| `std.js` / `std.ts` | full ES statement/expression surface incl. ASI and restricted productions; TS via `grammar ts extends js`                                                                | line mode, `soft`, `eol`-ASI, postfix repetition, delegation         |

**JSON** — the whole grammar, verbatim:

```checkmate
grammar json {
    skip [ ' ', '\t', '\r', '\n' ]

    rule value {
        oneof {
            null   => "null"
            bool   => oneof { t => "true", f => "false" }
            number => number
            string => $str text
            array  => ( "[" each sep "," { value } as items "]" )
            object => ( "{" each sep "," { member } as fields "}" )
        }
    }

    rule member {
        $str key ":" value
    }

    rule number {
        optional { "-" }
        oneof { zero => "0", pos => ( [1-9] as first scan [0-9] as rest ) }
        optional { "." scan [0-9] as frac }
        // ...
    }
}

mega value(json.value as v) {
    @toValue($v)
}
```

```checkmate
import std.json

infer config = mega(json.value) {
    {
        "host": "db.local",
        "ports": [5432, 6432],
        "retries": 3
    }
}
```

`@toValue` turns the capture tree into typed `code` constructing a `jsonValue` enum; a CMON-oriented variant splices directly into struct literals for §11.1 deserialization.

**TOML** — dotted keys, tables, and the line/soft split:

```checkmate
grammar toml {
    skip    [ ' ', '\t' ]                 // line-oriented: tables are lines
    comment ( "#" )

    rule document {
        each { oneof { table => tableHeader, kv => keyval } } as items
    }

    rule tableHeader {
        oneof {
            arrayTable => ( "[[" dottedKey path "]]" eol )
            table      => ( "[" dottedKey path "]" eol )
        }
    }

    rule keyval {
        dottedKey key "=" value eol        // trailing comments: eol's job
    }

    rule dottedKey {
        each sep "." {
            oneof { bare => scan [A-Za-z0-9_-] as part, quoted => $str part }
        } as parts
    }

    rule value {
        oneof {
            string   => string
            integer  => integer
            float    => floatV
            bool     => oneof { t => "true", f => "false" }
            datetime => datetime
            array    => ( "[" soft { each sep "," trailing { value } as items "]" } )
            inline   => ( "{" soft { each sep "," { dottedKey k "=" value v } as fields "}" } )
        }
    }

    // rule string (basic / literal / multiline), rule integer
    // (dec, 0x, 0o, 0b), rule floatV, rule datetime — elided
}

mega value(toml.document as doc) {
    require(@tablesConsistent($doc), "table redefined or reopened with a conflicting type")
    @toValue($doc)
}
```

Two details are load-bearing. Bare key segments are `scan [A-Za-z0-9_-]` — the exact TOML bare-key set, _without_ `.` — because `$tag` would maximal-munch the dots and collapse `owner.name` into one segment; quoted segments are the `oneof`'s second branch. And arrays/inline tables are `soft` regions: `[1,\n 2, 3,]` spans lines exactly as TOML 1.0 allows, while `keyval` stays line-terminated. Standalone comment lines are transparent (§8.2) and trailing comments are consumed by `eol`, so no comment rule appears in `document` or `keyval` — the discipline is in the machinery, not sprinkled through the grammar. Table reopening is _semantic_ validation — a `require` over the capture tree, per §8.3.4's decision table.

**YAML** — indentation, anchors, block scalars:

```checkmate
grammar yaml {
    skip    [ ' ' ]
    comment ( "#" )

    rule document {
        optional { "---" }
        oneof { blockDoc => blockNode, flowDoc => flowNode }
        optional { "..." }
    }

    rule node {
        oneof {
            anchor => ( "&" $word name node )
            alias  => ( "*" $word name )
            flow   => ( peek { line } flowNode )
            block  => ( peek { not { line } } blockNode )
        }
    }

    rule blockNode {
        oneof {
            seq => indent { each+ { "-" node } as items }
            map => indent { each+ { field } as fields }
        }
        // plain multi-line scalars, column-exact variants — elided
    }

    rule field {
        key key ":"
        oneof {
            blockScalar => (
                oneof { literal => "|", folded => ">" }
                optional { $tag header }
                indent verbatim as text
            )
            sameLine => ( peek { line } node value )
            block    => node value
            empty    => peek { not { line } }
        }
    }

    rule flowNode {
        oneof {
            seq    => ( "[" soft { each sep "," { node } as items "]" } )
            map    => ( "{" soft { each sep "," { key key ":" node } as fields "}" } )
            scalar => scalar
        }
    }

    // rule key, rule scalar (plain, quoted, multi-line continuations),
    // tags ("!!str" …), merge keys ("<<") — elided
}

mega value(yaml.document as doc) {
    require(@anchorsResolve($doc), "alias references an undefined anchor")
    @toValue($doc)
}
```

```checkmate
import std.yaml

infer settings = mega(yaml.value) {
    ---
    title: Checkmate
    limits:
        fuel: 1000000
        deadlineMs: 50
    tags: [embeddable, aot, arc]
    defaults: &def
        retries: 3
    production:
        <<: *def
        retries: 5
}
```

`peek { line }` / `peek { not { line } }` dispatch same-line values from block values; §8.3.5's strictly-deeper rule then separates a nested block from a following sibling. Flow collections are `soft`, so `[a,\n b]` — multi-line flow, valid YAML — parses. Comments at any column between entries are transparent lines (§8.3.5).

**Regular expressions** — the acid test for character-level power (~90 rules total; the heart):

```checkmate
grammar re {
    skip [ ]

    rule pattern {
        each sep "|" { alternative } as alts
    }

    rule alternative {
        each { quantified } as terms
    }

    rule quantified {
        atom
        optional {
            oneof {
                star  => ( "*"  quantSuffix )
                plus  => ( "+"  quantSuffix )
                opt   => ( "?"  quantSuffix )
                bound => ( "{" $int min optional { "," optional { $int max } } "}" quantSuffix )
            }
        }
    }

    rule quantSuffix {
        optional { oneof { lazy => "?", possessive => "+" } }
    }

    rule atom {
        oneof {
            anyChar  => "."
            anchor   => oneof { start => "^", end => "$" }
            class    => class
            escape   => escape
            group    => group
            literal  => [^.^$*+?(){}|/\[\]] as ch       // exactly one character
        }
    }

    rule class {
        "[" optional { "^" }
        each { classItem } as items
        "]"
    }

    rule classItem {
        oneof {
            posix   => ( "[:" $tag name ":]" )
            escape  => escape
            range   => ( any lo "-" peek { not { "]" } } any hi )
            single  => ( peek { not { "]" } } any as ch )
        }
    }

    rule escape {
        "\\"
        oneof {
            classEscape  => scan [dDwWsS] as kind
            anchorEscape => oneof { b => "b", B => "B", A => "A", z => "z", Z => "Z", G => "G" }
            ctrl         => oneof { n => "n", r => "r", t => "t", f => "f", "0" => "0" }
            backref      => oneof { num => scan [1-9] as index,
                                    named => ( "k" "<" $word name ">" ) }
            unicodeClass => ( "p" "{" $tag category "}" )
            char         => [^A-Za-z0-9] as ch           // escaped punctuation only
        }
    }

    rule group {
        "("
        oneof {
            lookBehindNeg => ( "?<!" pattern )
            lookBehind    => ( "?<=" pattern )
            named         => ( "?<" $word name ">" pattern )
            lookAheadNeg  => ( "?!"  pattern )
            lookAhead     => ( "?="  pattern )
            atomic        => ( "?>"  pattern )
            inlineFlags   => ( "?" scan [-imsxu]* as flags ":" pattern )
            nonCapturing  => ( "?:"  pattern )
            capturing     => pattern
        }
        ")"
    }
}

mega compile(re.pattern as p) {
    @emitMatcher($p)
}
```

Three guards make the shipped example parse correctly. Class items refuse `]` — `range` before the `]`-guard, `single` behind one — so `[\w.+-]` is four items (`\w`, `.`, `+`, `-`) with the class closing at its own bracket, and a class never swallows past its closer. Literal atoms are single characters (a class, not a `scan` run), so `ab*` is `a` then `b*` — quantifiers attach to the last character, as in every regex engine. And the escape fallback is escaped _punctuation_ only: `\Q`, `\g`, or any unsupported alphanumeric escape is a parse error pointing at its span — PCRE constructs outside the declared scope (conditionals, subroutines, `\Q…\E`) fail loudly instead of silently becoming literals. `\A`, `\z`, `\Z`, `\G`, `\b`, `\B` are anchor escapes, not characters. Prefix-colliding group forms are listed longest-first. `@emitMatcher` inspects the capture tree: backreference- and lookaround-free patterns compile to a linear-time Thompson NFA; the rest to a memoized backtracking matcher. That dispatch is ordinary compile-time Checkmate — the macro author owns the strategy.

```checkmate
import std.re

infer isEmail = mega(re.compile) {
    ^[\w.+-]+@[\w-]+(\.[\w-]+)+$
}
```

**HTML with embedded CSS and JavaScript** — the composition showcase:

```checkmate
// File: std/html.cm  (excerpt)
import std.css
import std.js

grammar html {
    skip    [ ' ', '\t', '\r', '\n' ]
    comment ( "<!--" until "-->" )
    string  ( '"' )  string ( "'" )

    rule document {
        optional { "<!" i"doctype" $tag name ">" }
        each { content } as children
    }

    rule content {
        oneof {
            element => element
            text    => until { "<" [a-zA-Z!/] } as text
        }
    }

    rule element {
        "<" i$tag name
        each { attribute } as attrs
        oneof {
            scriptEl => (
                where name == "script" ">"
                until { "</" i$tag close where close == name } as body
                "</" i$tag close ">"
            )
            styleEl => (
                where name == "style" ">"
                until { "</" i$tag close where close == name } as body
                "</" i$tag close ">"
            )
            selfClose => ( where @isVoid(name) "/>" )
            voidEl    => ( where @isVoid(name) ">" )
            rawEl     => (
                where @isRawText(name) ">"
                until { "</" i$tag close where close == name } as body
                "</" i$tag close ">"
            )
            normal => (
                ">"
                each { content } as children
                "</" i$tag close
                where close == name
                ">"
            )
        }
    }

    rule attribute {
        i$tag name
        optional {
            "="
            oneof {
                quoted => $str value
                single => ( "'" until "'" as value "'" )
                bare   => scan [^ \t\r\n>] as value
            }
        }
    }
}

mega fragment(html.document as doc) {
    engine.ui.mount([each in doc.children {
        match ($item) {
            element => @emitElement($item)
            text    => engine.ui.text(@decodeEntities($item.text))
        }
    }])
}
```

```checkmate
import std.html

mega(html.fragment) {
    <!doctype html>
    <html>
        <style>
            body { margin: 0; font: 14px system-ui }
            .hud > .bar { width: 100% }
            .hud > .bar:hover { opacity: 0.8 }
        </style>
        <body>
            <div class="hud">
                <span class="bar">HP</span>
                <p>1 < 2</p>
            </div>
            <script>
                console.log("hud mounted")
            </script>
        </body>
    </html>
}
```

**Conformance notes — every choice here is a stress-test scar.** Tag and attribute names are `i$tag` captures, folded to lowercase: `<BR>` is void, `</DIV>` closes `<div>`, `where name == "script"` sees `<SCRIPT>` — HTML is case-insensitive and so is the grammar. Text nodes stop only at tag starts — `until { "<" [a-zA-Z!/] }` — so `1 < 2` is text, not a parse error. Raw-text and script/style bodies are captured _lexically_: `until { "</" i$tag close where close == name }` stops at the first end tag whose folded name matches, ignoring strings in the embedded language, because that is HTML's actual rule — `<script>document.write("</script>")</script>` ends the script inside the string, and `@emitElement` then reports the broken JavaScript at the string's span (write `<\/script>`). The same whole-tail stop condition means `</b` inside a `<textarea>` is content, not a terminator. The bodies are parsed _late_ — `@emitElement` calls `cm.parse(js.program, $item.body, …)` and `cm.parse(css.sheet, $item.body, …)` — rather than by inline delegation, because inline delegation is string-aware and would diverge from the spec (§8.3.8). `style="…"` attribute values are likewise re-parsed by `css.decls` at expansion time. Non-void self-closing tags (`<div/>`) are errors — std.html is a validator. Foreign content (SVG/MathML), where self-closing is significant, is out of scope: `<svg><path/></svg>` is rejected with the furthest failure at the `/>`, not mis-parsed.

**JavaScript and TypeScript** — the excerpt that matters:

```checkmate
// File: std/js.cm  (excerpt)
import std.re

grammar js {
    skip    [ ' ', '\t' ]                  // line-oriented: ASI is defined over lines
    comment ( "//" )                       // to end of line
    comment ( "/*" until "*/" )
    string  ( '"' )  string ( "'" )  string ( '`' multiline island ( "${" "}" ) )

    rule program {
        each { statement } as stmts
    }

    rule statement {
        oneof {
            block  => ( "{" optional { eol } each { statement } as body "}" optional { eol } )
            if     => ( "if" soft { "(" expression cond ")" } optional { eol } statement then
                        optional { "else" optional { eol } statement otherwise } )
            while  => ( "while" soft { "(" expression cond ")" } optional { eol } statement body )
            ret    => ( "return" optional { expression value } semi )
            decl   => ( oneof { let => "let", const => "const" } $word name
                        optional { soft { "=" expression init } } semi )
            expr   => ( expression value semi )
            // for, class, try, switch, throw — elided; same shape
        }
    }

    // ASI: a statement ends at ";" (consuming any following line ends) or at
    // end of line.  `ret` is the restricted production: its value is NOT
    // wrapped in soft, so `return` followed by a line end returns undefined
    // and the next line starts a new statement — JavaScript's actual rule.
    rule semi {
        oneof { explicit => ( ";" optional { eol } ), inserted => eol }
    }

    rule expression { assignment }        // full precedence ladder elided;
                                          // every binary level is
                                          //   operand each { soft { op operand } }

    rule unary {
        oneof {
            neg     => ( "-" soft { unary } )
            not     => ( "!" soft { unary } )
            postfix => postfix
        }
    }

    // no left recursion: member/call chains are a postfix repetition
    rule postfix {
        primary first
        each {
            soft {
                oneof {
                    call   => ( "(" soft { each sep "," { expression arg } as args ")" } )
                    member => ( "." $word field )
                    index  => ( "[" soft { expression idx } "]" )
                }
            }
        } as ops
    }

    rule primary {
        oneof {
            number   => $float literal        // hex/octal/binary forms elided
            string   => $str literal
            ident    => $word name
            regex    => re.literal            // grammar reuse
            paren    => ( "(" soft { expression inner ")" } )
            // object & array literals, template literals (a parameterized
            // $template<"${" "}" expression>), arrow functions, new, await — elided
        }
    }
}

mega run(js.program as program) {
    engine.javascript.Execute(@emitSource($program))
}
```

```checkmate
import std.js

mega(js.run) {
    const greeting = `Hello, ${ mega(json.value) { "world" } }!`
    console.log(greeting)
}
```

Because JS template literals are declared with an `island ( "${" "}" )` in the grammar profile (§8.2), the region scanner pierces the string literal, discovers the nested `mega(json.value)`, and expands it before handing the text to the JS parser. The generated JS becomes ``const greeting = `Hello, world!`;``. ASI works correctly: `return` followed by a newline matches `semi`'s `inserted => eol` branch, terminating the statement exactly where JavaScript specifies.

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
        // Changes lost here because nothing is returned
        // TODO: `cme` has to error here
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

## Appendix A — Operators and Expressions

This appendix is normative. It defines the complete inventory of binary and unary
operators, their precedence and associativity, operand typing, evaluation semantics,
the compound assignment statement forms, and the interaction between operators and
significant newlines. Constructs deliberately left out of the language are listed in
§A.9.

### A.1. Operator Table

Precedence is listed from loosest to tightest. All binary operators are
left-associative, except the comparison operators, which are non-associative (§A.3).

| Level | Operators                   | Category       | Associativity   |
| ----- | --------------------------- | -------------- | --------------- |
| 1     | `\|\|`                      | logical or     | left            |
| 2     | `&&`                        | logical and    | left            |
| 3     | `==` `!=` `<` `<=` `>` `>=` | comparison     | non-associative |
| 4     | `+` `-`                     | additive       | left            |
| 5     | `*` `/` `%`                 | multiplicative | left            |
| 6     | `-` `!` (prefix)            | unary          | prefix          |

Parenthesized expressions override precedence and bind tighter than every operator
in this table.

Note: because §A.3 requires parentheses when `&&` and `||` are mixed, the relative
precedence of levels 1 and 2 is deliberately unobservable.

### A.2. Grammar

```ebnf
expr           → logic_or
logic_or       → logic_and { "||" logic_and }
logic_and      → comparison { "&&" comparison }
comparison     → additive [ cmp_op additive ]    ; at most one — see §A.3
cmp_op         → "==" | "!=" | "<" | "<=" | ">" | ">="
additive       → multiplicative { ("+" | "-") multiplicative }
multiplicative → unary { ("*" | "/" | "%") unary }
unary          → ( "-" | "!" ) unary | primary
primary        → literal | identifier | "(" expr ")"
```

`{ }` means zero or more repetitions; `[ ]` means an optional part. The grammar alone
permits mixing `&&` with `||`; the parenthesization rules in §A.3 reject it at compile
time.

Unary operators nest freely: `!!flag` and `-(-x)` are valid, and since `--` is not a
token, `--x` is identical to `-(-x)`.

Worked examples of tree shape:

```text
1 + 2 * 3          10 - 4 - 3        -x * y
     +                  -              *
    / \                / \            / \
   1   *              -   3        (-x)  y
      / \            / \
     2   3         10   4
```

`1 + 2 * 3` is `7`; `10 - 4 - 3` is `3` (left-associative); `-x * y` is `(-x) * y`.

### A.3. Mandatory Parenthesization

Two constructs are compile-time errors unless explicitly parenthesized. Both rules
exist because the unparenthesized form is a well-known source of silent mistakes.

**Rule 1 — Mixing logical operators.** An expression containing both `&&` and `||`
must parenthesize the mixing:

```checkmate
a || b && c       // error: mixed && and ||
a || (b && c)     // ok
(a || b) && c     // ok
a && b && c       // ok: same operator, left-associative
a || b || c       // ok: same operator, left-associative
```

**Rule 2 — Comparisons are non-associative.** An operand of a comparison operator may
not itself be a comparison expression unless parenthesized:

```checkmate
a < b < c         // error: chained comparison; write a < b && b < c
a == b < c        // error; write a == (b < c)
(a < b) == c      // ok (c must be bool)
```

### A.4. Operand Typing

Operators are strict about operand types. There are no implicit coercions between
types (§2.4), including through operators.

| Operators         | Operand types                                                          | Result      |
| ----------------- | ---------------------------------------------------------------------- | ----------- |
| `+`               | both `int`, or both `float`                                            | as operands |
| `+`               | at least one `str`; other side `str`, `int`, `float`, or `bool` (§A.6) | `str`       |
| `-` `*`           | both `int`, or both `float`                                            | as operands |
| `/`               | both `int`, or both `float`                                            | as operands |
| `%`               | both `int`                                                             | `int`       |
| `<` `<=` `>` `>=` | both `int`, or both `float`                                            | `bool`      |
| `==` `!=`         | both operands of the same type                                         | `bool`      |
| `&&` `\|\|`       | both `bool`                                                            | `bool`      |
| `-` (unary)       | `int` or `float`                                                       | as operand  |
| `!` (unary)       | `bool`                                                                 | `bool`      |

Additional rules:

- `+` is numeric addition only when both operands are numeric. If either operand is
  `str`, `+` is string concatenation (§A.6). The meaning of each `+` node is
  determined entirely by its operand types — never by context.
- No operator other than `+` accepts a `str` operand. In particular `"ab" * 3` is a
  type error; there is no string repetition.
- `==` and `!=` are strict same-type, value equality: `str` compares by content,
  `float` follows IEEE 754 (so `NaN == NaN` is false), `bool` and `int` compare by
  value. Cross-type equality is never permitted: `1 == "1"` is a type error, not
  `true`. Equality for structs and enums will be defined structurally when those
  types are introduced; the same-type rule will not change.
- The stringification described in §A.6 applies only within concatenation. It is not
  a general coercion, and a `str` is never converted to a numeric type.

### A.5. Evaluation Semantics

- **Short-circuiting.** `lhs && rhs` evaluates `rhs` only when `lhs` is `true`;
  `lhs || rhs` evaluates `rhs` only when `lhs` is `false`.
- **Integer division.** `int / int` truncates toward zero and yields an `int`:
  `7 / 2` is `3`, `-7 / 2` is `-3`. `a % b` is the remainder of that truncated
  division, with the sign of `a`: `-7 % 2` is `-1`, `7 % -2` is `1`.
- **Float division.** `float / float` is ordinary IEEE 754 division.
- **Division and remainder by zero** terminate the invocation at runtime, consistent
  with the overflow policy of §2.4.
- **Overflow** follows §2.4: checked, terminating the invocation. Behavior at numeric
  literal boundaries (e.g., the most negative `int` value written as a negated
  literal) is unspecified in this version and will be pinned down together with the
  overflow-checking implementation.

### A.6. String Concatenation

When either operand of `+` is a `str`, the other operand is converted to its
canonical string form and concatenated:

- `int` — decimal digits, prefixed by `-` when negative
- `bool` — `true` or `false`
- `float` — the shortest decimal representation that round-trips to the same value
- `str` — used as-is

```checkmate
"HP: " + 100      // "HP: 100"
"ok: " + true     // "ok: true"
1.5 + "x"         // "1.5x"
"a" + 1 + 2       // "a12"   — parsed as ("a" + 1) + 2
1 + 2 + "a"       // "3a"    — parsed as (1 + 2) + "a"
```

The two final examples are the left-associativity consequence: the mixed forms are
deterministic, but mixing numeric and string operands across a chain is discouraged
style. There is no conversion in the other direction: a `str` operand never becomes a
number.

### A.7. Compound Assignment

The compound assignment operators are `+=`, `-=`, `*=`, `/=`, and `%=`. Each is a
**statement**, exactly equivalent to expanding the operator:

```checkmate
x += 10           // identical to: x = x + 10
s += "!"          // identical to: s = s + "!"
s += 100          // identical to: s = s + 100  → uses §A.6 stringification
```

- Compound assignment yields no value. It cannot be chained or embedded in an
  expression: `x += y += 1` and `a = (b += 1)` are compile-time errors.
- Plain `=` remains a statement as well (§2.10); `a = b = c` is a compile-time error.
- The assignment target is evaluated exactly once. (Significant once indexable
  targets such as arrays are introduced.)

### A.8. Operators and Newlines

Statement delimiting follows the established newline rule: a line break is
significant after a token that can end a statement, and insignificant otherwise.
Since no operator can end a statement, an expression continues across a line break
when the line ends with a binary operator or an open parenthesis:

```checkmate
int total = base +
    bonus          // ok: trailing operator continues the expression
```

A line that _begins_ with a binary operator is a compile-time error:

```checkmate
int total = base
    + bonus        // error: leading binary operator
```

After a trailing operator, the continuation line may begin with a unary operator;
the leading `-` below is unary, applied to `b`, not a statement-starting binary
operator:

```checkmate
int d = a +
    -b             // ok: a + (-b)
```

As illustrated in §2.6 and §2.12, newlines inside parentheses are insignificant, so
bracketed expressions wrap freely regardless of operator position.

### A.9. Deliberately Absent

The following constructs do not exist in this version of Checkmate:

- **Bitwise operators** (`&` `|` `^` `<<` `>>` `~`) — reserved for a future appendix.
- **Ternary conditional** (`?:`) — use `if`/`else`.
- **Exponentiation** — expected to arrive as a host math capability, not an operator.
- **Increment/decrement** (`++` `--`) — value semantics make them pointless; write
  `x += 1`.
- **Assignment as an expression** — assignment and compound assignment yield no
  value and cannot chain (§A.7).
- **Cross-type arithmetic, comparison, or equality** — never permitted (§A.4).
