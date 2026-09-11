//! Build script: compiles the C host application (apps/c_host/main.c) into
//! this crate so the `cargo test` run exercises the real C consumer of the
//! ABI end to end — the same source file the standalone Makefile builds
//! against the shipped static library. The `CME_HOST_EMBEDDED` define swaps
//! the app's `main` for a plain entry function so it links harmlessly
//! alongside Rust's own test harness.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let c_app = manifest_dir
        .join("../..")
        .join("apps/c_host/main.c")
        .canonicalize()
        .expect("apps/c_host/main.c exists");
    let header_dir = manifest_dir.join("include");

    cc::Build::new()
        .file(&c_app)
        .include(&header_dir)
        .define("CME_HOST_EMBEDDED", None)
        .flag_if_supported("-std=c11")
        .warnings(true)
        .compile("cme_host_app");

    println!("cargo:rerun-if-changed={}", c_app.display());
    println!(
        "cargo:rerun-if-changed={}",
        header_dir.join("cme.h").display()
    );
}
