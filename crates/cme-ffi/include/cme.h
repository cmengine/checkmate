/*
 * cme.h — the stable C host API for Checkmate (WHITEPAPER §13.2).
 *
 * The ABI mirrors the Rust host API (§13.1) over the shipped front end and
 * tree-walking interpreter: load a program (a source text, a file, or a
 * §10 mod tree), create an execution context under §5.5 limits, invoke
 * entry points, and read results or errors back.
 *
 * ══════════════════════════ OWNERSHIP MODEL ══════════════════════════
 *  - Every *_destroy / *_free accepts NULL and does nothing.
 *  - cm_engine_t, cm_program_t, cm_context_t, cm_future_t, cm_value_t are
 *    opaque; every handle you receive is owned and must be released with
 *    its destroy call exactly once.
 *  - Arguments passed to cm_invoke are BORROWED: the call clones them into
 *    the script, and the caller keeps ownership of every cm_value_t* in
 *    the argument array.
 *  - Borrowed views (const char* names, child cm_value_t* accessors) stay
 *    valid as long as the OWNING handle lives and is not mutated; they are
 *    never freed separately.
 *  - cm_error_t and cm_value_to_string return heap-allocated strings; free
 *    them with cm_error_free / cm_string_free.
 *
 * ══════════════════════════ LIFETIMES ══════════════════════════
 *  - A cm_program_t must outlive every cm_context_t created from it.
 *    Destroy contexts first.
 *  - Strings borrowed from a program (entry/interface names) live as long
 *    as the program.
 *  - A cm_future_t must be polled/destroyed before its context is
 *    destroyed.
 *
 * ══════════════════════════ THREADS ══════════════════════════
 *  - Engines and programs are immutable: share them across threads.
 *  - A context may be used from multiple threads — every invocation gets
 *    its own fuel cell and deadline (§5.5) — but one FUTURE or one VALUE
 *    handle is not synchronized: confine each to a single thread at a
 *    time.
 *  - §5.7 reentrancy: a host capability that re-enters its own executing
 *    invocation is prohibited. (Host capabilities are future work; the
 *    rule is stated so embedders can rely on it.)
 *
 * ══════════════════════════ FUTURES ══════════════════════════
 *  Invocations over the current interpreter complete synchronously, so
 *  the first cm_future_poll returns CM_READY or CM_ERROR. The CM_PENDING
 *  state exists for the continuation-splitting VM (§4, §5.2); the polling
 *  loop a host writes today is the loop it keeps when suspension lands.
 */

#ifndef CME_H
#define CME_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ */
/* Version                                                            */
/* ------------------------------------------------------------------ */

/** The Checkmate release this library was built from, e.g. "0.2.0". */
const char* cm_version(void);

/* ------------------------------------------------------------------ */
/* Status codes (accessor-style calls)                                */
/* ------------------------------------------------------------------ */

typedef enum cm_status {
    CM_OK = 0,
    /** A required argument was NULL. */
    CM_ERR_NULL = 1,
    /** The value kind does not match the accessor. */
    CM_ERR_KIND = 2,
    /** An index argument was out of bounds. */
    CM_ERR_INDEX = 3,
    /** A struct field (or map key) does not exist. */
    CM_ERR_MISSING = 4,
    /** The object is in the wrong state for the call. */
    CM_ERR_STATE = 5
} cm_status_t;

/* ------------------------------------------------------------------ */
/* Errors                                                             */
/* ------------------------------------------------------------------ */

/**
 * Coarse failure families. CM_ERROR_LIMIT groups the §5.5 budget
 * failures (fuel, deadline, call depth); CM_ERROR_UNKNOWN_ENTRY marks a
 * host request for an entry point the program does not declare (§2.1).
 */
typedef enum cm_error_kind {
    CM_ERROR_NONE = 0,
    /** The source failed to compile (parse, validate, or type check). */
    CM_ERROR_COMPILE = 1,
    /** A script-side runtime failure (overflow, division by zero, …). */
    CM_ERROR_RUNTIME = 2,
    /** A §5.5 limit tripped: fuel, wall-clock deadline, or call depth. */
    CM_ERROR_LIMIT = 3,
    /** The requested function / impl member does not exist. */
    CM_ERROR_UNKNOWN_ENTRY = 4,
    /** A host argument was unusable (NULL, not UTF-8, …). */
    CM_ERROR_INVALID_ARG = 5,
    /** A file or mod tree could not be read. */
    CM_ERROR_IO = 6
} cm_error_kind_t;

/**
 * An error report. Strings are heap-allocated and owned; free the whole
 * report with cm_error_free. `message` and `file` are never NULL on a
 * report filled by this library ("" where not applicable).
 *
 * Fill convention: functions that take `cm_error_t* out` ALWAYS write it —
 * on success with kind CM_ERROR_NONE and empty strings. The struct you
 * pass must be freshly zeroed (or cm_error_free'd since its last fill);
 * the call overwrites every field without freeing prior contents.
 */
typedef struct cm_error_t {
    cm_error_kind_t kind;
    char* message;
    /** Owning module's path for mod builds; "" otherwise. */
    char* file;
    /** 1-based line of the failure; 0 when there is no position. */
    int32_t line;
    /** 1-based CHARACTER column of the failure; 0 when none. */
    int32_t column;
} cm_error_t;

/** Initializer for a stack-allocated report: `cm_error_t err = CM_ERROR_INIT;`
 *  (a C99 compound literal, so re-assignment works too). */
#define CM_ERROR_INIT \
    ((cm_error_t) { CM_ERROR_NONE, NULL, NULL, 0, 0 })

/** Frees `error->message` and `error->file`, zeroes the report. NULL-safe. */
void cm_error_free(cm_error_t* error);

/** Frees a heap string returned by this library (cm_value_to_string). NULL-safe. */
void cm_string_free(char* text);

/* ------------------------------------------------------------------ */
/* Execution limits (§5.5)                                            */
/* ------------------------------------------------------------------ */

/**
 * §5.5 invocation constraints. Zero means "unset/default" for every field:
 * fuel 0 = unmetered, deadline 0 = none, depth 0 = the engine default
 * (1024). A host running scripts that legitimately recurse near the
 * default must provide adequate native stack or lower `max_call_depth`.
 */
typedef struct cm_limits_t {
    /** Deterministic operation budget (§5.5 fuel metering). */
    uint64_t fuel;
    /** Wall-clock budget in milliseconds, observed at safepoints only. */
    uint64_t deadline_ms;
    /** Maximum nested call frames. */
    size_t max_call_depth;
} cm_limits_t;

#define CM_LIMITS_INIT \
    ((cm_limits_t) { 0, 0, 0 })

/* ------------------------------------------------------------------ */
/* Opaque handles                                                     */
/* ------------------------------------------------------------------ */

typedef struct cm_engine_t cm_engine_t;
typedef struct cm_program_t cm_program_t;
typedef struct cm_context_t cm_context_t;
typedef struct cm_future_t cm_future_t;
typedef struct cm_value_t cm_value_t;

/* ------------------------------------------------------------------ */
/* Engine and program lifecycle                                       */
/* ------------------------------------------------------------------ */

cm_engine_t* cm_engine_new(void);
void cm_engine_destroy(cm_engine_t* engine);

/**
 * Compiles a source text. Applies the full gate: megaprogram expansion
 * when present (§8), parse, the §10 standalone-import check, and the type
 * checker. Returns NULL on any diagnostic — fill `out_error` (if
 * non-NULL) with kind CM_ERROR_COMPILE and rendered `path:line:column:
 * message` diagnostics (path empty for this entry).
 */
cm_program_t* cm_engine_load_source(cm_engine_t* engine,
                                    const char* source,
                                    cm_error_t* out_error);

/** As cm_engine_load_source, for a `.cm` file; IO failures are CM_ERROR_IO. */
cm_program_t* cm_engine_load_file(cm_engine_t* engine,
                                  const char* path,
                                  cm_error_t* out_error);

/**
 * Compiles a §10 mod tree: `root` names the mod directory (or its
 * mod.toml). Diagnostics re-anchor to the owning module — `out_error->file`
 * names it, so hosts never see virtual-text coordinates.
 */
cm_program_t* cm_engine_load_mod(cm_engine_t* engine,
                                 const char* root,
                                 cm_error_t* out_error);

void cm_program_destroy(cm_program_t* program);

/** Number of top-level functions (§2.1 entry points), declaration order. */
size_t cm_program_entry_count(const cm_program_t* program);

/** Borrowed name of entry `index`; NULL when out of bounds. */
const char* cm_program_entry_name(const cm_program_t* program, size_t index);

/** Number of §10.4 impl targets the program implements. */
size_t cm_program_interface_count(const cm_program_t* program);

/** Borrowed impl target path (`engine.gamemode`); NULL when out of bounds. */
const char* cm_program_interface_name(const cm_program_t* program, size_t index);

/** 1 when the program came from a mod tree, 0 otherwise (0 on NULL). */
int cm_program_is_mod(const cm_program_t* program);

/* ------------------------------------------------------------------ */
/* Contexts                                                           */
/* ------------------------------------------------------------------ */

/**
 * Creates an execution context over `program` under `limits` (borrowed;
 * NULL selects the defaults). The program must outlive the context.
 */
cm_context_t* cm_engine_create_context(cm_engine_t* engine,
                                       const cm_program_t* program,
                                       const cm_limits_t* limits);

void cm_context_destroy(cm_context_t* context);

/* ------------------------------------------------------------------ */
/* Invocation (§13.2)                                                 */
/* ------------------------------------------------------------------ */

/**
 * Invokes `member` — a top-level function when `target` is NULL or "",
 * otherwise a §10.4 impl member of that target path — with `argc`
 * borrowed arguments. The whitepaper's shape:
 *
 *     cm_future_t* f = cm_invoke(ctx, "engine.gamemode", "OnTick", NULL, 0);
 *
 * §5.7 reentrancy: while an invocation is active on a thread, a host
 * capability dispatched from it may not call cm_invoke on the SAME
 * context before the original call returns. The reentrant call yields an
 * ERROR future whose report carries kind CM_ERROR_INVALID_ARG and a
 * message naming §5.7; the original invocation is unaffected. Invoking a
 * DIFFERENT context (or from another thread) stays legal.
 *
 * Returns NULL only for host misuse: NULL context, NULL member, args NULL
 * with argc > 0, or a NULL element inside args. Every real failure
 * (unknown entry, script error, budget) rides the returned future.
 */
cm_future_t* cm_invoke(cm_context_t* context,
                       const char* target,
                       const char* member,
                       cm_value_t* const* args,
                       size_t argc);

/* ------------------------------------------------------------------ */
/* Futures                                                            */
/* ------------------------------------------------------------------ */

typedef enum cm_poll_result {
    /** Invocation still running (future VMs; unreachable today). */
    CM_PENDING = 0,
    /** Finished successfully; take the value with cm_future_take_value. */
    CM_READY = 1,
    /** Failed; take the report with cm_future_get_error. */
    CM_ERROR = 2
} cm_poll_result_t;

/**
 * Polls the future. Idempotent: READY and ERROR stay stable across
 * repeated polls. When `out_value` is non-NULL and the future is READY,
 * it receives a NEWLY OWNED handle (NULL when the value was already
 * taken); the caller destroys it. The whitepaper's loop passes NULL:
 *
 *     while ((result = cm_future_poll(future, NULL)) == CM_PENDING) { … }
 */
cm_poll_result_t cm_future_poll(cm_future_t* future, cm_value_t** out_value);

/**
 * Moves the result value out of a READY future: an owned handle, or NULL
 * when the future is not READY, carries no value, or was already taken.
 * Polling afterwards stays stable (CM_READY without a value).
 */
cm_value_t* cm_future_take_value(cm_future_t* future);

/**
 * Copies the error report out of an ERROR future. The report is a fresh
 * owned copy on every call — free it with cm_error_free. A future that is
 * not in error yields kind CM_ERROR_NONE with an empty message, so this
 * is always safe to call.
 */
cm_error_t cm_future_get_error(cm_future_t* future);

void cm_future_destroy(cm_future_t* future);

/* ------------------------------------------------------------------ */
/* Schema contract (WHITEPAPER §9)                                    */
/* ------------------------------------------------------------------ */

/**
 * One parsed `.cm` schema file (§9.2: one namespace root per file). The
 * schema configures script-side checking — imports, capability calls,
 * `impl` completeness, `since` version gating, `requires` edges — for
 * every program the engine loads AFTER registration.
 */
typedef struct cm_schema_t cm_schema_t;

/**
 * Parses schema source text. Returns NULL on any defect, filling
 * `out_error` with kind CM_ERROR_COMPILE and the rendered diagnostics.
 */
cm_schema_t* cm_schema_parse(const char* text, cm_error_t* out_error);

/** As cm_schema_parse, reading a `.cm` file; IO failures are CM_ERROR_IO. */
cm_schema_t* cm_schema_parse_file(const char* path, cm_error_t* out_error);

void cm_schema_destroy(cm_schema_t* schema);

/**
 * Registers the schema with the engine. The whole registered set is
 * re-validated (§9.2/§9.4): duplicate namespaces, cross-namespace type
 * collisions, and unresolved `requires` edges fail with CM_ERROR_INVALID_ARG
 * and the set is left unchanged. The schema handle stays owned by the
 * caller and may be destroyed after registration (the engine keeps a copy).
 */
cm_status_t cm_engine_register_schema(cm_engine_t* engine,
                                      const cm_schema_t* schema);

/* ------------------------------------------------------------------ */
/* Capability providers (§9.1, §13.2)                                 */
/* ------------------------------------------------------------------ */

/**
 * The C provider contract: invoked when a script calls a member of the
 * capability this provider is registered for. `args` are BORROWED
 * (owned by the engine; valid for the call only); `argc` is their count.
 * Return a NEWLY OWNED cm_value_t* (Value::Void for void members), or
 * NULL with `out_error` filled to fail the invocation. `user` is the
 * opaque pointer given at registration.
 */
typedef cm_value_t* (*cm_capability_fn)(void* user,
                                        cm_value_t* const* args,
                                        size_t argc,
                                        cm_error_t* out_error);

/** One capability member: its schema name plus the provider function. */
typedef struct cm_capability_member {
    const char* name;
    cm_capability_fn fn;
} cm_capability_member_t;

/**
 * Registers the provider for a capability path (`namespace.capability`,
 * §9.1). Members are borrowed for the call. The provider-presence check
 * runs at LOAD time: a program that calls a capability without a
 * registered provider fails to load, so wiring mistakes are deterministic
 * before any invocation.
 *
 * THREADS: `user` must stay valid for the engine's lifetime; provider
 * calls may arrive from any thread that invokes into the engine (§5:
 * invocations race with nothing, but the SAME provider function may run
 * concurrently across contexts).
 */
cm_status_t cm_engine_register_capability(cm_engine_t* engine,
                                          const char* path,
                                          const cm_capability_member_t* members,
                                          size_t count,
                                          void* user);

/* ------------------------------------------------------------------ */
/* Values                                                             */
/* ------------------------------------------------------------------ */

typedef enum cm_value_kind {
    CM_VALUE_INVALID = 0,
    CM_VALUE_VOID = 1,
    CM_VALUE_INT = 2,
    CM_VALUE_FLOAT = 3,
    CM_VALUE_BOOL = 4,
    CM_VALUE_STR = 5,
    CM_VALUE_STRUCT = 6,
    CM_VALUE_ENUM = 7,
    CM_VALUE_ARRAY = 8,
    CM_VALUE_MAP = 9
} cm_value_kind_t;

/* Constructors — each returns an OWNED handle, NULL on invalid input
   (NULL text, invalid UTF-8). */

cm_value_t* cm_value_void(void);
cm_value_t* cm_value_int(int64_t value);
cm_value_t* cm_value_float(double value);
/** Non-zero is true. */
cm_value_t* cm_value_bool(int value);
/** NUL-terminated, must be valid UTF-8. */
cm_value_t* cm_value_str(const char* text);
/** Length-delimited variant; must be valid UTF-8. */
cm_value_t* cm_value_str_len(const char* text, size_t length);
/** Empty struct of `type_name` ("" allowed but useless). */
cm_value_t* cm_value_struct(const char* type_name);
/** Empty enum value: `type_name.variant` with no payload yet. */
cm_value_t* cm_value_enum(const char* type_name, const char* variant);
cm_value_t* cm_value_array(void);
cm_value_t* cm_value_map(void);

/* Builders — the child handle is MOVED IN and consumed on success: do not
   destroy it afterwards. On failure the child is left untouched and still
   owned by the caller. */

/** Sets or replaces field `name` (clones nothing; moves `field`). */
cm_status_t cm_struct_set_field(cm_value_t* strukt,
                                const char* name,
                                cm_value_t* field);
/** Appends a payload value in order. */
cm_status_t cm_enum_push(cm_value_t* enum_value, cm_value_t* payload);
/** Appends an element. */
cm_status_t cm_array_push(cm_value_t* array, cm_value_t* item);
/** Sets `key -> value`, replacing an existing equal key. Moves both. */
cm_status_t cm_map_set(cm_value_t* map, cm_value_t* key, cm_value_t* value);

/** Deep copy: fully independent. NULL returns NULL. */
cm_value_t* cm_value_clone(const cm_value_t* value);

/* Accessors — scalars write `out` and return CM_OK, or return
   CM_ERR_NULL / CM_ERR_KIND without touching `out`.
   String results are OWNED C strings (free with cm_string_free); a NUL
   byte inside script text renders as U+FFFD because C strings cannot
   carry embedded NULs. `*out_length` (when requested) reports the
   ORIGINAL byte length. Only child cm_value_t* views are borrowed. */

cm_value_kind_t cm_value_kind(const cm_value_t* value);
cm_status_t cm_value_as_int(const cm_value_t* value, int64_t* out);
cm_status_t cm_value_as_float(const cm_value_t* value, double* out);
cm_status_t cm_value_as_bool(const cm_value_t* value, int* out);
/** Owned copy of the text; free with cm_string_free. */
cm_status_t cm_value_as_str(const cm_value_t* value,
                            char** out,
                            size_t* out_length);
/** Owned copy of the type name; free with cm_string_free. */
cm_status_t cm_value_struct_name(const cm_value_t* value, char** out);
/** Owned copy of the enum's type name; free with cm_string_free. */
cm_status_t cm_value_enum_type(const cm_value_t* value, char** out);
/** Owned copy of the enum's variant name; free with cm_string_free. */
cm_status_t cm_value_enum_variant(const cm_value_t* value, char** out);

/**
 * Element counts by kind: str = BYTE length, array = elements, map =
 * entries, struct = fields, enum = payload count; 0 for scalars, void,
 * and NULL.
 */
size_t cm_value_len(const cm_value_t* value);

/** Borrowed element `index`; NULL when out of bounds or wrong kind. */
const cm_value_t* cm_value_array_get(const cm_value_t* value, size_t index);

/** Borrowed key of entry `index`; NULL when out of bounds or wrong kind. */
const cm_value_t* cm_value_map_key(const cm_value_t* value, size_t index);

/** Borrowed payload value `index`; NULL when out of bounds or wrong kind. */
const cm_value_t* cm_value_enum_payload(const cm_value_t* value, size_t index);

/** Borrowed value of entry `index`; NULL when out of bounds or wrong kind. */
const cm_value_t* cm_value_map_value(const cm_value_t* value, size_t index);

/** Borrowed field value; NULL when absent or wrong kind. */
const cm_value_t* cm_value_struct_field(const cm_value_t* value,
                                        const char* name);

/** Canonical CMON display (§11.1, the language's own rendering); owned. */
char* cm_value_to_string(const cm_value_t* value);

void cm_value_destroy(cm_value_t* value);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* CME_H */
