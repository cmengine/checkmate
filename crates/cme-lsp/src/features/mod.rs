//! One module per LSP capability. Every feature is a pure function over
//! the salsa database (for document state) and the [`crate::analysis`]
//! symbol table (for resolution), so the server backend only routes
//! requests and publishes results.

pub mod completion;
pub mod definition;
pub mod diagnostics;
pub mod hover;
pub mod mega_completion;
pub mod schema_docs;
pub mod symbols;
pub mod tokens;
