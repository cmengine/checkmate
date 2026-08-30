pub mod ast {
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
    }

    #[derive(Debug, PartialEq, Clone)]
    pub enum Stmt {
        VarDecl { ty: Type, name: String, expr: Expr },
    }
}
