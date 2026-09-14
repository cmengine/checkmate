//! The C bindings generator (WHITEPAPER §9.6, C half): the generated
//! header's shape, its compile-time verification macros, and the
//! round-trip through the C compiler (where the workspace build compiles
//! the real output).

use cme_compiler::schema::{codegen_c, parse_schema_file};

fn generate(source: &str) -> String {
    let outcome = parse_schema_file(source);
    assert!(
        outcome.is_clean(),
        "test schema must parse cleanly: {:?}",
        outcome
            .diagnostics
            .iter()
            .map(|d| d.message().to_string())
            .collect::<Vec<_>>()
    );
    codegen_c(&outcome.file.expect("file"), "cme.h")
}

const ENGINE: &str = "
schema engine 1.4.0

struct TextureHandle {
    int id
}

struct Asset {
    str name
    int size
    TextureHandle handle
    str[] tags
}

enum LoadError {
    NotFound
    Corrupt(str reason)
    Stale(int generation)
}

capability graphics {
    since 1.0.0 TextureHandle LoadTexture(str path)
    since 1.0.0 void DrawTexture(TextureHandle tex, Vec2Like position)
    since 1.2.0 int DrawSprite(TextureHandle tex, Vec2Like position, int frame)
}

interface loader {
    since 1.0.0 bool IsAvailable(str name)
    since 1.4.0 optional void Invalidate(str token)
}
";

#[test]
fn the_header_carries_the_contract_metadata() {
    let header = generate(ENGINE);
    assert!(header.contains("#ifndef CME_SCHEMA_GEN_ENGINE_H"));
    assert!(header.contains("#include \"cme.h\""));
    assert!(header.contains("#define CME_SCHEMA_ENGINE_VERSION \"1.4.0\""));
    assert!(header.contains("#define CME_SCHEMA_ENGINE_VERSION_MINOR 4"));
    assert!(header.ends_with("#endif /* CME_SCHEMA_GEN_ENGINE_H */\n"));
}

#[test]
fn capability_members_get_verified_function_pointer_types() {
    let header = generate(ENGINE);
    // One function-pointer type per member, with the cm_value ABI shape.
    assert!(header.contains(
        "typedef cm_value_t* (*cme_engine_graphics_LoadTexture_fn)(void* user, cm_value_t* const* args, size_t argc, cm_error_t* out_error);"
    ));
    // The vtable struct gathers them.
    assert!(header.contains("typedef struct cme_engine_graphics_vtable {"));
    assert!(header.contains("    cme_engine_graphics_LoadTexture_fn LoadTexture;"));
    assert!(header.contains("    cme_engine_graphics_DrawSprite_fn DrawSprite;"));
}

#[test]
fn the_register_macro_static_asserts_every_signature() {
    let header = generate(ENGINE);
    // One _Generic static assert per member...
    let load_texture_assert = header
        .lines()
        .find(|line| line.contains("fn_LoadTexture), cme_engine_graphics_LoadTexture_fn: 1"))
        .expect("a _Generic assert for LoadTexture");
    assert!(load_texture_assert.contains("_Static_assert"));
    assert!(header.matches("_Static_assert").count() >= 3);
    // ...and a member table with the exact member count.
    assert!(header.contains("cme_members_, 3, (user)"));
    assert!(header.contains("{ \"LoadTexture\", fn_LoadTexture }, \\"));
    // The macro name follows the schema namespace + capability.
    assert!(header.contains("#define CME_ENGINE_GRAPHICS_REGISTER(engine, user"));
}

#[test]
fn interface_helpers_have_exact_arity_and_optional_flags() {
    let header = generate(ENGINE);
    // The required member's helper takes exactly one value argument.
    assert!(header.contains(
        "static inline cm_value_t* cme_engine_loader_IsAvailable_invoke(\n    cm_context_t* ctx, cm_value_t* arg0, cm_error_t* out_error)"
    ));
    assert!(header.contains("cm_invoke(ctx, \"engine.loader\", \"IsAvailable\","));
    // The optional member carries its metadata defines.
    assert!(header.contains("#define CME_ENGINE_LOADER_Invalidate_OPTIONAL 1"));
    assert!(header.contains("#define CME_ENGINE_LOADER_IsAvailable_OPTIONAL 0"));
}

#[test]
fn schema_types_get_pack_and_unpack_helpers() {
    let header = generate(ENGINE);
    assert!(header.contains("typedef struct cme_engine_TextureHandle {"));
    assert!(header.contains("int64_t id;"));
    // Every pack takes the C value BY ADDRESS, so nesting composes.
    assert!(header.contains(
        "static inline cm_value_t* cme_engine_TextureHandle_pack(const cme_engine_TextureHandle* value)"
    ));
    assert!(header.contains("cm_value_struct_field(value, \"id\")"));
    assert!(header.contains("cm_value_as_int(f, &out->id)"));
}

#[test]
fn str_fields_unpack_to_owned_copies_and_pack_from_plain_chars() {
    let header = generate(ENGINE);
    // The struct carries the field (it used to be silently dropped).
    assert!(header.contains("char* name; /* owned after unpack: free with cm_string_free */"));
    // Unpack reads an owned copy; pack reads any NUL-terminated string.
    assert!(header.contains("cm_value_as_str(f, &out->name, NULL)"));
    assert!(header.contains("cm_value_str(value->name)"));
    // The nested schema type recurses through the generated helpers.
    assert!(header.contains("cme_engine_TextureHandle_unpack(f, &out->handle)"));
    assert!(header.contains("cme_engine_TextureHandle_pack(&value->handle)"));
    // Container fields stay on the generic accessor layer, by design.
    assert!(header.contains("`tags` uses the generic cm_value accessors (container shape)"));
}

#[test]
fn enums_render_a_real_tagged_union_with_payloads() {
    let header = generate(ENGINE);
    // The tag enum and the union carry per-variant payload members.
    assert!(header.contains("CME_engine_LoadError_NotFound = 0,"));
    assert!(header.contains("char* reason; /* owned after unpack: free with cm_string_free */"));
    assert!(header.contains("int64_t generation;"));
    // Unpack dispatches on the variant name and frees the copy on every path.
    assert!(header.contains("if (strcmp(variant_name, \"Corrupt\") == 0) {"));
    assert!(header.contains("cm_value_enum_payload(value, 0)"));
    assert!(header.contains("cm_string_free(variant_name)"));
    // Pack pushes typed payloads; the str payload copies through cm_value_str.
    assert!(header.contains("cm_value_t* e = cm_value_enum(\"LoadError\", \"Stale\")"));
    assert!(header.contains("cm_value_int(value->as.Stale.generation)"));
}

#[test]
fn the_future_helper_is_guarded_for_multi_header_includes() {
    let header = generate(ENGINE);
    assert!(header.contains("#ifndef CME_SCHEMA_GEN_FUTURE_TAKE_SYNC"));
    assert!(header.contains("#define CME_SCHEMA_GEN_FUTURE_TAKE_SYNC"));
    assert!(header.contains("#endif /* CME_SCHEMA_GEN_FUTURE_TAKE_SYNC */"));
    // Exactly one definition per header.
    assert_eq!(
        header
            .matches("static inline cm_value_t* cme_future_take_sync")
            .count(),
        1
    );
}

/// Uniqueness for the compile tests' temp dirs (they run in parallel).
static COMPILE_DIR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Compiles `source` (a full C translation unit) with `-fsyntax-only`
/// against the FFI headers plus `headers` (name → text, written into a
/// temp include dir). Returns `Ok(())` when the unit compiles, `Err(true)`
/// when the compiler rejected it, `Err(false)` when no C toolchain is
/// available (environments without `cc` skip the compile-level checks).
fn compile_unit(source: &str, headers: &[(&str, String)]) -> Result<(), bool> {
    let dir = std::env::temp_dir().join(format!(
        "cme_codegen_c_{}_{}",
        std::process::id(),
        COMPILE_DIR.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (name, text) in headers {
        std::fs::write(dir.join(name), text).expect("write header");
    }

    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let include = concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/cme-ffi/include");
    let outcome = std::process::Command::new(&compiler)
        .args([
            "-std=c11",
            "-fsyntax-only",
            "-Wall",
            "-Wextra",
            "-I",
            include,
            "-I",
            dir.to_str().expect("utf8 temp dir"),
            "-x",
            "c",
            "-",
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .as_mut()
                .expect("stdin")
                .write_all(source.as_bytes())?;
            child.wait()
        });
    let _ = std::fs::remove_dir_all(&dir);
    match outcome {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(true),
        Err(_) => Err(false),
    }
}

#[test]
fn the_generated_header_compiles_as_c11() {
    let header = generate(ENGINE);
    if let Err(true) = compile_unit(
        "#include \"engine_schema_gen.h\"\nint main(void){return 0;}\n",
        &[("engine_schema_gen.h", header.clone())],
    ) {
        panic!("the C compiler rejected the generated header:\n{header}");
    }
}

/// The positive half of the C compile-time story: a consumer that
/// implements every capability member with the exact schema signature,
/// registers through the _Generic-verified macro, builds and reads the
/// generated structs/enums, and calls interface helpers at their exact
/// arity — all of it must compile.
#[test]
fn a_correct_consumer_compiles() {
    let header = generate(ENGINE);
    let consumer = r#"
#include "engine_schema_gen.h"

static cm_value_t* host_LoadTexture(void* user, cm_value_t* const* args, size_t argc, cm_error_t* out_error) {
    (void)user; (void)args; (void)argc; (void)out_error;
    cme_engine_TextureHandle tex = { 42 };
    return cme_engine_TextureHandle_pack(&tex);
}
static cm_value_t* host_DrawTexture(void* user, cm_value_t* const* args, size_t argc, cm_error_t* out_error) {
    (void)user; (void)args; (void)argc; (void)out_error;
    return cm_value_void();
}
static cm_value_t* host_DrawSprite(void* user, cm_value_t* const* args, size_t argc, cm_error_t* out_error) {
    (void)user; (void)args; (void)argc; (void)out_error;
    return cm_value_int(7);
}

static int registered(void) {
    cm_engine_t* engine = (cm_engine_t*)0;
    CME_ENGINE_GRAPHICS_REGISTER(engine, NULL, host_LoadTexture, host_DrawTexture, host_DrawSprite);
    return 1;
}

static int typed_data(void) {
    /* Struct pack/unpack round trip shape, with a str and a nested type. */
    cme_engine_Asset asset = { 0 };
    asset.size = 3;
    asset.handle.id = 9;
    cm_value_t* value = cme_engine_Asset_pack(&asset);
    if (!value) return 0;
    cme_engine_Asset back = { 0 };
    cm_status_t st = cme_engine_Asset_unpack(value, &back);
    cm_value_destroy(value);
    if (st != CM_OK || back.size != 3 || back.handle.id != 9) return 0;

    /* Enum pack/unpack: bare and payload variants. */
    cme_engine_LoadError err = { 0 };
    err.variant = CME_engine_LoadError_Stale;
    err.as.Stale.generation = 4;
    cm_value_t* eval = cme_engine_LoadError_pack(&err);
    if (!eval) return 0;
    cme_engine_LoadError eback = { 0 };
    st = cme_engine_LoadError_unpack(eval, &eback);
    cm_value_destroy(eval);
    if (st != CM_OK || eback.variant != CME_engine_LoadError_Stale) return 0;
    return 1;
}

static int typed_interface_calls(cm_context_t* ctx) {
    cm_error_t err = CM_ERROR_INIT;
    /* Exact arity: these helpers take exactly the schema's parameter list. */
    cm_value_t* name = cm_value_str("hero.png");
    cm_value_t* ok = cme_engine_loader_IsAvailable_invoke(ctx, name, &err);
    cm_value_destroy(ok);
    cm_value_destroy(name);
    /* The optional member is invocable too. */
    cm_value_t* token = cm_value_str("t");
    cm_value_t* void_result = cme_engine_loader_Invalidate_invoke(ctx, token, &err);
    cm_value_destroy(void_result);
    cm_value_destroy(token);
    return 1;
}

int main(void) { return registered() && typed_data() ? 0 : 1; }
"#;
    if let Err(true) = compile_unit(consumer, &[("engine_schema_gen.h", header.clone())]) {
        panic!("a schema-conforming C consumer must compile:\n{header}");
    }
}

/// The negative half, rule 1: a provider whose SIGNATURE differs from the
/// schema (here: wrong return type) must FAIL the _Static_assert — the
/// compile-time verification the whole C generator exists for.
#[test]
fn a_wrong_signature_provider_fails_the_compile() {
    let header = generate(ENGINE);
    let consumer = r#"
#include "engine_schema_gen.h"
/* WRONG: returns int instead of cm_value_t*. */
static int host_LoadTexture(void* user, cm_value_t* const* args, size_t argc, cm_error_t* out_error) {
    (void)user; (void)args; (void)argc; (void)out_error; return 1;
}
static cm_value_t* host_DrawTexture(void* user, cm_value_t* const* args, size_t argc, cm_error_t* out_error) {
    (void)user; (void)args; (void)argc; (void)out_error; return cm_value_void();
}
static cm_value_t* host_DrawSprite(void* user, cm_value_t* const* args, size_t argc, cm_error_t* out_error) {
    (void)user; (void)args; (void)argc; (void)out_error; return cm_value_int(0);
}
int main(void) {
    cm_engine_t* engine = (cm_engine_t*)0;
    CME_ENGINE_GRAPHICS_REGISTER(engine, NULL, host_LoadTexture, host_DrawTexture, host_DrawSprite);
    return 0;
}
"#;
    match compile_unit(consumer, &[("engine_schema_gen.h", header.clone())]) {
        Ok(()) => panic!("a wrong-signature provider must NOT compile:\n{header}"),
        Err(true) => {}  // rejected: the verification works
        Err(false) => {} // no C toolchain; structural tests still pin the shape
    }
}

/// The negative half, rule 2: a MISSING member argument fails the
/// preprocessor (the macro's parameter count is the schema's member list).
#[test]
fn a_missing_member_fails_the_preprocessor() {
    let header = generate(ENGINE);
    let consumer = r#"
#include "engine_schema_gen.h"
static cm_value_t* host_LoadTexture(void* user, cm_value_t* const* args, size_t argc, cm_error_t* out_error) {
    (void)user; (void)args; (void)argc; (void)out_error; return cm_value_int(1);
}
static cm_value_t* host_DrawTexture(void* user, cm_value_t* const* args, size_t argc, cm_error_t* out_error) {
    (void)user; (void)args; (void)argc; (void)out_error; return cm_value_void();
}
/* host_DrawSprite is MISSING from the registration below. */
int main(void) {
    cm_engine_t* engine = (cm_engine_t*)0;
    CME_ENGINE_GRAPHICS_REGISTER(engine, NULL, host_LoadTexture, host_DrawTexture);
    return 0;
}
"#;
    match compile_unit(consumer, &[("engine_schema_gen.h", header.clone())]) {
        Ok(()) => panic!("a missing member must NOT compile:\n{header}"),
        Err(true) => {}
        Err(false) => {}
    }
}

/// §9.2: one header per namespace; two generated headers must coexist in
/// one translation unit (the guarded future helper proves it).
#[test]
fn two_generated_headers_coexist_in_one_translation_unit() {
    let engine = generate(ENGINE);
    const PHYSICS: &str = "
schema physics 2.0.0
struct Body {
    float mass
}
capability dynamics {
    since 2.0.0 void Apply(Body body)
}
";
    let physics = generate(PHYSICS);
    if let Err(true) = compile_unit(
        "#include \"engine_schema_gen.h\"\n#include \"physics_schema_gen.h\"\nint main(void){return 0;}\n",
        &[
            ("engine_schema_gen.h", engine.clone()),
            ("physics_schema_gen.h", physics.clone()),
        ],
    ) {
        panic!(
            "two generated headers must coexist:\n--- engine ---\n{engine}\n--- physics ---\n{physics}"
        );
    }
}
