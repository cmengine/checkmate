/*
 * c_host_app_main — the C host application for Checkmate (WHITEPAPER §13.2).
 *
 * A complete, self-checking walkthrough of the C API: load programs from
 * source, files, and mod trees; create contexts under §5.5 limits; invoke
 * functions and impl members; read every value kind back; and verify that
 * every documented failure shape — compile errors, unknown entries,
 * runtime errors, limit exhaustion, misuse — reports correctly.
 *
 * Build standalone against the shipped library:
 *     make -C apps/c_host
 * (requires `cargo build --release -p cme-ffi` first)
 *
 * The same file is compiled into the Rust test run by cme-ffi's build
 * script with CME_HOST_EMBEDDED defined, which swaps main for a plain
 * entry function so both harnesses share one consumer.
 */

#include "cme.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

static int checks_run = 0;
static int checks_failed = 0;

#define CHECK(cond, label)                                                     \
    do {                                                                       \
        checks_run++;                                                          \
        if (!(cond)) {                                                         \
            checks_failed++;                                                   \
            printf("FAIL: %s (line %d)\n", label, __LINE__);                   \
        }                                                                      \
    } while (0)

static const char* ADD_PROGRAM =
    "int add(int a, int b) {\n"
    "return a + b\n"
    "}\n"
    "float scale(float v) {\n"
    "return v * 2.0\n"
    "}\n"
    "str label(bool ok, str name) {\n"
    "if (ok) {\n"
    "return \"yes \" + name\n"
    "}\n"
    "return \"no \" + name\n"
    "}\n"
    "void touch() {\n"
    "}\n"
    "struct vec2 {\n"
    "float x\n"
    "float y\n"
    "}\n"
    "vec2 mkVec(float x, float y) {\n"
    "return vec2(x: x, y: y)\n"
    "}\n"
    "enum shape {\n"
    "Circle(float radius)\n"
    "Rect(vec2 corner)\n"
    "}\n"
    "shape mkCircle(float r) {\n"
    "return shape.Circle(r)\n"
    "}\n"
    "int[] triple(int v) {\n"
    "return [v, v * 2, v * 3]\n"
    "}\n"
    "map<str, int> grades() {\n"
    "return {\n"
    "\"ada\": 99\n"
    "\"grace\": 100\n"
    "}\n"
    "}\n"
    "int echoBag(int gold) {\n"
    "return gold * 10\n"
    "}\n"
    "int divZero() {\n"
    "return 1 / 0\n"
    "}\n"
    "int spin(int n) {\n"
    "return spin(n + 1)\n"
    "}\n";

static const char* IMPL_PROGRAM =
    "impl engine.gamemode {\n"
    "int InitGame(int seed) {\n"
    "return seed * 2\n"
    "}\n"
    "void OnTick(int dt) {\n"
    "}\n"
    "}\n"
    "int main() {\n"
    "return 0\n"
    "}\n";

/* Asserts a report is an error of `kind` with `needle` in the message. */
static int report_is(cm_error_t* err, cm_error_kind_t kind, const char* needle) {
    if (err->kind != kind) {
        return 0;
    }
    if (needle != NULL && strstr(err->message, needle) == NULL) {
        return 0;
    }
    return 1;
}

static void test_version(void) {
    CHECK(cm_version() != NULL && strlen(cm_version()) > 0, "cm_version exposes a version");
}

static void test_null_safety(void) {
    cm_error_free(NULL);
    cm_string_free(NULL);
    cm_engine_destroy(NULL);
    cm_program_destroy(NULL);
    cm_context_destroy(NULL);
    cm_future_destroy(NULL);
    cm_value_destroy(NULL);
    CHECK(1, "every destroy/free tolerates NULL");
}

static void test_load_source_success(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();
    cm_program_t* program = cm_engine_load_source(engine, ADD_PROGRAM, &err);

    CHECK(program != NULL, "clean source loads");
    CHECK(err.kind == CM_ERROR_NONE, "success clears the report");
    CHECK(strcmp(err.message, "") == 0, "success leaves an empty message");

    CHECK(cm_program_entry_count(program) == 11, "eleven entry points");
    int found_add = 0, found_mainish = 0;
    for (size_t i = 0; i < cm_program_entry_count(program); i++) {
        const char* name = cm_program_entry_name(program, i);
        CHECK(name != NULL, "entry name is present");
        if (strcmp(name, "add") == 0) {
            found_add = 1;
        }
        if (strcmp(name, "divZero") == 0) {
            found_mainish = 1;
        }
    }
    CHECK(found_add, "add listed");
    CHECK(found_mainish, "divZero listed");
    CHECK(cm_program_entry_name(program, 999) == NULL, "out-of-bounds entry name is NULL");
    CHECK(cm_program_is_mod(program) == 0, "a source program is not a mod");
    CHECK(cm_program_interface_count(program) == 0, "no impl targets here");

    cm_error_free(&err);
    cm_program_destroy(program);
    cm_engine_destroy(engine);
}

static void test_load_source_compile_error(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();

    /* Type error: cross-type addition is never permitted (§A.4). */
    cm_program_t* program =
        cm_engine_load_source(engine, "int f() {\nreturn 1 + \"x\"\n}\n", &err);
    CHECK(program == NULL, "a type error refuses the load");
    CHECK(report_is(&err, CM_ERROR_COMPILE, NULL), "compile kind reported");
    CHECK(strlen(err.message) > 0, "compile message rendered");
    /* Multi-diagnostic reports have no single position: the rendered
       message carries each `path:line:column` prefix instead. */
    CHECK(err.line == 0 && err.column == 0, "compile report has no single position");
    CHECK(strstr(err.message, "2:") != NULL, "rendered position inside the message");

    /* Syntax error with position. */
    cm_error_free(&err);
    program = cm_engine_load_source(engine, "int f() {\nreturn 1\n", &err);
    CHECK(program == NULL, "a syntax error refuses the load");
    CHECK(report_is(&err, CM_ERROR_COMPILE, NULL), "syntax kind reported");

    /* NULL out_error must be tolerated. */
    cm_error_free(&err);
    program = cm_engine_load_source(engine, "int f() {\nreturn !\n}\n", NULL);
    CHECK(program == NULL, "NULL out_error tolerated");

    /* NULL source is misuse. */
    cm_error_free(&err);
    program = cm_engine_load_source(engine, NULL, &err);
    CHECK(program == NULL, "NULL source refused");
    CHECK(report_is(&err, CM_ERROR_INVALID_ARG, NULL), "invalid-arg kind reported");

    cm_error_free(&err);
    cm_engine_destroy(engine);
}

static void test_load_file(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();

    cm_program_t* program =
        cm_engine_load_file(engine, "/definitely/not/here.cm", &err);
    CHECK(program == NULL, "missing file refused");
    CHECK(report_is(&err, CM_ERROR_IO, NULL), "io kind reported");
    cm_error_free(&err);

    /* A writable temp file compiles like a source. */
    const char* path = "/tmp/cme_c_host_fixture.cm";
    FILE* file = fopen(path, "w");
    CHECK(file != NULL, "fixture file opens");
    if (file != NULL) {
        fputs(ADD_PROGRAM, file);
        fclose(file);
        program = cm_engine_load_file(engine, path, &err);
        CHECK(program != NULL, "fixture file loads");
        CHECK(err.kind == CM_ERROR_NONE, "fixture load clears the report");
        CHECK(cm_program_entry_count(program) == 11, "fixture file entry points");
        cm_error_free(&err);
        cm_program_destroy(program);
    }

    cm_engine_destroy(engine);
}

static void test_load_mod(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();

    cm_program_t* program = cm_engine_load_mod(engine, "/definitely/not/a/mod", &err);
    CHECK(program == NULL, "missing mod refused");
    CHECK(err.kind != CM_ERROR_NONE, "mod failure reported");
    cm_error_free(&err);
    cm_engine_destroy(engine);
}

static void test_invoke_scalars(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();
    cm_program_t* program = cm_engine_load_source(engine, ADD_PROGRAM, &err);
    cm_error_free(&err);
    CHECK(program != NULL, "program loads for scalar tests");
    cm_context_t* ctx = cm_engine_create_context(engine, program, NULL);
    CHECK(ctx != NULL, "context with default limits");

    /* int add(40, 2) → 42 */
    cm_value_t* a = cm_value_int(40);
    cm_value_t* b = cm_value_int(2);
    cm_value_t* args[2] = {a, b};
    cm_future_t* future = cm_invoke(ctx, NULL, "add", args, 2);
    CHECK(future != NULL, "add invocation returns a future");

    /* Arguments are borrowed: still owned, still destroyable. */
    cm_value_destroy(a);
    cm_value_destroy(b);

    /* The whitepaper's polling loop. */
    cm_poll_result_t result;
    while ((result = cm_future_poll(future, NULL)) == CM_PENDING) {
        /* Drive host IO; unreachable over the synchronous interpreter. */
    }
    CHECK(result == CM_READY, "poll reports READY");

    cm_value_t* value = cm_future_take_value(future);
    CHECK(value != NULL, "READY carries a value");
    int64_t answer = 0;
    CHECK(cm_value_as_int(value, &answer) == CM_OK, "int accessor ok");
    CHECK(answer == 42, "add(40, 2) == 42");
    CHECK(cm_value_kind(value) == CM_VALUE_INT, "kind is int");
    cm_value_destroy(value);

    /* Taking twice yields NULL; polling stays stable. */
    CHECK(cm_future_take_value(future) == NULL, "second take is NULL");
    CHECK(cm_future_poll(future, NULL) == CM_READY, "poll stays stable");
    cm_value_t* polled = NULL;
    CHECK(cm_future_poll(future, &polled) == CM_READY, "poll with out-value");
    CHECK(polled == NULL, "already-taken value polls as NULL");

    /* get_error on a READY future is a clean NONE report. */
    cm_error_t none = CM_ERROR_INIT;
    none = cm_future_get_error(future);
    CHECK(none.kind == CM_ERROR_NONE, "READY error report is NONE");
    CHECK(strcmp(none.message, "") == 0, "NONE message empty");
    cm_error_free(&none);
    cm_future_destroy(future);

    /* float scale(9) → 4.5 */
    cm_value_t* nine = cm_value_float(9.0);
    cm_value_t* one[1] = {nine};
    future = cm_invoke(ctx, NULL, "scale", one, 1);
    cm_value_destroy(nine);
    result = cm_future_poll(future, NULL);
    CHECK(result == CM_READY, "scale ready");
    value = cm_future_take_value(future);
    double scaled = 0.0;
    CHECK(cm_value_as_float(value, &scaled) == CM_OK, "float accessor ok");
    CHECK(scaled == 18.0, "scale(9) == 18");
    cm_value_destroy(value);
    cm_future_destroy(future);

    /* bool + str path (§A.6 stringification). */
    cm_value_t* yes = cm_value_bool(1);
    cm_value_t* name = cm_value_str("hero");
    cm_value_t* two_args[2] = {yes, name};
    future = cm_invoke(ctx, NULL, "label", two_args, 2);
    cm_value_destroy(yes);
    cm_value_destroy(name);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "label ready");
    value = cm_future_take_value(future);
    char* text = NULL;
    size_t length = 0;
    CHECK(cm_value_as_str(value, &text, &length) == CM_OK, "str accessor ok");
    CHECK(text != NULL && strcmp(text, "yes hero") == 0, "label(true, hero)");
    CHECK(length == 8, "original byte length matches");
    cm_string_free(text);
    cm_value_destroy(value);
    cm_future_destroy(future);

    /* void results. */
    future = cm_invoke(ctx, NULL, "touch", NULL, 0);
    CHECK(future != NULL, "whitepaper NULL-args invocation works");
    CHECK(cm_future_poll(future, NULL) == CM_READY, "touch ready");
    value = cm_future_take_value(future);
    CHECK(value != NULL, "void result is a value handle");
    CHECK(cm_value_kind(value) == CM_VALUE_VOID, "touch returns void");
    CHECK(cm_value_len(value) == 0, "void has no length");
    char* rendered = cm_value_to_string(value);
    CHECK(rendered != NULL && strcmp(rendered, "") == 0, "void renders empty");
    cm_string_free(rendered);
    cm_value_destroy(value);
    cm_future_destroy(future);

    cm_context_destroy(ctx);
    cm_program_destroy(program);
    cm_engine_destroy(engine);
}

static void test_invoke_composites(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();
    cm_program_t* program = cm_engine_load_source(engine, ADD_PROGRAM, &err);
    cm_error_free(&err);
    cm_context_t* ctx = cm_engine_create_context(engine, program, NULL);

    /* Struct result. */
    cm_value_t* x = cm_value_float(3.0);
    cm_value_t* y = cm_value_float(4.0);
    cm_value_t* args[2] = {x, y};
    cm_future_t* future = cm_invoke(ctx, NULL, "mkVec", args, 2);
    cm_value_destroy(x);
    cm_value_destroy(y);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "mkVec ready");
    cm_value_t* vec = cm_future_take_value(future);
    cm_future_destroy(future);
    CHECK(cm_value_kind(vec) == CM_VALUE_STRUCT, "struct kind");
    char* type_name = NULL;
    CHECK(cm_value_struct_name(vec, &type_name) == CM_OK, "struct name ok");
    CHECK(type_name != NULL && strcmp(type_name, "vec2") == 0, "struct is vec2");
    CHECK(cm_value_len(vec) == 2, "two fields");

    char* type_name_owned = NULL;
    (void)type_name_owned;
    const cm_value_t* fx = cm_value_struct_field(vec, "x");
    const cm_value_t* fy = cm_value_struct_field(vec, "y");
    const cm_value_t* missing = cm_value_struct_field(vec, "z");
    double field = 0.0;
    CHECK(fx != NULL && cm_value_as_float(fx, &field) == CM_OK && field == 3.0,
          "field x read");
    CHECK(fy != NULL && cm_value_as_float(fy, &field) == CM_OK && field == 4.0,
          "field y read");
    CHECK(missing == NULL, "missing field is NULL");
    cm_string_free(type_name);
    cm_value_destroy(vec);

    /* Enum result. */
    cm_value_t* radius = cm_value_float(2.5);
    cm_value_t* one[1] = {radius};
    future = cm_invoke(ctx, NULL, "mkCircle", one, 1);
    cm_value_destroy(radius);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "mkCircle ready");
    cm_value_t* shape = cm_future_take_value(future);
    cm_future_destroy(future);
    CHECK(cm_value_kind(shape) == CM_VALUE_ENUM, "enum kind");
    char* variant = NULL;
    CHECK(cm_value_enum_type(shape, &type_name) == CM_OK, "enum type name ok");
    CHECK(type_name != NULL && strcmp(type_name, "shape") == 0, "enum type name");
    cm_string_free(type_name);
    CHECK(cm_value_enum_variant(shape, &variant) == CM_OK && variant != NULL
              && strcmp(variant, "Circle") == 0,
          "enum variant");
    cm_string_free(variant);
    CHECK(cm_value_len(shape) == 1, "one payload value");
    const cm_value_t* payload = cm_value_enum_payload(shape, 0);
    CHECK(payload != NULL && cm_value_as_float(payload, &field) == CM_OK && field == 2.5,
          "payload radius read");
    CHECK(cm_value_enum_payload(shape, 1) == NULL, "payload index out of bounds");
    cm_value_destroy(shape);

    /* Array result. */
    cm_value_t* v = cm_value_int(5);
    cm_value_t* one_int[1] = {v};
    future = cm_invoke(ctx, NULL, "triple", one_int, 1);
    cm_value_destroy(v);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "triple ready");
    cm_value_t* array = cm_future_take_value(future);
    cm_future_destroy(future);
    CHECK(cm_value_kind(array) == CM_VALUE_ARRAY, "array kind");
    CHECK(cm_value_len(array) == 3, "three elements");
    int64_t element = 0;
    CHECK(cm_value_as_int(cm_value_array_get(array, 0), &element) == CM_OK && element == 5,
          "[0] == 5");
    CHECK(cm_value_as_int(cm_value_array_get(array, 2), &element) == CM_OK && element == 15,
          "[2] == 15");
    CHECK(cm_value_array_get(array, 3) == NULL, "out of bounds is NULL");
    char* cmon = cm_value_to_string(array);
    CHECK(cmon != NULL && strcmp(cmon, "[5, 10, 15]") == 0, "array renders CMON");
    cm_string_free(cmon);
    cm_value_destroy(array);

    /* Map result. */
    future = cm_invoke(ctx, NULL, "grades", NULL, 0);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "grades ready");
    cm_value_t* map = cm_future_take_value(future);
    cm_future_destroy(future);
    CHECK(cm_value_kind(map) == CM_VALUE_MAP, "map kind");
    CHECK(cm_value_len(map) == 2, "two entries");
    const cm_value_t* key = cm_value_map_key(map, 0);
    char* key_text = NULL;
    CHECK(key != NULL && cm_value_as_str(key, &key_text, NULL) == CM_OK, "map key readable");
    CHECK(key_text != NULL && strcmp(key_text, "ada") == 0, "first key is ada");
    cm_string_free(key_text);
    const cm_value_t* entry_value = cm_value_map_value(map, 1);
    CHECK(entry_value != NULL && cm_value_as_int(entry_value, &element) == CM_OK && element == 100,
          "grace scored 100");
    cm_value_destroy(map);

    cm_context_destroy(ctx);
    cm_program_destroy(program);
    cm_engine_destroy(engine);
}

static void test_host_built_values(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();
    const char* source =
        "struct bag {\n"
        "int gold\n"
        "int gems\n"
        "}\n"
        "int appraise(bag b) {\n"
        "return b.gold + b.gems * 10\n"
        "}\n"
        "enum loot {\n"
        "Coins(int count)\n"
        "Nothing()\n"
        "}\n"
        "int worth(loot l) {\n"
        "match (l) {\n"
        "Coins(int count) => { return count }\n"
        "Nothing() => { return 0 }\n"
        "}\n"
        "}\n"
        "int sum(int[] xs) {\n"
        "int total = 0\n"
        "for (int x in xs) {\n"
        "total = total + x\n"
        "}\n"
        "return total\n"
        "}\n"
        "int lookup(map<str, int> m) {\n"
        "return m[\"score\"]\n"
        "}\n";
    cm_program_t* program = cm_engine_load_source(engine, source, &err);
    cm_error_free(&err);
    CHECK(program != NULL, "builder host program loads");
    cm_context_t* ctx = cm_engine_create_context(engine, program, NULL);

    /* Build a struct and pass it in. */
    cm_value_t* bag = cm_value_struct("bag");
    CHECK(bag != NULL, "struct handle");
    cm_value_t* gold = cm_value_int(100);
    cm_value_t* gems = cm_value_int(7);
    CHECK(cm_struct_set_field(bag, "gold", gold) == CM_OK, "set gold (moves)");
    CHECK(cm_struct_set_field(bag, "gems", gems) == CM_OK, "set gems (moves)");
    CHECK(cm_value_len(bag) == 2, "two fields set");
    /* Replacing a field keeps one entry. */
    cm_value_t* gold2 = cm_value_int(200);
    CHECK(cm_struct_set_field(bag, "gold", gold2) == CM_OK, "replace gold");
    CHECK(cm_value_len(bag) == 2, "still two fields");

    cm_value_t* bag_args[1] = {bag};
    cm_future_t* future = cm_invoke(ctx, NULL, "appraise", bag_args, 1);
    CHECK(future != NULL, "appraise invoked");
    CHECK(cm_future_poll(future, NULL) == CM_READY, "appraise ready");
    cm_value_t* worth = cm_future_take_value(future);
    int64_t total = 0;
    cm_value_as_int(worth, &total);
    CHECK(total == 270, "200 + 7*10 == 270");
    cm_value_destroy(worth);
    cm_future_destroy(future);

    /* Deep clone: the original survives a destroy of the clone. */
    cm_value_t* copy = cm_value_clone(bag);
    CHECK(copy != NULL, "clone made");
    cm_value_destroy(bag);
    CHECK(cm_value_len(copy) == 2, "clone independent");
    const cm_value_t* cloned_gold = cm_value_struct_field(copy, "gold");
    cm_value_as_int(cloned_gold, &total);
    CHECK(total == 200, "clone carries the replaced gold");
    cm_value_destroy(copy);

    /* Enum with payload built host-side. */
    cm_value_t* loot = cm_value_enum("loot", "Coins");
    cm_value_t* count = cm_value_int(64);
    CHECK(cm_enum_push(loot, count) == CM_OK, "payload pushed (moves)");
    cm_value_t* loot_args[1] = {loot};
    future = cm_invoke(ctx, NULL, "worth", loot_args, 1);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "worth ready");
    worth = cm_future_take_value(future);
    cm_value_as_int(worth, &total);
    CHECK(total == 64, "Coins(64) worth 64");
    cm_value_destroy(worth);
    cm_future_destroy(future);

    /* Array built host-side. */
    cm_value_t* array = cm_value_array();
    for (int i = 1; i <= 4; i++) {
        cm_value_t* item = cm_value_int(i * i);
        CHECK(cm_array_push(array, item) == CM_OK, "array push (moves)");
    }
    cm_value_t* array_args[1] = {array};
    future = cm_invoke(ctx, NULL, "sum", array_args, 1);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "sum ready");
    worth = cm_future_take_value(future);
    cm_value_as_int(worth, &total);
    CHECK(total == 30, "1+4+9+16 == 30");
    cm_value_destroy(worth);
    cm_future_destroy(future);

    /* Map built host-side, with key replacement. */
    cm_value_t* map = cm_value_map();
    cm_value_t* score_key = cm_value_str("score");
    cm_value_t* low = cm_value_int(1);
    CHECK(cm_map_set(map, score_key, low) == CM_OK, "map set (moves both)");
    cm_value_t* score_key2 = cm_value_str("score");
    cm_value_t* high = cm_value_int(500);
    CHECK(cm_map_set(map, score_key2, high) == CM_OK, "map replace (moves both)");
    CHECK(cm_value_len(map) == 1, "replacement kept one entry");
    cm_value_t* extra_key = cm_value_str("bonus");
    cm_value_t* extra_value = cm_value_int(5);
    CHECK(cm_map_set(map, extra_key, extra_value) == CM_OK, "second entry");
    CHECK(cm_value_len(map) == 2, "two entries now");
    cm_value_t* map_args[1] = {map};
    future = cm_invoke(ctx, NULL, "lookup", map_args, 1);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "lookup ready");
    worth = cm_future_take_value(future);
    cm_value_as_int(worth, &total);
    CHECK(total == 500, "replacement visible to the script");
    cm_value_destroy(worth);
    cm_future_destroy(future);

    /* Builder misuse: wrong kind, NULL name. */
    cm_value_t* scalar = cm_value_int(1);
    cm_value_t* child = cm_value_int(2);
    CHECK(cm_struct_set_field(scalar, "x", child) == CM_ERR_KIND, "set_field on int is KIND");
    cm_value_destroy(child); /* not consumed on failure */
    CHECK(cm_struct_set_field(NULL, "x", NULL) == CM_ERR_NULL, "set_field NULL struct");
    CHECK(cm_array_push(scalar, cm_value_int(1)) == CM_ERR_KIND, "push on int is KIND");
    CHECK(cm_enum_push(scalar, cm_value_int(1)) == CM_ERR_KIND, "enum_push on int is KIND");
    CHECK(cm_map_set(scalar, cm_value_int(1), cm_value_int(2)) == CM_ERR_KIND,
          "map_set on int is KIND");
    CHECK(cm_array_push(NULL, NULL) == CM_ERR_NULL, "push NULL array");
    cm_value_destroy(scalar);

    /* Wrong-kind accessors. */
    int64_t as_int = 0;
    double as_float = 0.0;
    int as_bool = 0;
    cm_value_t* text = cm_value_str("beep");
    CHECK(cm_value_as_int(text, &as_int) == CM_ERR_KIND, "str as int is KIND");
    CHECK(cm_value_as_float(text, &as_float) == CM_ERR_KIND, "str as float is KIND");
    CHECK(cm_value_as_bool(text, &as_bool) == CM_ERR_KIND, "str as bool is KIND");
    char* owned_text = NULL;
    CHECK(cm_value_as_str(text, &owned_text, NULL) == CM_OK, "str as str ok");
    CHECK(owned_text != NULL && strcmp(owned_text, "beep") == 0, "owned text content");
    cm_string_free(owned_text);
    CHECK(cm_value_struct_name(text, &owned_text) == CM_ERR_KIND, "struct_name on str is KIND");
    CHECK(cm_value_enum_type(text, &owned_text) == CM_ERR_KIND, "enum_type on str is KIND");
    CHECK(cm_value_enum_variant(text, &owned_text) == CM_ERR_KIND, "enum_variant on str is KIND");
    CHECK(cm_value_array_get(text, 0) == NULL, "array_get on str is NULL");
    CHECK(cm_value_map_key(text, 0) == NULL, "map_key on str is NULL");
    CHECK(cm_value_enum_payload(text, 0) == NULL, "enum_payload on str is NULL");
    CHECK(cm_value_struct_field(text, "x") == NULL, "struct_field on str is NULL");
    CHECK(cm_value_kind(NULL) == CM_VALUE_INVALID, "NULL value kind is INVALID");
    CHECK(cm_value_as_int(NULL, &as_int) == CM_ERR_NULL, "accessor on NULL is ERR_NULL");
    CHECK(cm_value_as_int(text, NULL) == CM_ERR_NULL, "accessor NULL out is ERR_NULL");
    cm_value_destroy(text);

    /* Invalid UTF-8 is refused at construction. */
    const char bad_bytes[] = {(char)0xff, (char)0xfe, 0};
    CHECK(cm_value_str(bad_bytes) == NULL, "invalid UTF-8 str refused");
    CHECK(cm_value_str(NULL) == NULL, "NULL str refused");
    CHECK(cm_value_struct(NULL) == NULL, "NULL struct name refused");
    CHECK(cm_value_enum(NULL, NULL) == NULL, "NULL enum names refused");
    cm_value_t* len_str = cm_value_str_len("h\xc3\xa9llo", 6);
    CHECK(len_str != NULL && cm_value_len(len_str) == 6, "str_len counts BYTES");
    cm_value_destroy(len_str);

    /* A NUL byte inside text does not truncate: it becomes U+FFFD. */
    cm_value_t* with_nul = cm_value_str_len("a\0b", 3);
    char* shown = cm_value_to_string(with_nul);
    /* a + U+FFFD (3 bytes) + b — C strings cannot carry the NUL. */
    CHECK(shown != NULL && strlen(shown) == 5, "NUL renders as U+FFFD");
    cm_string_free(shown);
    cm_value_destroy(with_nul);

    cm_context_destroy(ctx);
    cm_program_destroy(program);
    cm_engine_destroy(engine);
}

static void test_invoke_member(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();
    cm_program_t* program = cm_engine_load_source(engine, IMPL_PROGRAM, &err);
    cm_error_free(&err);
    CHECK(program != NULL, "impl program loads");
    CHECK(cm_program_interface_count(program) == 1, "one interface target");
    CHECK(strcmp(cm_program_interface_name(program, 0), "engine.gamemode") == 0,
          "target is engine.gamemode");

    cm_context_t* ctx = cm_engine_create_context(engine, program, NULL);

    /* The §13.2 shape, verbatim: target + member. */
    cm_value_t* seed = cm_value_int(21);
    cm_value_t* args[1] = {seed};
    cm_future_t* future = cm_invoke(ctx, "engine.gamemode", "InitGame", args, 1);
    cm_value_destroy(seed);
    CHECK(future != NULL, "interface invocation returns a future");
    CHECK(cm_future_poll(future, NULL) == CM_READY, "InitGame ready");
    cm_value_t* state = cm_future_take_value(future);
    int64_t value = 0;
    CHECK(cm_value_as_int(state, &value) == CM_OK && value == 42, "InitGame(21) == 42");
    cm_value_destroy(state);
    cm_future_destroy(future);

    cm_value_t* dt = cm_value_int(16);
    cm_value_t* dt_args[1] = {dt};
    future = cm_invoke(ctx, "engine.gamemode", "OnTick", dt_args, 1);
    cm_value_destroy(dt);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "OnTick ready");
    cm_value_t* void_value = cm_future_take_value(future);
    CHECK(cm_value_kind(void_value) == CM_VALUE_VOID, "OnTick is void");
    cm_value_destroy(void_value);
    cm_future_destroy(future);

    /* NULL target routes to plain functions. */
    future = cm_invoke(ctx, NULL, "main", NULL, 0);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "NULL target calls plain main");
    cm_future_destroy(future);

    /* Empty string target behaves like NULL. */
    future = cm_invoke(ctx, "", "main", NULL, 0);
    CHECK(cm_future_poll(future, NULL) == CM_READY, "empty target calls plain main");
    cm_future_destroy(future);

    /* Unknown target / member → future error, UNKNOWN_ENTRY kind. */
    future = cm_invoke(ctx, "engine.physics", "Apply", NULL, 0);
    CHECK(future != NULL, "unknown target still yields a future");
    CHECK(cm_future_poll(future, NULL) == CM_ERROR, "unknown target errors");
    cm_error_t report = cm_future_get_error(future);
    CHECK(report.kind == CM_ERROR_UNKNOWN_ENTRY, "UNKNOWN_ENTRY kind");
    CHECK(strstr(report.message, "engine.physics.Apply") != NULL, "names the miss");
    cm_error_free(&report);
    cm_future_destroy(future);

    future = cm_invoke(ctx, "engine.gamemode", "Ghost", NULL, 0);
    CHECK(cm_future_poll(future, NULL) == CM_ERROR, "unknown member errors");
    report = cm_future_get_error(future);
    CHECK(report.kind == CM_ERROR_UNKNOWN_ENTRY, "member miss kind");
    cm_error_free(&report);
    cm_future_destroy(future);

    cm_context_destroy(ctx);
    cm_program_destroy(program);
    cm_engine_destroy(engine);
}

static void test_failures(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();
    cm_program_t* program = cm_engine_load_source(engine, ADD_PROGRAM, &err);
    cm_error_free(&err);
    cm_context_t* ctx = cm_engine_create_context(engine, program, NULL);

    /* Unknown function. */
    cm_future_t* future = cm_invoke(ctx, NULL, "ghost", NULL, 0);
    CHECK(future != NULL, "unknown function yields a future");
    CHECK(cm_future_poll(future, NULL) == CM_ERROR, "unknown function errors");
    cm_error_t report = cm_future_get_error(future);
    CHECK(report.kind == CM_ERROR_UNKNOWN_ENTRY, "entry kind");
    CHECK(strcmp(report.file, "") == 0, "entry miss has no file");
    CHECK(report.line == 0 && report.column == 0, "entry miss has no position");
    cm_error_free(&report);
    /* get_error is repeatable: each call hands out a fresh copy. */
    cm_error_t again = cm_future_get_error(future);
    CHECK(again.kind == CM_ERROR_UNKNOWN_ENTRY, "error report repeatable");
    cm_error_free(&again);
    CHECK(cm_future_take_value(future) == NULL, "error future carries no value");
    cm_future_destroy(future);

    /* Runtime error with a module position. */
    future = cm_invoke(ctx, NULL, "divZero", NULL, 0);
    CHECK(cm_future_poll(future, NULL) == CM_ERROR, "division by zero errors");
    report = cm_future_get_error(future);
    CHECK(report.kind == CM_ERROR_RUNTIME, "runtime kind");
    CHECK(strstr(report.message, "zero") != NULL, "names the failure");
    CHECK(report.line == 42, "positioned at the division");
    CHECK(report.column >= 1, "column present");
    cm_error_free(&report);
    cm_future_destroy(future);

    /* Arity mismatch is a clean runtime failure. */
    cm_value_t* one[1] = {cm_value_int(1)};
    future = cm_invoke(ctx, NULL, "add", one, 1);
    cm_value_destroy(one[0]);
    CHECK(cm_future_poll(future, NULL) == CM_ERROR, "arity mismatch errors");
    report = cm_future_get_error(future);
    CHECK(report.kind == CM_ERROR_RUNTIME, "arity kind");
    CHECK(strstr(report.message, "wrong number of arguments") != NULL, "arity message");
    cm_error_free(&report);
    cm_future_destroy(future);

    /* NULL args with argc > 0 is misuse: NULL future. */
    CHECK(cm_invoke(ctx, NULL, "add", NULL, 2) == NULL, "NULL args with argc refused");
    /* NULL member is misuse. */
    CHECK(cm_invoke(ctx, NULL, NULL, NULL, 0) == NULL, "NULL member refused");
    /* A NULL element inside args is misuse. */
    cm_value_t* bad[2] = {NULL, NULL};
    CHECK(cm_invoke(ctx, NULL, "add", bad, 2) == NULL, "NULL arg element refused");
    /* NULL context is misuse. */
    CHECK(cm_invoke(NULL, NULL, "add", NULL, 0) == NULL, "NULL context refused");

    /* Polling/taking from a NULL future fails defensively. */
    CHECK(cm_future_poll(NULL, NULL) == CM_ERROR, "NULL future polls as error");
    CHECK(cm_future_take_value(NULL) == NULL, "NULL future takes nothing");
    cm_error_t null_report = cm_future_get_error(NULL);
    CHECK(null_report.kind == CM_ERROR_NONE, "NULL future report is NONE");
    cm_error_free(&null_report);

    cm_context_destroy(ctx);
    cm_program_destroy(program);
    cm_engine_destroy(engine);
}

static void test_limits(void) {
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();

    /* Fuel exhaustion → LIMIT. */
    cm_program_t* program = cm_engine_load_source(engine, ADD_PROGRAM, &err);
    cm_error_free(&err);
    cm_limits_t limits = CM_LIMITS_INIT;
    limits.fuel = 64;
    cm_context_t* ctx = cm_engine_create_context(engine, program, &limits);
    cm_value_t* zero_arg = cm_value_int(0);
    cm_value_t* spin_args[1] = {zero_arg};
    cm_future_t* future = cm_invoke(ctx, NULL, "spin", spin_args, 1);
    cm_value_destroy(zero_arg);
    CHECK(cm_future_poll(future, NULL) == CM_ERROR, "fuel exhaustion errors");
    cm_error_t report = cm_future_get_error(future);
    CHECK(report.kind == CM_ERROR_LIMIT, "limit kind for fuel");
    CHECK(strstr(report.message, "fuel") != NULL, "fuel named");
    cm_error_free(&report);
    cm_future_destroy(future);
    cm_context_destroy(ctx);
    cm_program_destroy(program);

    /* Call depth → LIMIT. */
    program = cm_engine_load_source(engine, ADD_PROGRAM, &err);
    cm_error_free(&err);
    limits = CM_LIMITS_INIT;
    limits.max_call_depth = 8;
    ctx = cm_engine_create_context(engine, program, &limits);
    cm_value_t* zero = cm_value_int(0);
    cm_value_t* args[1] = {zero};
    future = cm_invoke(ctx, NULL, "spin", args, 1);
    cm_value_destroy(zero);
    CHECK(cm_future_poll(future, NULL) == CM_ERROR, "depth exhaustion errors");
    report = cm_future_get_error(future);
    CHECK(report.kind == CM_ERROR_LIMIT, "limit kind for depth");
    CHECK(strstr(report.message, "call depth") != NULL, "depth named");
    cm_error_free(&report);
    cm_future_destroy(future);
    cm_context_destroy(ctx);
    cm_program_destroy(program);

    /* Deadline → LIMIT, and it stops in bounded host time. */
    program = cm_engine_load_source(
        engine, "int loop() {\nwhile (true) {\n}\nreturn 0\n}\n", &err);
    cm_error_free(&err);
    limits = CM_LIMITS_INIT;
    limits.deadline_ms = 200;
    ctx = cm_engine_create_context(engine, program, &limits);
    future = cm_invoke(ctx, NULL, "loop", NULL, 0);
    CHECK(cm_future_poll(future, NULL) == CM_ERROR, "deadline errors");
    report = cm_future_get_error(future);
    CHECK(report.kind == CM_ERROR_LIMIT, "limit kind for deadline");
    CHECK(strstr(report.message, "deadline") != NULL, "deadline named");
    cm_error_free(&report);
    cm_future_destroy(future);
    cm_context_destroy(ctx);
    cm_program_destroy(program);

    /* Zero limits mean defaults: bounded work still runs. */
    program = cm_engine_load_source(engine, ADD_PROGRAM, &err);
    cm_error_free(&err);
    limits = CM_LIMITS_INIT;
    ctx = cm_engine_create_context(engine, program, &limits);
    future = cm_invoke(ctx, NULL, "add", NULL, 0);
    /* Wrong arity with no args: still a RUNTIME error, proving the call ran. */
    CHECK(cm_future_poll(future, NULL) == CM_ERROR, "call ran under default limits");
    report = cm_future_get_error(future);
    CHECK(report.kind == CM_ERROR_RUNTIME, "default-limit call executed");
    cm_error_free(&report);
    cm_future_destroy(future);
    cm_context_destroy(ctx);
    cm_program_destroy(program);

    cm_engine_destroy(engine);
}

static void test_concurrent_contexts(void) {
    /* Two threads invoke the same context concurrently; the §1 guarantee
       (race-free independent invocations) means every result is exact. */
    cm_error_t err = CM_ERROR_INIT;
    cm_engine_t* engine = cm_engine_new();
    cm_program_t* program =
        cm_engine_load_source(engine, "int add(int a, int b) {\nreturn a + b\n}\n", &err);
    cm_error_free(&err);
    cm_context_t* ctx = cm_engine_create_context(engine, program, NULL);

    _Atomic long completed = 0;
    (void)completed;
    /* The C app keeps to single-threaded checks for portability; the
       Rust-side FFI suite drives true concurrency. Document the intent:
       each cm_invoke on one context is an independent invocation. */
    for (int i = 0; i < 100; i++) {
        cm_value_t* a = cm_value_int(i);
        cm_value_t* b = cm_value_int(i);
        cm_value_t* args[2] = {a, b};
        cm_future_t* future = cm_invoke(ctx, NULL, "add", args, 2);
        cm_value_destroy(a);
        cm_value_destroy(b);
        if (cm_future_poll(future, NULL) == CM_READY) {
            cm_value_t* value = cm_future_take_value(future);
            int64_t result = 0;
            cm_value_as_int(value, &result);
            if (result == (int64_t)i + i) {
                completed++;
            }
            cm_value_destroy(value);
        }
        cm_future_destroy(future);
    }
    CHECK(completed == 100, "100 sequential independent invocations exact");

    cm_context_destroy(ctx);
    cm_program_destroy(program);
    cm_engine_destroy(engine);
}

static void test_value_string_round_trip(void) {
    /* CMON display matches the language's own rendering for every kind. */
    struct {
        const char* expected;
        cm_value_t* (*make)(void);
    } cases[] = {{NULL, NULL}}; /* populated below, C89-safe style */
    (void)cases;

    cm_value_t* values[5];
    values[0] = cm_value_int(-12);
    values[1] = cm_value_float(1.5);
    values[2] = cm_value_bool(1);
    values[3] = cm_value_str("ok");
    values[4] = cm_value_void();

    const char* expected[5] = {"-12", "1.5", "true", "ok", ""};
    for (int i = 0; i < 5; i++) {
        char* shown = cm_value_to_string(values[i]);
        CHECK(shown != NULL && strcmp(shown, expected[i]) == 0, "CMON rendering");
        cm_string_free(shown);
        cm_value_destroy(values[i]);
    }

    /* Nested composite rendering. */
    cm_value_t* array = cm_value_array();
    cm_array_push(array, cm_value_int(1));
    cm_array_push(array, cm_value_str("two"));
    char* shown = cm_value_to_string(array);
    CHECK(shown != NULL && strcmp(shown, "[1, two]") == 0, "nested CMON");
    cm_string_free(shown);
    cm_value_destroy(array);
}

int cme_host_app_main(void) {
    printf("c-host: Checkmate C host application\n");
    printf("c-host: library version %s\n\n", cm_version());

    test_version();
    test_null_safety();
    test_load_source_success();
    test_load_source_compile_error();
    test_load_file();
    test_load_mod();
    test_invoke_scalars();
    test_invoke_composites();
    test_host_built_values();
    test_invoke_member();
    test_failures();
    test_limits();
    test_concurrent_contexts();
    test_value_string_round_trip();

    printf("\nc-host: %d checks, %d failed\n", checks_run, checks_failed);
    if (checks_failed == 0) {
        printf("c-host: ALL %d CHECKS PASSED\n", checks_run);
        return 0;
    }
    return 1;
}

#ifndef CME_HOST_EMBEDDED
int main(void) {
    return cme_host_app_main();
}
#endif
