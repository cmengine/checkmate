//! Build script: compiles the C host application (apps/c_host/main.c) into
//! this crate so the `cargo test` run exercises the real C consumer of the
//! ABI end to end — the same source file the standalone Makefile builds
//! against the shipped static library. The `CME_HOST_EMBEDDED` define swaps
//! the app's `main` for a plain entry function so it links harmlessly
//! alongside Rust's own test harness.
//!
//! The §9.6 half of the story happens HERE too: the build generates the C
//! schema header from `apps/c_host/engine.cm` through the real front end
//! (`cme-compiler`), and the consumer compiles against it with
//! `CME_HOST_HAS_SCHEMA_GEN` defined — so the REGISTER macro's
//! `_Static_assert`s and the typed pack/unpack helpers are exercised by
//! every `cargo test` run, exactly as a real host build would.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let c_app = manifest_dir
        .join("../..")
        .join("apps/c_host/main.c")
        .canonicalize()
        .expect("apps/c_host/main.c exists");
    let schema = manifest_dir
        .join("../..")
        .join("apps/c_host/engine.cm")
        .canonicalize()
        .expect("apps/c_host/engine.cm exists");
    let header_dir = manifest_dir.join("include");

    // The §9.6 schema header, generated from the same file the script
    // toolchain checks against. A defect in the schema fails the BUILD —
    // the C host can never compile against a broken contract.
    let schema_text = std::fs::read_to_string(&schema).expect("read engine.cm");
    let outcome = cme_compiler::schema::parse_schema_file(&schema_text);
    let diagnostics: Vec<String> = outcome
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message().to_string())
        .collect();
    assert!(
        outcome.is_clean(),
        "apps/c_host/engine.cm has schema defects: {diagnostics:?}"
    );
    let header = cme_compiler::schema::codegen_c(
        outcome.file.as_ref().expect("a clean parse yields the file"),
        "cme.h",
    );
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let generated = out_dir.join("chost_schema_gen.h");
    std::fs::write(&generated, header).expect("write generated schema header");

    cc::Build::new()
        .file(&c_app)
        .include(&header_dir)
        .include(&out_dir)
        .define("CME_HOST_EMBEDDED", None)
        .define("CME_HOST_HAS_SCHEMA_GEN", None)
        .flag_if_supported("-std=c11")
        .warnings(true)
        .compile("cme_host_app");

    println!("cargo:rerun-if-changed={}", c_app.display());
    println!("cargo:rerun-if-changed={}", schema.display());
    println!("cargo:rerun-if-changed={}", header_dir.join("cme.h").display());
    println!("cargo:rerun-if-changed={}", generated.display());
}
