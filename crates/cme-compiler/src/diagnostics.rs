//! The diagnostics model shared by every compiler stage.

use crate::lexer::LexError;
use cme_core::Span;
use std::fmt;

/// What stage produced a diagnostic.
#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// A lexer failure, with its specific shape.
    Lex(LexError),
    /// A parse-stage failure (free-form message).
    Parse,
}

/// A single diagnostic: what went wrong, where, and from which stage.
#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub struct Diagnostic {
    kind: DiagnosticKind,
    message: String,
    span: Span,
}

impl Diagnostic {
    /// Wraps a lexer error; message and span come from the error itself.
    pub fn lex(error: LexError) -> Self {
        Self {
            span: error.span(),
            message: error.to_string(),
            kind: DiagnosticKind::Lex(error),
        }
    }

    /// A parse-stage diagnostic pointing at `span`.
    pub fn parse(message: impl Into<String>, span: Span) -> Self {
        Self {
            kind: DiagnosticKind::Parse,
            message: message.into(),
            span,
        }
    }

    pub fn kind(&self) -> &DiagnosticKind {
        &self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn span(&self) -> Span {
        self.span
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
