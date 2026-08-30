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
    }
}

pub use ast::Span;
