//! The megaprogram subsystem (WHITEPAPER §8): scanning, pattern matching,
//! template elaboration, and source-level expansion.
//!
//! Owner architecture note: the main lexer/parser/AST only detect magic
//! declarations and usages; this subsystem expands magic calls into normal
//! Checkmate SOURCE TEXT (not AST) and hands the result back to the main
//! compiler, which re-lexes, parses, checks, and runs it like hand-written
//! code. That text-level contract is what makes `cme expand` possible.

pub mod cteval;
pub mod ctxexpr;
pub mod expand;
pub mod matcher;
pub mod pattern;
pub mod profile;
pub mod scan;
pub mod template;
