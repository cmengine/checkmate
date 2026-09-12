//! The Rust host API for Checkmate (WHITEPAPER §13.1): load a program,
//! create an execution context under §5.5 limits, and invoke entry points.
//!
//! ```no_run
//! use cme_api::{Engine, ExecutionLimits, Value};
//!
//! let engine = Engine::new();
//! let program = engine.load_source("int main() {\nreturn 40 + 2\n}\n")?;
//!
//! // §5.5: the host owns every execution constraint of the invocation.
//! let limits = ExecutionLimits {
//!     fuel: Some(1_000_000),
//!     deadline_ms: Some(50),
//!     max_call_depth: 64,
//! };
//! let context = engine.create_context(&program, limits);
//!
//! let answer = context.invoke("main", &[])?;
//! assert_eq!(answer, Value::Int(42));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # The pipeline behind a [`CompiledProgram`]
//!
//! Loading runs the same gate the CLI applies before `run`: megaprograms
//! expand first (§8, only when the source mentions the subsystem), then
//! `parse_source` + the §10 standalone-import check + the type checker run
//! over the result. A program that produced ANY diagnostic never reaches a
//! [`Context`] — the [`Engine`] refuses to hand one out, so every
//! [`Context::invoke`] executes code the checker accepted.
//!
//! `Engine::load_mod` compiles a whole §10 mod tree (`mod.toml` + `src/`):
//! each module's megaprograms expand per file, the modules link into one
//! virtual program, and compile diagnostics re-anchor to the owning module
//! so error text reads `my_mod/src/main.cm:3:5: ...` instead of exposing
//! virtual-text coordinates.
//!
//! # What hosts can invoke
//!
//! Checkmate has no implicit entry point (§2.1): the host targets specific
//! functions. A [`Context`] invokes
//!
//! - top-level functions: `context.invoke("fib", &[Value::Int(10)])`, and
//! - §10.4 impl members — the "interface functions" a mod implements:
//!   `context.invoke_member("engine.gamemode", "OnTick", &args)`.
//!
//! Arguments and results are [`Value`]s (re-exported from the interpreter);
//! `From` impls build scalars from plain Rust data and `as_int`-style
//! accessors unpack results without pattern-matching.
//!
//! # Whitepaper alignment notes
//!
//! §13.1's example routes calls through schema-generated trait objects and
//! awaits them (`gamemode.init_game(config).await`). Those pieces are the
//! schema system's (§9) and the async VM's (§4, §5.2) — not yet
//! implemented, per the repository state notes. Until they land, this API
//! exposes the same load → limit → invoke shape over the shipped front end
//! and tree-walking interpreter, and impl-member invocation covers the
//! `cm_invoke(ctx, "engine.gamemode", "OnTick", …)` surface of the C API
//! (§13.2). Every invocation completes synchronously; §5.7 reentrancy rules
//! become observable once host capabilities can call back into scripts.
//!
//! # Threads
//!
//! [`Engine`] and [`CompiledProgram`] are immutable and shareable
//! (`Send + Sync`). A [`Context`] is `Send + Sync` too: invocations share
//! nothing mutable — each creates its own fuel cell and interpreter frame
//! — so independent invocations race with nothing (§1: concurrent
//! execution across invocations is inherently race-free). One context used
//! from several threads runs those invocations concurrently by design.

pub use cme_compiler::Diagnostic;
pub use cme_compiler::mods::{LoadedModule, ModManifest};
/// The §9 schema surface, re-exported so hosts and generated bindings
/// never need `cme-compiler` directly: schema types carry the contract,
/// [`SchemaContext`] is the active grant a load is checked against, and
/// [`parse_schema_file`] parses `.cm` schema files.
pub use cme_compiler::schema::{
    SchemaContext, SchemaFile, SchemaIssue, SchemaParseOutcome, SchemaSet, Version,
    parse_schema_file,
};
pub use cme_core::Span;
/// The AST pieces schema declarations are built from — generated bindings
/// (§9.6) construct [`SchemaFile`] descriptors with them.
pub use cme_core::ast::{FieldDef, Param, PrimitiveType, Type, VariantDecl};
pub use cme_core::schema::{
    ContractKind, MemberRequirement, RequiresPath, SchemaContract, SchemaEnum, SchemaItem,
    SchemaMember, SchemaStruct, is_camel_case, is_pascal_case,
};
pub use cme_interp::{CapabilityHost, InterpErrorKind, MAX_CALL_DEPTH, Value};

mod context;
mod engine;
mod render;

pub use context::{Context, ErrorKind, ExecutionError, ExecutionLimits};
pub use engine::{
    CapabilityProvider, CompileError, CompiledProgram, Engine, LoadError, ProgramKind, SchemaError,
};

#[cfg(test)]
mod tests {
    use super::{Engine, ErrorKind, ExecutionLimits, MAX_CALL_DEPTH, Value};

    #[test]
    fn the_facade_surface_composes_end_to_end() {
        let engine = Engine::new();
        let program = engine
            .load_source("int twice(int v) {\nreturn v * 2\n}\n")
            .expect("clean source compiles");
        let context = engine.create_context(&program, ExecutionLimits::default());
        assert_eq!(
            context.invoke("twice", &[Value::Int(21)]),
            Ok(Value::Int(42))
        );
        assert_eq!(
            context.invoke("nope", &[]).unwrap_err().kind,
            ErrorKind::UnknownEntry
        );
        assert_eq!(ExecutionLimits::default().max_call_depth, MAX_CALL_DEPTH);
    }
}
