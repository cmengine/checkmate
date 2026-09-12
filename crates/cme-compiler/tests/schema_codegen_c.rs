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
schema engine v1.4.0

struct TextureHandle {
    int id
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
    assert!(header.contains("static inline cm_value_t* cme_engine_TextureHandle_pack("));
    assert!(header.contains("cm_value_struct_field(value, \"id\")"));
    // Str fields are documented as riding the string accessor.
    assert!(header.contains("cm_value_as_int(f, &out->id)"));
}

#[test]
fn the_generated_header_compiles_as_c11() {
    // Compiled where a C toolchain is available (the c_host build does the
    // same thing for real): a parse-level check via the compiler.
    let header = generate(ENGINE);
    let dir = std::env::temp_dir().join(format!("cme_codegen_c_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("engine_schema_gen.h");
    std::fs::write(&path, &header).expect("write header");

    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let include = concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/cme-ffi/include");
    let status = std::process::Command::new(&compiler)
        .args([
            "-std=c11",
            "-fsyntax-only",
            "-Wall",
            "-Wextra",
            "-I",
            include,
            "-I",
            path.parent().expect("parent").to_str().expect("utf8"),
            "-x",
            "c",
            "-",
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            let _ = child
                .stdin
                .as_mut()
                .expect("stdin")
                .write_all(b"#include \"engine_schema_gen.h\"\nint main(void){return 0;}\n");
            child.wait()
        });

    let _ = std::fs::remove_dir_all(&dir);
    match status {
        Ok(code) if code.success() => {}
        Ok(_) => panic!("the C compiler rejected the generated header:\n{header}"),
        // No C toolchain in the environment: the structural tests above
        // still pin the shape.
        Err(_) => {}
    }
}
