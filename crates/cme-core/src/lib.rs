pub mod ast {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Span {
        pub start: usize,
        pub end: usize,
    }

    impl Span {
        pub fn new(start: usize, end: usize) -> Self {
            Self { start, end }
        }

        /// A zero-width span marking a position where source text is missing.
        /// Used by error-tolerant parsing to plant "missing node" placeholders
        /// (for example, an initializer the user has not typed yet).
        pub fn missing(offset: usize) -> Self {
            Self {
                start: offset,
                end: offset,
            }
        }
    }

    /// A recorded syntax error attached to an `Invalid` AST node.
    ///
    /// `span` points at the precise error location (the offending token or the
    /// position where something was expected), while the enclosing `Invalid`
    /// node's own `span` covers the whole source region the parser skipped.
    /// The same diagnostics are also returned by the parser, which stays the
    /// canonical list for reporting; the copy here keeps each broken region
    /// self-describing as the tree is passed between components.
    #[derive(Debug, PartialEq, Eq, Clone)]
    pub struct SyntaxError {
        pub message: String,
        pub span: Span,
    }

    impl SyntaxError {
        pub fn new(message: impl Into<String>, span: Span) -> Self {
            Self {
                message: message.into(),
                span,
            }
        }
    }

    #[derive(Debug, PartialEq, Clone)]
    pub enum Type {
        Int,
        Float,
        Bool,
        Str,
        Infer,
    }

    #[derive(Debug, PartialEq, Clone)]
    pub enum Expr {
        IntLit(i64),
        FloatLit(f64),
        StrLit(String),
        BoolLit(bool),
        Ident(String),
        Binary {
            op: BinaryOp,
            lhs: Box<Expr>,
            rhs: Box<Expr>,
        },
        Unary {
            op: UnaryOp,
            expr: Box<Expr>,
        },
        /// A placeholder for a region of source the parser could not
        /// interpret as an expression. The parser never stops: it plants this
        /// node, records the diagnostic, and continues, so surrounding
        /// statements stay intact for tooling (for example an LSP that still
        /// sees the declared variable). A zero-width `span` marks source text
        /// that is missing entirely, such as an initializer not yet typed.
        Invalid {
            error: SyntaxError,
            span: Span,
        },
    }

    impl Expr {
        /// Returns `true` if this expression or any subexpression is `Invalid`.
        pub fn contains_invalid(&self) -> bool {
            match self {
                Expr::Invalid { .. } => true,
                Expr::Binary { lhs, rhs, .. } => lhs.contains_invalid() || rhs.contains_invalid(),
                Expr::Unary { expr, .. } => expr.contains_invalid(),
                _ => false,
            }
        }
    }

    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub enum BinaryOp {
        Or,
        And,
        Eq,
        Ne,
        Lt,
        Le,
        Gt,
        Ge,
        Add,
        Sub,
        Mul,
        Div,
        Rem,
    }

    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub enum UnaryOp {
        Neg,
        Not,
    }

    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub enum CompoundOp {
        Add,
        Sub,
        Mul,
        Div,
        Rem,
    }

    #[derive(Debug, PartialEq, Clone)]
    pub enum Stmt {
        VarDecl {
            ty: Type,
            name: String,
            expr: Expr,
        },
        Assign {
            name: String,
            expr: Expr,
        },
        CompoundAssign {
            target: String,
            op: CompoundOp,
            expr: Expr,
        },
        /// A placeholder for a whole statement the parser could not recognize
        /// (not even its head). Its `span` covers the skipped source region so
        /// statement positions stay aligned with the file, which keeps
        /// document outlines and symbol tables stable on broken code.
        Invalid {
            error: SyntaxError,
            span: Span,
        },
    }

    impl Stmt {
        /// Returns `true` if this statement is or contains an `Invalid` node.
        /// Execution-facing consumers can use this (or the parser's diagnostics
        /// list) as a gate before running a program, while tooling consumers
        /// may instead keep working around the broken parts.
        pub fn contains_invalid(&self) -> bool {
            match self {
                Stmt::Invalid { .. } => true,
                Stmt::VarDecl { expr, .. }
                | Stmt::Assign { expr, .. }
                | Stmt::CompoundAssign { expr, .. } => expr.contains_invalid(),
            }
        }
    }
}

pub use ast::Span;

#[cfg(test)]
mod tests {
    use super::ast::{Expr, Span, Stmt, SyntaxError, Type};

    #[test]
    fn invalid_nodes_report_containment() {
        let broken = Expr::Invalid {
            error: SyntaxError::new("Expected an expression", Span::new(0, 1)),
            span: Span::new(0, 1),
        };
        let healthy = Expr::IntLit(1);

        assert!(broken.contains_invalid());
        assert!(!healthy.contains_invalid());
        assert!(
            Stmt::VarDecl {
                ty: Type::Int,
                name: "i".into(),
                expr: broken
            }
            .contains_invalid()
        );
        assert!(
            !Stmt::Assign {
                name: "i".into(),
                expr: healthy
            }
            .contains_invalid()
        );
    }

    #[test]
    fn missing_spans_are_zero_width() {
        let span = Span::missing(7);
        assert_eq!(span.start, 7);
        assert_eq!(span.end, 7);
    }
}
