//! `cme-lsp` implements the Checkmate language server. The binary surface is
//! the unified `cme` CLI (`cme lsp`); this crate owns the analysis pipeline
//! and the LSP backend.
//!
//! Architecture (WHITEPAPER §12, §14):
//!
//! - [`db`] — the salsa incremental database. Every opened document is a
//!   salsa input; parsing, megaprogram expansion, and checking are tracked
//!   queries, so an edit recomputes only what the edit invalidated and
//!   unchanged files reuse their memos.
//! - [`analysis`] — the symbol table and best-effort name/type resolution
//!   built on top of the recovered AST.
//! - [`convert`] — byte-offset spans ↔ LSP ranges through a line index.
//! - [`features`] — one module per LSP capability (diagnostics, hover,
//!   completion, symbols, definition, references, semantic tokens).
//! - [`server`] — the `tower-lsp-server` backend and the stdio entry point.

pub mod analysis;
pub mod convert;
pub mod db;
pub mod features;
pub mod resolve;
pub mod server;

/// Runs the language server over stdio. Blocks until the client disconnects.
pub fn run_stdio() -> Result<(), String> {
    server::run_stdio()
}
