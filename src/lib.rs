#[cfg(feature = "api")]
pub use cme_api as api;

// WHITEPAPER §13.1 spells the embedding surface `use cme::{Engine,
// ExecutionLimits}` — the flat re-exports keep that literal shape, with
// `Value` as the argument/result currency.
#[cfg(feature = "api")]
pub use cme_api::{
    CompiledProgram, Context, Engine, ExecutionLimits, InterfaceProxy, MAX_CALL_DEPTH, Value,
};

#[cfg(feature = "compiler")]
pub use cme_compiler as compiler;

// §9.6: the procedural macro that turns a .cm schema file into
// compile-time-verified host bindings (capability traits, interface
// proxies, and the runtime schema descriptor).
#[cfg(feature = "schema-macro")]
pub use cme_schema_macro::cme_schema_bindings;

#[cfg(feature = "core")]
pub use cme_core as lang;

#[cfg(feature = "interp")]
pub use cme_interp as interp;

#[cfg(feature = "runtime")]
pub use cme_runtime as runtime;
