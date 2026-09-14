# Embedding in C

Checkmate exposes a stable C ABI for C and C++ hosts
(`crates/cme-ffi/include/cme.h`, built as `libcme_ffi.a` / `.so`). This
guide walks the full lifecycle: build and link, load programs, create
contexts, invoke through futures, move values across the boundary,
handle errors, register schemas, and provide capabilities.

## Build and link

```sh
cd checkmate
cargo build --release -p cme-ffi
# produces target/release/libcme_ffi.a (static) and libcme_ffi.so (dynamic)
```

Then in your host:

```sh
cc -std=c11 -I checkmate/crates/cme-ffi/include -o myhost main.c \
   checkmate/target/release/libcme_ffi.a -lm -lpthread -ldl
```

Include the single header:

```c
#include "cme.h"
```

A working consumer lives at
[`apps/c_host`](https://github.com/cmengine/checkmate/tree/mom/apps/c_host)
(its `Makefile` builds standalone against the static library, and the
same source runs inside `cargo test` via cme-ffi's build script).

## The ownership model (read this first)

`cme.h` documents the contract; here is the operational summary:

- Opaque handles: `cm_engine_t`, `cm_program_t`, `cm_context_t`,
  `cm_future_t`, `cm_value_t`. Every handle you receive is **owned**;
  release it with its destroy call exactly once. Every `*_destroy` /
  `*_free` accepts `NULL` and does nothing.
- **Arguments to `cm_invoke` are borrowed**: the call clones them into
  the script; you keep ownership of every `cm_value_t*` in the argument
  array.
- Borrowed views (child `cm_value_t*` accessors, `const char*` names)
  stay valid as long as the **owning** handle lives and is not mutated;
  never free them.
- Heap strings returned by the library (`cm_value_to_string`, string
  accessors) are **owned copies** — free with `cm_string_free`.
- `cm_error_t` reports are owned-strings-in-struct; free the whole
  report with `cm_error_free`.

### Lifetimes

- A `cm_program_t` must **outlive** every `cm_context_t` created from
  it — destroy contexts first.
- A `cm_future_t` must be polled/destroyed before its context is
  destroyed.
- Strings borrowed from a program (entry/interface names) live as long
  as the program.

## Lifecycle: engine → program → context → invoke

```c
#include "cme.h"
#include <stdio.h>

int main(void) {
    cm_engine_t* engine = cm_engine_new();

    cm_error_t err = CM_ERROR_INIT;
    const char* src =
        "int add(int a, int b) {\n"
        "    return a + b\n"
        "}\n";

    cm_program_t* program = cm_engine_load_source(engine, src, &err);
    if (program == NULL) {
        fprintf(stderr, "compile: %s\n", err.message);
        cm_error_free(&err);
        cm_engine_destroy(engine);
        return 1;
    }
    // err.kind == CM_ERROR_NONE, empty strings: success always fills it.

    cm_limits_t limits = CM_LIMITS_INIT;   // 0 = unset/default everywhere
    limits.fuel = 1000000;
    limits.deadline_ms = 50;
    limits.max_call_depth = 64;

    cm_context_t* ctx = cm_engine_create_context(engine, program, &limits);

    cm_value_t* a = cm_value_int(40);
    cm_value_t* b = cm_value_int(2);
    cm_value_t* args[2] = { a, b };

    cm_future_t* future = cm_invoke(ctx, NULL, "add", args, 2);
    // target==NULL ⇒ top-level function "add".

    cm_value_t* result = NULL;
    cm_poll_result_t poll = cm_future_poll(future, &result);
    if (poll == CM_READY && result != NULL) {
        int64_t n = 0;
        cm_value_as_int(result, &n);
        printf("add(40, 2) = %lld\n", (long long)n);   // 42
        cm_value_destroy(result);
    } else if (poll == CM_ERROR) {
        cm_error_t run_err = cm_future_get_error(future);
        fprintf(stderr, "run: %s\n", run_err.message);
        cm_error_free(&run_err);
    }

    cm_future_destroy(future);
    cm_value_destroy(a);        // borrowed args: still yours to free
    cm_value_destroy(b);
    cm_context_destroy(ctx);    // contexts before programs
    cm_program_destroy(program);
    cm_engine_destroy(engine);
    return 0;
}
```

## Loading programs

```c
cm_program_t* cm_engine_load_source(cm_engine_t*, const char* source, cm_error_t* out);
cm_program_t* cm_engine_load_file  (cm_engine_t*, const char* path,   cm_error_t* out);
cm_program_t* cm_engine_load_mod   (cm_engine_t*, const char* root,   cm_error_t* out);
```

- All three apply the full gate (megaprogram expansion, parse,
  standalone-import check, type check). **`NULL` return ⇒ a diagnostic
  exists**; `out_error` carries kind `CM_ERROR_COMPILE` (or
  `CM_ERROR_IO` for read failures) with rendered `path:line:column:`
  messages.
- `load_mod` compiles a whole §10 mod tree (`root` = the mod directory
  or its `mod.toml`); diagnostics re-anchor to the owning module and
  `out_error->file` names it — hosts never see virtual-text
  coordinates.

### Entry-point metadata

```c
size_t      entries = cm_program_entry_count(program);
const char* name0   = cm_program_entry_name(program, 0);       // borrowed

size_t      targets = cm_program_interface_count(program);
const char* target0 = cm_program_interface_name(program, 0);   // "engine.gamemode"

int is_mod = cm_program_is_mod(program);
```

## Contexts and limits

```c
cm_context_t* cm_engine_create_context(cm_engine_t*, const cm_program_t*, const cm_limits_t*);
void          cm_context_destroy(cm_context_t*);
```

`cm_limits_t` — zero means "unset" for every field:

| Field | Meaning | Zero |
| --- | --- | --- |
| `fuel` | deterministic operation budget (§5.5) | unmetered |
| `deadline_ms` | wall-clock budget, checked at safepoints | none |
| `max_call_depth` | max nested call frames | engine default (1024) |

Contexts are cheap; each invocation gets a **fresh** fuel cell and
deadline, so one context can serve many invocations — including from
multiple threads. A host running scripts that legitimately recurse near
the default must provide adequate native stack or lower
`max_call_depth` — the depth guard, not the native stack, is what must
stop runaway recursion.

## Invocation and futures

```c
cm_future_t* cm_invoke(cm_context_t* ctx,
                       const char* target,     // NULL/"" ⇒ top-level function
                       const char* member,
                       cm_value_t* const* args,
                       size_t argc);
```

- `target == NULL` invokes the top-level function `member`.
- `target == "engine.gamemode"` invokes the §10.4 impl member
  `member` of that target path — the whitepaper shape:
  `cm_invoke(ctx, "engine.gamemode", "OnTick", NULL, 0)`.
- **Borrowed arguments**: the call clones `args` into the script; you
  keep ownership. Pass `NULL` with `argc == 0` for no arguments.
- Returns `NULL` **only** for host misuse (NULL context, NULL member,
  `args == NULL` with `argc > 0`, NULL element inside `args`). Every
  real failure — unknown entry, script error, budget — rides the
  returned future.

### The poll loop

```c
typedef enum cm_poll_result { CM_PENDING, CM_READY, CM_ERROR } cm_poll_result_t;

cm_poll_result_t cm_future_poll(cm_future_t*, cm_value_t** out_value);
cm_value_t*      cm_future_take_value(cm_future_t*);
cm_error_t       cm_future_get_error(cm_future_t*);
void             cm_future_destroy(cm_future_t*);
```

Over the current synchronous interpreter, **the first poll returns
`CM_READY` or `CM_ERROR`**; `CM_PENDING` exists for the future
continuation-splitting VM (§4/§5.2) — the polling loop you write today
is the loop you keep when suspension lands. Polling is idempotent:
`READY`/`ERROR` stay stable across repeated polls.

- `cm_future_poll(f, &out)`: when READY, `out` receives a **newly
  owned** handle (NULL if the value was already taken). Destroy it.
- `cm_future_take_value(f)`: moves the result out (same ownership).
- `cm_future_get_error(f)`: a **fresh owned copy** of the error report
  every call — free it with `cm_error_free`. Always safe to call
  (kind `CM_ERROR_NONE` when not in error).

## Values

### Kinds

```c
typedef enum cm_value_kind {
    CM_VALUE_INVALID, CM_VALUE_VOID, CM_VALUE_INT, CM_VALUE_FLOAT,
    CM_VALUE_BOOL, CM_VALUE_STR, CM_VALUE_STRUCT, CM_VALUE_ENUM,
    CM_VALUE_ARRAY, CM_VALUE_MAP
} cm_value_kind_t;
```

### Constructors (each returns an OWNED handle; NULL on invalid input)

```c
cm_value_t* cm_value_void(void);
cm_value_t* cm_value_int(int64_t);
cm_value_t* cm_value_float(double);
cm_value_t* cm_value_bool(int);              // non-zero is true
cm_value_t* cm_value_str(const char*);       // NUL-terminated, valid UTF-8
cm_value_t* cm_value_str_len(const char*, size_t);
cm_value_t* cm_value_struct(const char* type_name);
cm_value_t* cm_value_enum(const char* type_name, const char* variant);
cm_value_t* cm_value_array(void);
cm_value_t* cm_value_map(void);
```

### Builders (children are MOVED IN — do not destroy them afterwards)

```c
cm_status_t cm_struct_set_field(cm_value_t* strukt, const char* name, cm_value_t* field);
cm_status_t cm_enum_push(cm_value_t* enum_value, cm_value_t* payload);
cm_status_t cm_array_push(cm_value_t* array, cm_value_t* item);
cm_status_t cm_map_set(cm_value_t* map, cm_value_t* key, cm_value_t* value);
```

On failure the child is left untouched and still owned by the caller.

Building the script value `Item(id: 1, sku: "sword", price: 25)`:

```c
cm_value_t* item = cm_value_struct("Item");
cm_struct_set_field(item, "id",    cm_value_int(1));
cm_struct_set_field(item, "sku",   cm_value_str("sword"));
cm_struct_set_field(item, "price", cm_value_int(25));
```

### Accessors (scalars write `out` and return `CM_OK`; children are borrowed)

```c
cm_value_kind_t cm_value_kind(const cm_value_t*);
cm_status_t cm_value_as_int(const cm_value_t*, int64_t* out);
cm_status_t cm_value_as_float(const cm_value_t*, double* out);
cm_status_t cm_value_as_bool(const cm_value_t*, int* out);
cm_status_t cm_value_as_str(const cm_value_t*, char** out, size_t* out_length); // OWNED copy

cm_status_t cm_value_struct_name(const cm_value_t*, char** out);   // OWNED
cm_status_t cm_value_enum_type(const cm_value_t*, char** out);     // OWNED
cm_status_t cm_value_enum_variant(const cm_value_t*, char** out);  // OWNED

size_t               cm_value_len(const cm_value_t*);          // str bytes / array elems / map entries / struct fields / enum payloads
const cm_value_t*    cm_value_array_get(const cm_value_t*, size_t index);
const cm_value_t*    cm_value_map_key(const cm_value_t*, size_t index);
const cm_value_t*    cm_value_map_value(const cm_value_t*, size_t index);
const cm_value_t*    cm_value_struct_field(const cm_value_t*, const char* name);
const cm_value_t*    cm_value_enum_payload(const cm_value_t*, size_t index);

char* cm_value_to_string(const cm_value_t*);   // canonical CMON display; OWNED
void  cm_value_destroy(cm_value_t*);
```

Kind mismatches return `CM_ERR_KIND` without touching `out`; missing
fields return `CM_ERR_MISSING`; bad indices `CM_ERR_INDEX`. Iterate a
map by index pairs:

```c
for (size_t i = 0; i < cm_value_len(map); i++) {
    const cm_value_t* k = cm_value_map_key(map, i);
    const cm_value_t* v = cm_value_map_value(map, i);
    /* borrowed views: valid while `map` lives */
}
```

## Errors

```c
typedef enum cm_error_kind {
    CM_ERROR_NONE, CM_ERROR_COMPILE, CM_ERROR_RUNTIME, CM_ERROR_LIMIT,
    CM_ERROR_UNKNOWN_ENTRY, CM_ERROR_INVALID_ARG, CM_ERROR_IO
} cm_error_kind_t;

typedef struct cm_error_t {
    cm_error_kind_t kind;
    char* message;   // owned
    char* file;      // owning module for mod builds; "" otherwise
    int32_t line;    // 1-based; 0 when there is no position
    int32_t column;  // 1-based CHARACTER column; 0 when none
} cm_error_t;
```

- **Fill convention**: functions taking `cm_error_t* out` **always**
  write it — success fills kind `CM_ERROR_NONE` with empty strings.
  Pass a freshly zeroed report or `CM_ERROR_INIT`.
- Free with `cm_error_free(&err)`; it zeroes the struct, so
  re-filling/re-using is safe.
- `CM_ERROR_LIMIT` groups fuel/deadline/call-depth failures;
  `CM_ERROR_UNKNOWN_ENTRY` marks a missing entry point; a capability
  re-entering its context (§5.7) reports `CM_ERROR_INVALID_ARG` with a
  message naming §5.7.

## Schemas

```c
cm_schema_t* cm_schema_parse(const char* text, cm_error_t* out);
cm_schema_t* cm_schema_parse_file(const char* path, cm_error_t* out);
void         cm_schema_destroy(cm_schema_t*);
cm_status_t  cm_engine_register_schema(cm_engine_t*, const cm_schema_t*);
```

- Parse (NULL ⇒ defects; `out_error` renders them), then register.
  Registration **re-validates the whole set** — duplicate namespaces,
  cross-namespace type collisions, unresolved `requires` edges fail with
  `CM_ERROR_INVALID_ARG` and leave the engine unchanged.
- The schema handle stays owned by the **caller** and may be destroyed
  after registration (the engine keeps a copy).
- After registration, every later load is schema-gated: imports,
  capability calls, `impl` completeness, `since` version hiding,
  boundary capitalization — see [The Schema System](../schema/overview.md).

## Capability providers

```c
typedef cm_value_t* (*cm_capability_fn)(void* user,
                                        cm_value_t* const* args,
                                        size_t argc,
                                        cm_error_t* out_error);

typedef struct cm_capability_member {
    const char* name;        // schema member name ("LoadTexture")
    cm_capability_fn fn;
} cm_capability_member_t;

cm_status_t cm_engine_register_capability(cm_engine_t* engine,
                                          const char* path,   // "namespace.capability"
                                          const cm_capability_member_t* members,
                                          size_t count,
                                          void* user);
```

Provider contract:

- `args` are **borrowed** (owned by the engine; valid for the call
  only), `argc` their count, positional in schema declaration order.
- Return a **newly owned** `cm_value_t*` (the member's declared return
  type; `cm_value_void()` for void members).
- Or return `NULL` with `out_error` filled to fail the invocation
  (anchored at the script's call site).
- `user` is your opaque pointer; it must stay valid for the engine's
  lifetime. Provider calls may arrive from any thread that invokes into
  the engine — the **same** provider may run concurrently across
  contexts; guard shared state as you would in any threaded host.
- **§5.7 reentrancy**: a provider dispatched from a running invocation
  must not call `cm_invoke` on the **same context**; the nested call
  yields an ERROR future (`CM_ERROR_INVALID_ARG`, message naming §5.7)
  and the original invocation is unaffected.

The provider-presence check runs at **load** time: a program calling a
capability with no registered provider fails to load, so wiring
mistakes are deterministic before any invocation.

## Strings and UTF-8

- Strings **handed to C are owned copies** freed with
  `cm_string_free`. Rust `String` is not NUL-terminated, so borrowing
  `as_ptr()` would read past the end from C — the API caught this bug
  by construction; never try to borrow script text.
- Embedded NUL bytes in script text render as **U+FFFD** in C strings
  (C strings cannot carry them); `out_length` reports the *original*
  byte length.
- Strings **passed to the library** (`cm_value_str`,
  `cm_schema_parse`, paths) must be valid UTF-8; invalid input is
  rejected (`NULL` return / status), never mangled.

## Threads

- Engines and programs are immutable: **share them across threads**.
- A context may be used from multiple threads — every invocation gets
  its own fuel cell and deadline (§5.5) — but **one future or one value
  handle is not synchronized**: confine each to a single thread at a
  time.
- §5.7: concurrent invocation across *different* contexts is always
  legal; same-context re-entry through a capability is not.

## Generated schema headers

For compile-time-verified bindings, generate a header from your schema
and consume it:

```sh
cme codegen-c schemas/engine.cm > engine_schema_gen.h
```

The header gives you, per namespace:

- function-pointer typedefs per capability member + a **vtable**,
- a `CME_<NS>_<CAP>_REGISTER` registration macro whose
  `_Static_assert(_Generic(...))` lines verify every host
  implementation's signature **at compile time** — a missing member
  fails the preprocessor, a wrong signature fails the assert,
- exact-arity **interface invocation helpers** for host → script calls,
- struct/enum **pack/unpack helpers** over `cm_value_t` (by-name
  fields; `_pack` takes the host value BY ADDRESS so nested schema
  types compose recursively; `str` fields unpack to owned `char*`
  copies freed with `cm_string_free`; enums are tagged unions with
  per-variant payload structs dispatched through `cm_value_enum_variant`).

One header per namespace; several coexist in one translation unit. The
header is multi-include-safe and byte-deterministic. See
[Schema-Driven Embedding](schema-embedding.md) for the workflow; the
[`apps/c_host`](https://github.com/cmengine/checkmate/tree/mom/apps/c_host)
example consumes a generated header for real (246 self-checks).

## A complete capability round-trip

```c
static cm_value_t* fetch_item(void* user, cm_value_t* const* args,
                              size_t argc, cm_error_t* out_error) {
    (void)user;
    if (argc != 1 || cm_value_kind(args[0]) != CM_VALUE_STR) {
        cm_error_free(out_error);   // ensure a clean slate before filling
        *out_error = (cm_error_t){ CM_ERROR_INVALID_ARG,
                                   strdup("FetchItem expects a str sku"),
                                   strdup(""), 0, 0 };
        return NULL;
    }
    cm_value_t* item = cm_value_struct("Item");
    cm_struct_set_field(item, "id",    cm_value_int(1));
    cm_struct_set_field(item, "sku",   cm_value_str("sword"));
    cm_struct_set_field(item, "price", cm_value_int(25));
    return item;
}

static cm_value_t* record_sale(void* user, cm_value_t* const* args,
                               size_t argc, cm_error_t* out_error) {
    (void)user; (void)args; (void)argc; (void)out_error;
    return cm_value_void();
}

/* registration: */
cm_capability_member_t store_members[] = {
    { "FetchItem", fetch_item },
    { "RecordSale", record_sale },
};
cm_engine_register_capability(engine, "shop.store",
                              store_members, 2, NULL);
```

With `schema shop 1.0.0` registered and a script calling
`shop.store.FetchItem(sku)`, the load verifies presence, the checker
verifies signatures, and the invocation dispatches to `fetch_item`.

## Checklist for a production host

1. Always check `NULL` returns from loaders and read `out_error`
   immediately; free it with `cm_error_free`.
2. Destroy in order: futures → values → contexts → programs → engine.
3. Give every context real limits; unbounded fuel is a policy decision,
   not a default.
4. Treat `CM_ERROR_LIMIT` as expected traffic shaping, not a crash.
5. Never free borrowed views; never borrow script strings.
6. Generate and include the schema header — the compile-time signature
   checks are the ABI's best defense against drift.
