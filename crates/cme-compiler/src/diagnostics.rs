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
    /// A type-check-stage failure (free-form message).
    Type,
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

    /// A type-check-stage diagnostic pointing at `span`.
    pub fn type_error(message: impl Into<String>, span: Span) -> Self {
        Self {
            kind: DiagnosticKind::Type,
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

/// The result of a tolerant parse: the recovered AST plus every diagnostic.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ParseOutcome {
    pub statements: Vec<cme_core::ast::Stmt>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParseOutcome {
    pub fn is_clean(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn error(&self, id: cme_core::ast::ErrorId) -> Option<&Diagnostic> {
        self.diagnostics.get(id.0)
    }
}
