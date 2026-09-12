//! `cme_schema_bindings!` — compile-time-verified Rust host bindings from
//! a `.cm` schema file (WHITEPAPER §9.6).
//!
//! The macro runs the REAL schema front end (`cme-compiler`'s
//! [`parse_schema_file`]) at the host's compile time and generates, from
//! the same single source of truth the script toolchain uses:
//!
//! - **Boundary types (§9.3)** — schema structs and enums as native Rust
//!   types with [`Value`](cme_api::Value) pack/unpack;
//! - **Capability traits (§9.1)** — one trait per capability whose methods
//!   carry the schema signatures. The host implements the trait, so a
//!   missing member, a wrong parameter type, or a wrong return type is a
//!   COMPILE error in host code (§9.6: "the host build pipeline translates
//!   schemas directly into native Rust traits and proxies");
//! - **Interface proxies (§9.6)** — typed, compile-time-checked handles
//!   for the host's calls INTO the script;
//! - **A schema descriptor** — the same contract as a runtime
//!   [`SchemaFile`](cme_api::SchemaFile) for [`Engine::register_schema`]
//!   (cme_api::Engine::register_schema), so script-side checking and
//!   host-side bindings provably come from one file.
//!
//! # Usage
//!
//! ```ignore
//! cme_schema_bindings!("schemas/engine.cm");
//! // or, when the host does not use the `cme_api` crate name:
//! cme_schema_bindings!(path = "schemas/engine.cm", crate = ::my_reexport::api);
//! ```
//!
//! The path resolves relative to the invoking crate's manifest directory
//! (the same rule `include_str!` follows). Schema defects and unresolved
//! type references become `compile_error!` diagnostics — a schema that
//! does not parse cannot produce bindings.

use proc_macro::TokenStream;
use std::path::PathBuf;

mod codegen;
mod input;

/// Generates host bindings from a `.cm` schema file.
///
/// Inputs: `"<path>"` (relative to the invoking crate's manifest dir), or
/// `path = "<path>", crate = <::path::ToApi>` to control the crate the
/// generated code references (default `::cme_api`).
#[proc_macro]
pub fn cme_schema_bindings(input: TokenStream) -> TokenStream {
    let settings = match input::parse_settings(input) {
        Ok(settings) => settings,
        Err(error) => return error.into_compile_error(),
    };

    // Relative paths resolve against the CALLING crate's manifest
    // directory — cargo sets CARGO_MANIFEST_DIR for the crate being
    // compiled, which is exactly the include_str! rule.
    let absolute = if settings.path.is_absolute() {
        settings.path.clone()
    } else {
        match std::env::var("CARGO_MANIFEST_DIR") {
            Ok(manifest_dir) => PathBuf::from(manifest_dir).join(&settings.path),
            Err(_) => settings.path.clone(),
        }
    };

    let text = match std::fs::read_to_string(&absolute) {
        Ok(text) => text,
        Err(error) => {
            return input::CompileErrorMessage::new(format!(
                "cme_schema_bindings!: cannot read schema file {}: {error}",
                absolute.display()
            ))
            .into_compile_error();
        }
    };

    let outcome = cme_compiler::schema::parse_schema_file(&text);
    if !outcome.is_clean() {
        let listed: String = outcome
            .diagnostics
            .iter()
            .map(|diagnostic| format!("\n  - {}", diagnostic.message()))
            .collect();
        return input::CompileErrorMessage::new(format!(
            "cme_schema_bindings!: schema file {} has {} defect(s):{listed}",
            settings.path.display(),
            outcome.diagnostics.len()
        ))
        .into_compile_error();
    }
    let schema = outcome.file.expect("a clean parse yields the file");

    match codegen::generate(&schema, &settings.api_crate) {
        Ok(code) => code.parse().expect("generated bindings are valid Rust"),
        Err(errors) => {
            let listed: String = errors
                .iter()
                .map(|error| format!("\n  - {}", error.message))
                .collect();
            input::CompileErrorMessage::new(format!(
                "cme_schema_bindings!: schema file {} cannot produce bindings:{listed}",
                settings.path.display()
            ))
            .into_compile_error()
        }
    }
}
