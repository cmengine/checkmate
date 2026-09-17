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
//!
//! [`cme_schema_setup!`](crate::cme_schema_setup) is the one-call layer
//! over this macro for newcomers: it emits the same bindings plus the
//! engine/program/context/proxy glue, so the whole host bootstrap is a
//! single invocation.

use proc_macro::TokenStream;
use std::path::PathBuf;

mod codegen;
mod input;
mod setup;

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

    let schema = match load_schema("cme_schema_bindings!", &settings.path) {
        Ok(schema) => schema,
        Err(error) => return error.into_compile_error(),
    };

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

/// Generates the whole quick-start host from one invocation: the same
/// bindings [`cme_schema_bindings!`] produces for every `schema =` file,
/// plus the engine → program → context → proxy glue around them.
///
/// Inputs:
///
/// ```ignore
/// cme_schema_setup! {
///     schema = "schemas/origout.cm",        // repeatable; one module per namespace
///     program = mod "checkmate",            // mod "dir" | file "x.cm" | source "text"
///     crate = ::cme::api,                   // optional, default ::cme::api
///     limits = { fuel: 1_000_000, deadline_ms: 50, max_call_depth: 64 }, // optional
///     proxy = OrigoutTestProxy,             // optional, repeatable; `as name` renames
///     provider = window => MyService,       // optional, repeatable; wrapped in Arc::new
/// }
/// ```
///
/// The expansion defines, at the invocation site: one bindings module per
/// schema namespace (identical to [`cme_schema_bindings!`] output), a
/// `Host` struct with `new`/`engine`/`program`/`limits`/`context`/`run`/
/// `try_run`, the `HostError` failure modes, and a `Session` struct
/// carrying the context plus every requested proxy. The manual flow stays
/// the advanced-user surface; this macro only automates it.
#[proc_macro]
pub fn cme_schema_setup(input: TokenStream) -> TokenStream {
    let settings = match setup::parse_settings(input) {
        Ok(settings) => settings,
        Err(error) => return error.into_compile_error(),
    };

    let mut schemas = Vec::new();
    for path in &settings.schemas {
        match load_schema("cme_schema_setup!", path) {
            Ok(schema) => schemas.push(schema),
            Err(error) => return error.into_compile_error(),
        }
    }

    match setup::generate(&settings, &schemas) {
        Ok(code) => code.parse().expect("generated host glue is valid Rust"),
        Err(errors) => {
            let listed: String = errors
                .iter()
                .map(|error| format!("\n  - {}", error.message))
                .collect();
            input::CompileErrorMessage::new(format!("cme_schema_setup!:{listed}"))
                .into_compile_error()
        }
    }
}

/// Resolves `path` the way `include_str!` does (relative to the calling
/// crate's manifest directory), reads it, and runs the real schema front
/// end. A missing or unreadable file and any parse diagnostic become a
/// deferred `compile_error!` naming the macro that asked.
fn load_schema(
    macro_name: &str,
    path: &PathBuf,
) -> Result<cme_core::schema::SchemaFile, input::CompileErrorMessage> {
    // Relative paths resolve against the CALLING crate's manifest
    // directory — cargo sets CARGO_MANIFEST_DIR for the crate being
    // compiled, which is exactly the include_str! rule.
    let absolute = if path.is_absolute() {
        path.clone()
    } else {
        match std::env::var("CARGO_MANIFEST_DIR") {
            Ok(manifest_dir) => PathBuf::from(manifest_dir).join(path),
            Err(_) => path.clone(),
        }
    };

    let text = std::fs::read_to_string(&absolute).map_err(|error| {
        input::CompileErrorMessage::new(format!(
            "{macro_name}: cannot read schema file {}: {error}",
            absolute.display()
        ))
    })?;

    let outcome = cme_compiler::schema::parse_schema_file(&text);
    if !outcome.is_clean() {
        let listed: String = outcome
            .diagnostics
            .iter()
            .map(|diagnostic| format!("\n  - {}", diagnostic.message()))
            .collect();
        return Err(input::CompileErrorMessage::new(format!(
            "{macro_name}: schema file {} has {} defect(s):{listed}",
            path.display(),
            outcome.diagnostics.len()
        )));
    }
    Ok(outcome.file.expect("a clean parse yields the file"))
}
