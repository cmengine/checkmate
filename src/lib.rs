#[cfg(feature = "api")]
pub use cme_api as api;

// WHITEPAPER §13.1 spells the embedding surface `use cme::{Engine,
// ExecutionLimits}` — the flat re-exports keep that literal shape, with
// `Value` as the argument/result currency.
#[cfg(feature = "api")]
pub use cme_api::{CompiledProgram, Context, Engine, ExecutionLimits, Value};

#[cfg(feature = "compiler")]
pub use cme_compiler as compiler;

#[cfg(feature = "core")]
pub use cme_core as lang;

#[cfg(feature = "interp")]
pub use cme_interp as interp;

#[cfg(feature = "runtime")]
pub use cme_runtime as runtime;
