//! `cme-core` owns the language data model: the spanned AST, source spans, and
//! diagnostic references. Recognition and parsing stay in `cme-compiler`.
//!
//! A hand-built declaration looks like this:
//!
//! ```
//! use cme_core::ast::{Expr, ExprKind, PrimitiveType, Span, Stmt, StmtKind, Type};
//!
//! let stmt = Stmt::new(
//!     StmtKind::VarDecl {
//!         ty: Type::Prim(PrimitiveType::Int),
//!         name: "x".to_string(),
//!         expr: Expr::new(ExprKind::IntLit(1), Span::new(8, 9)),
//!     },
//!     Span::new(0, 9),
//! );
//! assert_eq!(stmt.span.end, 9);
//! ```

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

    /// The index of a diagnostic in the diagnostics list produced with an AST.
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub struct ErrorId(pub usize);

    /// A declared type. `None` (the "infer" pseudo-type) crystallizes at
    /// validation time; `Some(PrimitiveType)` covers the four scalar types.
    /// `Void` is only valid as a function return type.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Type {
        Infer,
        Prim(PrimitiveType),
        Void,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PrimitiveType {
        Int,
        Float,
        Bool,
        Str,
    }

    /// A function parameter: declared type plus name.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Param {
        pub ty: Type,
        pub name: String,
    }

    /// A braced block of statements. Own struct (rather than `Vec<Stmt>`) so
    /// the block's `{`/`}` span is preserved for tooling and future
    /// scope-aware consumers.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Block {
        pub span: Span,
        pub stmts: Vec<Stmt>,
    }

    #[derive(Debug, Clone)]
    pub struct Expr {
        pub span: Span,
        pub kind: ExprKind,
    }

    impl Expr {
        pub fn new(kind: ExprKind, span: Span) -> Self {
            Self { span, kind }
        }

        /// Returns `true` if this expression or any subexpression is `Invalid`.
        pub fn contains_invalid(&self) -> bool {
            match &self.kind {
                ExprKind::Invalid { .. } => true,
                ExprKind::Binary { lhs, rhs, .. } => {
                    lhs.contains_invalid() || rhs.contains_invalid()
                }
                ExprKind::Unary { expr, .. } => expr.contains_invalid(),
                _ => false,
            }
        }
    }

    impl PartialEq for Expr {
        fn eq(&self, other: &Self) -> bool {
            self.kind == other.kind
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum ExprKind {
        IntLit(i64),
        FloatLit(f64),
        StrLit(String),
        BoolLit(bool),
        Ident(String),
        Paren {
            expr: Box<Expr>,
        },
        Binary {
            op: BinaryOp,
            lhs: Box<Expr>,
            rhs: Box<Expr>,
        },
        Unary {
            op: UnaryOp,
            expr: Box<Expr>,
        },
        Call {
            name: String,
            args: Vec<Expr>,
        },
        /// A placeholder for a region of source the parser could not
        /// interpret as an expression. The parser never stops: it plants this
        /// node, records the diagnostic, and continues, so surrounding
        /// statements stay intact for tooling (for example an LSP that still
        /// sees the declared variable). A zero-width outer `span` marks source
        /// text that is missing entirely, such as an initializer not yet typed.
        Invalid {
            error: ErrorId,
        },
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

    #[derive(Debug, Clone)]
    pub struct Stmt {
        pub span: Span,
        pub kind: StmtKind,
    }

    impl Stmt {
        pub fn new(kind: StmtKind, span: Span) -> Self {
            Self { span, kind }
        }

        /// Returns `true` if this statement is or contains an `Invalid` node.
        /// Execution-facing consumers can use this (or the parser's diagnostics
        /// list) as a gate before running a program, while tooling consumers
        /// may instead keep working around the broken parts.
        pub fn contains_invalid(&self) -> bool {
            match &self.kind {
                StmtKind::Invalid { .. } => true,
                StmtKind::VarDecl { expr, .. }
                | StmtKind::Assign { expr, .. }
                | StmtKind::CompoundAssign { expr, .. }
                | StmtKind::Expression { expr } => expr.contains_invalid(),
                _ => false,
            }
        }
    }

    impl PartialEq for Stmt {
        fn eq(&self, other: &Self) -> bool {
            self.kind == other.kind
        }
    }

    #[derive(Debug, PartialEq, Clone)]
    pub enum StmtKind {
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
        /// A statement consisting solely of a call expression; its value is
        /// discarded. Other expression statements remain invalid.
        Expression {
            expr: Expr,
        },
        FuncDecl {
            name: String,
            params: Vec<Param>,
            return_ty: Type,
            body: Block,
        },
        If {
            cond: Expr,
            then_branch: Block,
            else_branch: Option<Box<Stmt>>,
        },
        While {
            cond: Expr,
            body: Block,
        },
        Return {
            value: Option<Expr>,
        },
        /// A braced block used as a statement body (`else { ... }`). The
        /// parser reuses the If node for chains, so a plain else block needs
        /// its own statement wrapper.
        Block(Block),
        /// A placeholder for a whole statement the parser could not recognize
        /// (not even its head). Its outer `span` covers the skipped source
        /// region so statement positions stay aligned with the file, which
        /// keeps document outlines and symbol tables stable on broken code.
        Invalid {
            error: ErrorId,
        },
    }
}

pub use ast::Span;

#[cfg(test)]
mod tests {
    use super::ast::{ErrorId, Expr, ExprKind, PrimitiveType, Span, Stmt, StmtKind, Type};

    #[test]
    fn invalid_nodes_report_containment() {
        let broken = Expr {
            span: Span::new(0, 1),
            kind: ExprKind::Invalid { error: ErrorId(0) },
        };
        let healthy = Expr::new(ExprKind::IntLit(1), Span::new(0, 1));

        assert!(broken.contains_invalid());
        assert!(!healthy.contains_invalid());
        assert!(
            Stmt {
                span: Span::new(0, 1),
                kind: StmtKind::VarDecl {
                    ty: Type::Prim(PrimitiveType::Int),
                    name: "i".into(),
                    expr: broken
                },
            }
            .contains_invalid()
        );
        assert!(
            !Stmt {
                span: Span::new(0, 1),
                kind: StmtKind::Assign {
                    name: "i".into(),
                    expr: healthy
                },
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
