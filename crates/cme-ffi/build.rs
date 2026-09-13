//! Build script: compiles the C host application into this crate so the
//! `cargo test` run exercises the real C consumer of the ABI end to end —
//! the same source file the standalone Makefile builds against the shipped
//! static library. The `CME_HOST_EMBEDDED` define swaps the app's `main`
//! for a plain entry function so it links harmlessly alongside Rust's own
//! test harness.
//!
//! The §9.6 half of the story happens HERE too: the build generates the C
//! schema header from the host's `engine.cm` through the real front end
//! (`cme-compiler`), and the consumer compiles against it with
//! `CME_HOST_HAS_SCHEMA_GEN` defined — so the REGISTER macro's
//! `_Static_assert`s and the typed pack/unpack helpers are exercised by
//! every `cargo test` run, exactly as a real host build would.
//!
//! Source layout: in a repo checkout the build uses `apps/c_host/` (the
//! files the Makefile consumes). A published crate cannot see outside its
//! own directory, so byte-identical fallbacks live in `c_src/` and are
//! used when the `apps/` tree is absent. A drift test pins the copies
//! together; refresh `c_src/` whenever `apps/c_host/` changes.

use std::env;
use std::path::PathBuf;

fn resolve_host_source(manifest_dir: &std::path::Path, name: &str) -> PathBuf {
    let repo = manifest_dir.join("../../apps/c_host").join(name);
    if repo.is_file() {
        return repo.canonicalize().expect("repo host source resolves");
    }
    let vendored = manifest_dir.join("c_src").join(name);
    vendored
        .canonicalize()
        .expect("vendored host source exists")
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let c_app = resolve_host_source(&manifest_dir, "main.c");
    let schema = resolve_host_source(&manifest_dir, "engine.cm");
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
        outcome
            .file
            .as_ref()
            .expect("a clean parse yields the file"),
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
    println!(
        "cargo:rerun-if-changed={}",
        header_dir.join("cme.h").display()
    );
    println!("cargo:rerun-if-changed={}", generated.display());
}
