//! `cme-core` owns the language data model: the spanned AST, source spans, and
//! diagnostic references. Recognition and parsing stay in `cme-compiler`.
//!
//! A hand-built declaration looks like this:
//!
//! ```
//! use cme_core::ast::{Expr, ExprKind, LValue, PrimitiveType, Span, Stmt, StmtKind, Type};
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
//!
//! // Assignments target lvalues: a variable, a field, or an index.
//! let stmt = Stmt::new(
//!     StmtKind::Assign {
//!         target: LValue::Var { name: "x".to_string() },
//!         expr: Expr::new(ExprKind::IntLit(2), Span::new(12, 13)),
//!     },
//!     Span::new(0, 13),
//! );
//! assert_eq!(stmt.span.end, 13);
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

    /// A declared type. `Infer` (the "infer" pseudo-type) crystallizes at
    /// validation time; `Void` is only valid as a function return type.
    /// `Named` covers struct and enum types (built-in `option<T>` /
    /// `result<T, E>` included), `Array` is `T[]`, and `Map` is `map<K, V>`
    /// (§2.4, §2.6–§2.9, §11).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Type {
        Infer,
        Prim(PrimitiveType),
        Void,
        /// A named struct/enum type with zero or more generic arguments:
        /// `vec2`, `option<int>`, `result<T, E>`, `pair<int, str>`.
        Named {
            name: String,
            args: Vec<Type>,
        },
        /// Array of the element type: `int[]`.
        Array(Box<Type>),
        /// Map with key and value types: `map<str, int>`.
        Map {
            key: Box<Type>,
            value: Box<Type>,
        },
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

    /// A named, typed field of a struct (§2.6) or an enum variant payload
    /// (§2.7). Enum payloads are comma-separated in declarations; struct
    /// fields are newline-delimited.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FieldDef {
        pub ty: Type,
        pub name: String,
    }

    /// A declared enum variant (§2.7): the variant name plus its typed
    /// payload fields.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct VariantDecl {
        pub name: String,
        pub fields: Vec<FieldDef>,
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
                ExprKind::Unary { expr, .. } | ExprKind::Paren { expr } => expr.contains_invalid(),
                ExprKind::Try { expr } => expr.contains_invalid(),
                ExprKind::Call { args, .. }
                | ExprKind::VariantCall { args, .. }
                | ExprKind::PathCall { args, .. } => args.iter().any(call_arg_contains_invalid),
                ExprKind::Field { obj, .. } => obj.contains_invalid(),
                ExprKind::Index { obj, index } => {
                    obj.contains_invalid() || index.contains_invalid()
                }
                ExprKind::Match { scrutinee, arms } => {
                    scrutinee.contains_invalid()
                        || arms.iter().any(|arm| arm.body.contains_invalid())
                }
                ExprKind::ArrayLit { elements } => elements.iter().any(Expr::contains_invalid),
                ExprKind::MapLit { entries } => entries
                    .iter()
                    .any(|(key, value)| key.contains_invalid() || value.contains_invalid()),
                ExprKind::Interpolated { parts } => parts.iter().any(|part| match part {
                    InterpPart::Literal(_) => false,
                    InterpPart::Expr(expr) => expr.contains_invalid(),
                }),
                _ => false,
            }
        }
    }

    fn call_arg_contains_invalid(arg: &CallArg) -> bool {
        match arg {
            CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr.contains_invalid(),
        }
    }

    impl PartialEq for Expr {
        fn eq(&self, other: &Self) -> bool {
            self.kind == other.kind
        }
    }

    /// One argument in a call or a construction: positional or named (§2.12).
    /// Mixing both forms in a single list is a compile-time syntax error.
    #[derive(Debug, Clone, PartialEq)]
    pub enum CallArg {
        Positional(Expr),
        Named { name: String, expr: Expr },
    }

    /// One part of an interpolated string (§2.8/§4.1): literal text or an
    /// embedded expression island `{expr}`.
    #[derive(Debug, Clone, PartialEq)]
    pub enum InterpPart {
        Literal(String),
        Expr(Box<Expr>),
    }

    /// One arm of a `match` expression (§2.15): the pattern plus the
    /// expression the arm yields.
    #[derive(Debug, Clone, PartialEq)]
    pub struct MatchArmExpr {
        pub pattern: Pattern,
        pub body: Expr,
    }

    /// One arm of a `match` statement (§2.15): the pattern plus the block
    /// executed when the pattern matches.
    #[derive(Debug, Clone, PartialEq)]
    pub struct MatchArmStmt {
        pub pattern: Pattern,
        pub body: Block,
    }

    /// A match arm pattern (§2.15): a variant of the scrutinee's enum with
    /// typed payload bindings, or the `_` wildcard.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Pattern {
        Wildcard,
        Variant {
            variant: String,
            bindings: Vec<FieldDef>,
        },
    }

    /// An assignment target (§2.10, §2.13, §A.7): a variable, a field of a
    /// target, or an index into a target. The base and every index along the
    /// chain are evaluated exactly once per assignment.
    #[derive(Debug, Clone, PartialEq)]
    pub enum LValue {
        Var { name: String },
        Field { base: Box<LValue>, name: String },
        Index { base: Box<LValue>, index: Expr },
    }

    impl LValue {
        /// Returns `true` if the index expressions in this chain contain an
        /// `Invalid` node.
        pub fn contains_invalid(&self) -> bool {
            match self {
                LValue::Var { .. } => false,
                LValue::Field { base, .. } => base.contains_invalid(),
                LValue::Index { base, index } => {
                    base.contains_invalid() || index.contains_invalid()
                }
            }
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
        /// A call to a function, a struct construction, or a built-in
        /// constructor (`Ok`, `Err`, `Some`, `None`): the callee is a plain
        /// name, disambiguated by the checker (§2.11, §2.6, §2.8).
        Call {
            name: String,
            args: Vec<CallArg>,
        },
        /// A qualified enum construction `Enum.Variant(args)` (§2.7), or an
        /// impl member call `Target.Member(args)` (§10.4) when `Target` is a
        /// struct/enum carrying an impl block. The checker and interpreter
        /// disambiguate: variant first, impl member second.
        VariantCall {
            enum_name: String,
            variant: String,
            args: Vec<CallArg>,
        },
        /// A call through a dotted path of three or more segments:
        /// `engine.gamemode.InitGame(args)` (§2.3, §10.4). The last segment
        /// names the member; the leading segments name the impl target.
        PathCall {
            path: Vec<String>,
            args: Vec<CallArg>,
        },
        /// `obj.name`: struct field access (§2.6) or `.length` on an array
        /// (§11).
        Field {
            obj: Box<Expr>,
            name: String,
        },
        /// `obj[index]`: array indexing or map lookup (§11).
        Index {
            obj: Box<Expr>,
            index: Box<Expr>,
        },
        /// `match (scrutinee) { arms }` in expression position: every arm
        /// yields a value of the same type (§2.15).
        Match {
            scrutinee: Box<Expr>,
            arms: Vec<MatchArmExpr>,
        },
        /// `[a, b, c]`: array literal (§11).
        ArrayLit {
            elements: Vec<Expr>,
        },
        /// `{ key: value }`: map literal (§11).
        MapLit {
            entries: Vec<(Expr, Expr)>,
        },
        /// `$"...{expr}..."`: interpolated string (§2.8/§4.1).
        Interpolated {
            parts: Vec<InterpPart>,
        },
        /// `expr?`: early-return propagation for `result<T, E>` (§2.8).
        Try {
            expr: Box<Expr>,
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
                StmtKind::StructDecl { fields, .. } => {
                    fields.iter().any(|field| field.ty.contains_invalid_type())
                }
                StmtKind::EnumDecl { variants, .. } => variants.iter().any(|variant| {
                    variant
                        .fields
                        .iter()
                        .any(|field| field.ty.contains_invalid_type())
                }),
                StmtKind::ImplDecl { members, .. } => {
                    members.iter().any(|member| member.contains_invalid())
                }
                _ => false,
            }
        }
    }

    /// Structural search for `Invalid` inside a declared type: array/map
    /// element types and generic arguments are the only positions where a
    /// broken type expression can hide.
    impl Type {
        fn contains_invalid_type(&self) -> bool {
            match self {
                Type::Array(elem) => elem.contains_invalid_type(),
                Type::Map { key, value } => {
                    key.contains_invalid_type() || value.contains_invalid_type()
                }
                Type::Named { args, .. } => args.iter().any(Self::contains_invalid_type),
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
            target: LValue,
            expr: Expr,
        },
        CompoundAssign {
            target: LValue,
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
        /// A struct type declaration (§2.6, §2.9): name, optional type
        /// parameters, and newline-delimited fields.
        StructDecl {
            name: String,
            type_params: Vec<String>,
            fields: Vec<FieldDef>,
        },
        /// An enum type declaration (§2.7, §2.9): name, optional type
        /// parameters, and newline-delimited variants.
        EnumDecl {
            name: String,
            type_params: Vec<String>,
            variants: Vec<VariantDecl>,
        },
        /// An impl block (§10.4): `impl target.path { members }`. The target
        /// is a dotted path — a locally declared struct/enum name (single
        /// segment) or a host-style namespace path (`engine.gamemode`).
        /// Members are plain function declarations scoped under the target;
        /// the parser guarantees every member is a `FuncDecl` statement, and
        /// blocks for the same target are unioned (a member implemented
        /// twice is a compile error).
        ImplDecl {
            target: Vec<String>,
            members: Vec<Stmt>,
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
        /// `for (elem elemName in iterable) { body }` (§2.14): iterates an
        /// array in order, binding each element to a fresh declaration.
        For {
            elem_ty: Type,
            elem_name: String,
            iterable: Expr,
            body: Block,
        },
        /// `match (scrutinee) { arms }` in statement position (§2.15): each
        /// arm executes its block when the pattern matches.
        Match {
            scrutinee: Expr,
            arms: Vec<MatchArmStmt>,
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
    use super::ast::{
        Block, CallArg, ErrorId, Expr, ExprKind, LValue, PrimitiveType, Span, Stmt, StmtKind, Type,
    };

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
                    target: LValue::Var { name: "i".into() },
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

    #[test]
    fn lvalue_chains_report_invalid_index_containment() {
        let broken = Expr {
            span: Span::new(0, 1),
            kind: ExprKind::Invalid { error: ErrorId(0) },
        };
        let target = LValue::Index {
            base: Box::new(LValue::Field {
                base: Box::new(LValue::Var { name: "m".into() }),
                name: "field".into(),
            }),
            index: broken,
        };
        assert!(target.contains_invalid());
    }

    #[test]
    fn impl_members_and_path_calls_report_containment() {
        let broken = Expr {
            span: Span::new(0, 1),
            kind: ExprKind::Invalid { error: ErrorId(0) },
        };

        // A path call whose arguments contain an Invalid node.
        let call = Expr::new(
            ExprKind::PathCall {
                path: vec!["engine".into(), "gamemode".into(), "InitGame".into()],
                args: vec![CallArg::Positional(broken)],
            },
            Span::new(0, 1),
        );
        assert!(call.contains_invalid());

        // An impl block reports an Invalid member statement.
        let broken_impl = Stmt::new(
            StmtKind::ImplDecl {
                target: vec!["engine".into(), "gamemode".into()],
                members: vec![Stmt::new(
                    StmtKind::Invalid { error: ErrorId(0) },
                    Span::new(0, 1),
                )],
            },
            Span::new(0, 1),
        );
        assert!(broken_impl.contains_invalid());

        // A member function declaration is opaque to containment, exactly
        // like a top-level function: the diagnostics list gates execution.
        let healthy = Expr::new(ExprKind::IntLit(1), Span::new(0, 1));
        let member = Stmt::new(
            StmtKind::FuncDecl {
                name: "InitGame".into(),
                params: Vec::new(),
                return_ty: Type::Void,
                body: Block {
                    span: Span::new(0, 1),
                    stmts: vec![Stmt::new(
                        StmtKind::Expression { expr: healthy },
                        Span::new(0, 1),
                    )],
                },
            },
            Span::new(0, 1),
        );
        let clean_impl = Stmt::new(
            StmtKind::ImplDecl {
                target: vec!["counter".into()],
                members: vec![member],
            },
            Span::new(0, 1),
        );
        assert!(!clean_impl.contains_invalid());
    }
}
