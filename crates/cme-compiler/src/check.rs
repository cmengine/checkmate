//! The static type checker for the basic subset. Normative sources:
//! WHITEPAPER §2.10–2.16 for declarations, functions, control flow, and
//! `infer` crystallization; Appendix A (§A.4–§A.7) for operand typing,
//! string concatenation, and compound assignment.
//!
//! The checker is two-pass: all function signatures are collected first
//! (so forward references and recursion resolve), then bodies are checked
//! against per-function scopes. It runs on any input: [`Invalid`] nodes
//! and untypable regions are skipped without cascading — a declaration
//! with a broken initializer still declares its name, and nothing inside
//! a broken subtree is reported twice. Function declarations are legal
//! only at top level: nested ones are reported and their bodies skipped.
//!
//! ```
//! let outcome = cme_compiler::parse_source("int f() {\nint hp = 100\nhp += 5\nreturn hp\n}\n");
//! assert!(cme_compiler::check::check(&outcome.statements).is_empty());
//! ```

use std::collections::HashMap;

use crate::diagnostics::Diagnostic;
use cme_core::Span;
use cme_core::ast::{
    BinaryOp, Block, CompoundOp, Expr, ExprKind, Param, PrimitiveType, Stmt, StmtKind, Type,
    UnaryOp,
};

/// A function signature collected in the first pass. `poisoned` marks a
/// signature the checker cannot use (an `infer` return type or an
/// `infer`/`void` parameter — both already rejected at parse level);
/// poisoned functions stay in the table so calls to them resolve silently
/// instead of reporting phantom "unknown function" errors.
struct FnSig {
    params: Vec<Param>,
    return_ty: Type,
    poisoned: bool,
}

/// The type of an expression during checking. `Void` only arises from
/// calling a void function where a value is needed (§2.4: void returns no
/// value). `Poison` marks a region already reported or unrecoverable
/// (an [`Invalid`] subtree, an untypable initializer); every check against
/// it passes silently so recovery diagnostics never cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueTy {
    Int,
    Float,
    Bool,
    Str,
    Void,
    Poison,
}

impl ValueTy {
    fn name(self) -> &'static str {
        match self {
            ValueTy::Int => "int",
            ValueTy::Float => "float",
            ValueTy::Bool => "bool",
            ValueTy::Str => "str",
            ValueTy::Void => "void",
            ValueTy::Poison => "poison",
        }
    }
}

impl std::fmt::Display for ValueTy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

fn value_ty_of(ty: &Type) -> ValueTy {
    match ty {
        Type::Prim(PrimitiveType::Int) => ValueTy::Int,
        Type::Prim(PrimitiveType::Float) => ValueTy::Float,
        Type::Prim(PrimitiveType::Bool) => ValueTy::Bool,
        Type::Prim(PrimitiveType::Str) => ValueTy::Str,
        Type::Void => ValueTy::Void,
        Type::Infer => ValueTy::Poison,
    }
}

/// True when a function's signature is unusable by the checker: `infer`
/// as a return type (§2.16: local declarations only) or `infer`/`void`
/// as a parameter type (§1: parameters are explicitly typed). Both are
/// parse-level errors already; poisoning only prevents cascades.
fn signature_poisoned(return_ty: &Type, params: &[Param]) -> bool {
    *return_ty == Type::Infer
        || params
            .iter()
            .any(|param| matches!(param.ty, Type::Infer | Type::Void))
}

/// Type-checks a whole program. Returns every violation found; the list
/// is empty exactly when the program satisfies §2.10–2.16 and §A.4–§A.7
/// for the basic subset.
pub fn check(statements: &[Stmt]) -> Vec<Diagnostic> {
    let mut checker = Checker::new();

    // Pass 1: top-level shape and function signatures. Forward references
    // and recursion resolve because every signature is registered before
    // any body is visited. Only the first registration of a name wins:
    // later duplicates are reported here and skipped everywhere else,
    // exactly like the calls that resolve to the first registration.
    let mut registered: Vec<usize> = Vec::new();
    for (index, statement) in statements.iter().enumerate() {
        match &statement.kind {
            StmtKind::FuncDecl {
                name,
                params,
                return_ty,
                ..
            } => {
                if checker.functions.contains_key(name) {
                    checker.report(format!("duplicate function `{name}`"), statement.span);
                } else {
                    checker.check_duplicate_params(params, statement.span);
                    checker.functions.insert(
                        name.clone(),
                        FnSig {
                            params: params.clone(),
                            return_ty: return_ty.clone(),
                            poisoned: signature_poisoned(return_ty, params),
                        },
                    );
                    registered.push(index);
                }
            }
            // Already reported at parse level; never cascaded here.
            StmtKind::Invalid { .. } => {}
            _ => checker.report(
                "only function declarations are allowed at top level",
                statement.span,
            ),
        }
    }

    // Pass 2: bodies, each against its own signature and fresh scopes. A
    // duplicate's body is not checked: calls resolve to the first
    // registration, so a dead definition's errors would only be noise on
    // top of the duplicate report.
    for index in registered {
        if let StmtKind::FuncDecl {
            name,
            params,
            return_ty,
            body,
            ..
        } = &statements[index].kind
            && !signature_poisoned(return_ty, params)
        {
            checker.check_function(name, params, return_ty, body);
        }
    }

    checker.diagnostics
}

struct Checker {
    diagnostics: Vec<Diagnostic>,
    functions: HashMap<String, FnSig>,
    /// Scope stack for the function currently being checked. Index 0
    /// holds the parameters together with the body's top-level statements
    /// (redeclaring a parameter there is a duplicate, not a shadow).
    scopes: Vec<HashMap<String, ValueTy>>,
    /// The function whose body is being checked, for `return` validation.
    current_fn: Option<(String, Type)>,
}

impl Checker {
    fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
            functions: HashMap::new(),
            scopes: Vec::new(),
            current_fn: None,
        }
    }

    fn report(&mut self, message: impl Into<String>, span: Span) {
        self.diagnostics.push(Diagnostic::type_error(message, span));
    }

    fn check_duplicate_params(&mut self, params: &[Param], span: Span) {
        let mut seen: Vec<&str> = Vec::new();
        for param in params {
            if seen.contains(&param.name.as_str()) {
                self.report(format!("duplicate parameter `{}`", param.name), span);
            } else {
                seen.push(&param.name);
            }
        }
    }

    fn check_function(&mut self, name: &str, params: &[Param], return_ty: &Type, body: &Block) {
        self.current_fn = Some((name.to_string(), return_ty.clone()));

        let mut scope = HashMap::new();
        for param in params {
            scope.insert(param.name.clone(), value_ty_of(&param.ty));
        }
        self.scopes.push(scope);

        for statement in &body.stmts {
            self.check_stmt(statement);
        }

        // §2.11 + the owner ruling: a non-void function must not be able
        // to fall off the end. Simple structural walk — an `if` without
        // `else` does not count, `while` never counts, and `if`/`else`
        // counts only when both branches transfer control.
        if *return_ty != Type::Void && !block_returns(body) {
            self.report(
                format!("missing return in non-void function `{name}`"),
                body.span,
            );
        }

        self.scopes.pop();
        self.current_fn = None;
    }

    fn check_block(&mut self, block: &Block) {
        self.scopes.push(HashMap::new());
        for statement in &block.stmts {
            self.check_stmt(statement);
        }
        self.scopes.pop();
    }

    /// Declares `name` in the innermost scope, reporting redeclaration in
    /// the same scope and shadowing of an enclosing scope (owner ruling:
    /// shadowing is forbidden). A declaration enters scope *after* its
    /// own initializer, so callers type the initializer first.
    fn declare(&mut self, name: &str, ty: ValueTy, span: Span) {
        if let Some(index) = self
            .scopes
            .iter()
            .rposition(|scope| scope.contains_key(name))
        {
            if index + 1 == self.scopes.len() {
                self.report(format!("duplicate declaration of `{name}`"), span);
            } else {
                self.report(
                    format!("declaration of `{name}` shadows a declaration in an enclosing scope"),
                    span,
                );
            }
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<ValueTy> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Invalid { .. } => {}
            // Nested declarations parse (tolerance) but are illegal: the
            // subset only has top-level function declarations. The body
            // of the illegal declaration is skipped, and calls to its
            // name stay unknown (it never registers).
            StmtKind::FuncDecl { .. } => {
                self.report(
                    "function declarations are only allowed at top level",
                    stmt.span,
                );
            }
            StmtKind::VarDecl { ty, name, expr } => {
                // §2.16: the initializer is typed before the name exists,
                // so `int a = a` reports the unknown name.
                let init = self.type_expr(expr);
                match ty {
                    Type::Infer => match init {
                        ValueTy::Void => {
                            self.report(
                                format!("cannot infer type for '{name}'; void initializer"),
                                stmt.span,
                            );
                            self.declare(name, ValueTy::Poison, stmt.span);
                        }
                        ValueTy::Poison => self.declare(name, ValueTy::Poison, stmt.span),
                        crystallized => self.declare(name, crystallized, stmt.span),
                    },
                    // §2.4: void returns no value; parse rejects this from
                    // source, hand-built trees get the same message here.
                    Type::Void => {
                        self.report("`void` is only valid as a function return type", stmt.span);
                        self.declare(name, ValueTy::Poison, stmt.span);
                    }
                    Type::Prim(_) => {
                        let declared = value_ty_of(ty);
                        if init != ValueTy::Poison && init != declared {
                            self.report(
                                format!(
                                    "type mismatch in declaration of `{name}`: expected `{declared}`, found `{init}`"
                                ),
                                stmt.span,
                            );
                        }
                        // The declaration survives a broken initializer with
                        // its declared type — that is the recovery design.
                        self.declare(name, declared, stmt.span);
                    }
                }
            }
            StmtKind::Assign { name, expr } => {
                let rhs = self.type_expr(expr);
                match self.lookup(name) {
                    Some(declared) if declared == ValueTy::Poison || rhs == ValueTy::Poison => {}
                    Some(declared) if rhs != declared => {
                        self.report(
                            format!(
                                "type mismatch in assignment to `{name}`: expected `{declared}`, found `{rhs}`"
                            ),
                            stmt.span,
                        );
                    }
                    Some(_) => {}
                    None => self.report(format!("unknown name `{name}`"), stmt.span),
                }
            }
            StmtKind::CompoundAssign { target, op, expr } => {
                // §A.7: `x op= e` is exactly `x = x op e`, so the operator
                // rules of §A.4/§A.6 apply with the target as the left
                // operand and the result must equal the target's type.
                let rhs = self.type_expr(expr);
                match self.lookup(target) {
                    Some(declared) if declared == ValueTy::Poison || rhs == ValueTy::Poison => {}
                    Some(declared) => {
                        let matches = binary_result(compound_to_binary(*op), declared, rhs)
                            .is_some_and(|result| result == declared);
                        if !matches {
                            self.report(
                                format!(
                                    "cannot apply `{}` to `{declared}` and `{rhs}`",
                                    compound_op_symbol(*op)
                                ),
                                stmt.span,
                            );
                        }
                    }
                    None => self.report(format!("unknown name `{target}`"), stmt.span),
                }
            }
            StmtKind::Expression { expr } => match &expr.kind {
                ExprKind::Call { .. } => {
                    self.type_expr(expr);
                }
                ExprKind::Invalid { .. } => {}
                // Owner ruling: only call expressions may be statements.
                _ => self.report("expression statements must be function calls", stmt.span),
            },
            StmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_ty = self.type_expr(cond);
                if cond_ty != ValueTy::Poison && cond_ty != ValueTy::Bool {
                    self.report(
                        format!("if condition must be `bool`, found `{cond_ty}`"),
                        cond.span,
                    );
                }
                self.check_block(then_branch);
                if let Some(else_stmt) = else_branch {
                    self.check_stmt(else_stmt);
                }
            }
            StmtKind::While { cond, body } => {
                let cond_ty = self.type_expr(cond);
                if cond_ty != ValueTy::Poison && cond_ty != ValueTy::Bool {
                    self.report(
                        format!("while condition must be `bool`, found `{cond_ty}`"),
                        cond.span,
                    );
                }
                self.check_block(body);
            }
            StmtKind::Return { value } => self.check_return(stmt, value.as_ref()),
            StmtKind::Block(block) => self.check_block(block),
        }
    }

    fn check_return(&mut self, stmt: &Stmt, value: Option<&Expr>) {
        let Some((name, return_ty)) = self.current_fn.clone() else {
            return;
        };
        match return_ty {
            Type::Void => {
                if let Some(expr) = value {
                    // Still resolve names inside the value.
                    self.type_expr(expr);
                    self.report(
                        format!("void function `{name}` cannot return a value"),
                        stmt.span,
                    );
                }
            }
            // An `infer` return type only reaches a hand-built tree (parse
            // rejects it); there is no expected type to compare against.
            Type::Infer => {
                if let Some(expr) = value {
                    self.type_expr(expr);
                }
            }
            Type::Prim(_) => match value {
                None => self.report(
                    format!("non-void function `{name}` must return a value"),
                    stmt.span,
                ),
                Some(expr) => {
                    let actual = self.type_expr(expr);
                    let expected = value_ty_of(&return_ty);
                    if actual != ValueTy::Poison && actual != expected {
                        self.report(
                            format!(
                                "wrong return type in `{name}`: expected `{expected}`, found `{actual}`"
                            ),
                            stmt.span,
                        );
                    }
                }
            },
        }
    }

    fn type_expr(&mut self, expr: &Expr) -> ValueTy {
        match &expr.kind {
            ExprKind::IntLit(_) => ValueTy::Int,
            ExprKind::FloatLit(_) => ValueTy::Float,
            ExprKind::StrLit(_) => ValueTy::Str,
            ExprKind::BoolLit(_) => ValueTy::Bool,
            // Recovery placeholder: already reported at parse level.
            ExprKind::Invalid { .. } => ValueTy::Poison,
            ExprKind::Ident(name) => self.lookup(name).unwrap_or_else(|| {
                self.report(format!("unknown name `{name}`"), expr.span);
                ValueTy::Poison
            }),
            ExprKind::Paren { expr } => self.type_expr(expr),
            ExprKind::Call { name, args } => self.type_call(name, args, expr.span),
            ExprKind::Unary { op, expr: inner } => {
                let operand = self.type_expr(inner);
                if operand == ValueTy::Poison {
                    return ValueTy::Poison;
                }
                let ok = match op {
                    UnaryOp::Neg => matches!(operand, ValueTy::Int | ValueTy::Float),
                    UnaryOp::Not => operand == ValueTy::Bool,
                };
                if !ok {
                    self.report(
                        format!("cannot apply `{}` to `{operand}`", unary_op_symbol(*op)),
                        expr.span,
                    );
                    return ValueTy::Poison;
                }
                operand
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let left = self.type_expr(lhs);
                let right = self.type_expr(rhs);
                if left == ValueTy::Poison || right == ValueTy::Poison {
                    return ValueTy::Poison;
                }
                match binary_result(*op, left, right) {
                    Some(result) => result,
                    None => {
                        self.report(
                            format!(
                                "cannot apply `{}` to `{left}` and `{right}`",
                                binary_op_symbol(*op)
                            ),
                            expr.span,
                        );
                        ValueTy::Poison
                    }
                }
            }
        }
    }

    fn type_call(&mut self, name: &str, args: &[Expr], span: Span) -> ValueTy {
        let Some(sig) = self.functions.get(name) else {
            // Arguments are still typed so unknown names inside them are
            // reported rather than swallowed.
            for arg in args {
                self.type_expr(arg);
            }
            self.report(format!("unknown function `{name}`"), span);
            return ValueTy::Poison;
        };

        // Copy the signature out so typing the arguments (which mutates
        // `self`) does not hold a borrow on the function table.
        let expected: Vec<ValueTy> = sig.params.iter().map(|p| value_ty_of(&p.ty)).collect();
        let result_ty = value_ty_of(&sig.return_ty);
        let poisoned = sig.poisoned;

        if args.len() != expected.len() {
            self.report(
                format!(
                    "wrong number of arguments to `{name}`: expected {}, found {}",
                    expected.len(),
                    args.len()
                ),
                span,
            );
        }

        let mut arg_types = Vec::with_capacity(args.len());
        for arg in args {
            arg_types.push(self.type_expr(arg));
        }

        if !poisoned {
            for ((arg, arg_ty), param_ty) in args.iter().zip(&arg_types).zip(&expected) {
                if *arg_ty != ValueTy::Poison && arg_ty != param_ty {
                    self.report(
                        format!(
                            "wrong argument type in call to `{name}`: expected `{param_ty}`, found `{arg_ty}`"
                        ),
                        arg.span,
                    );
                }
            }
        }

        if poisoned { ValueTy::Poison } else { result_ty }
    }
}

/// §A.4 operand typing. `None` means the operator does not accept the
/// operand types. `void` is not a value, so no operator accepts it.
fn binary_result(op: BinaryOp, left: ValueTy, right: ValueTy) -> Option<ValueTy> {
    if left == ValueTy::Void || right == ValueTy::Void {
        return None;
    }
    match op {
        BinaryOp::Add => match (left, right) {
            (ValueTy::Int, ValueTy::Int) => Some(ValueTy::Int),
            (ValueTy::Float, ValueTy::Float) => Some(ValueTy::Float),
            // §A.6: either side str, the other stringifies. (str, str) is
            // covered by the first alternative.
            (ValueTy::Str, ValueTy::Str | ValueTy::Int | ValueTy::Float | ValueTy::Bool)
            | (ValueTy::Int | ValueTy::Float | ValueTy::Bool, ValueTy::Str) => Some(ValueTy::Str),
            _ => None,
        },
        BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => match (left, right) {
            (ValueTy::Int, ValueTy::Int) => Some(ValueTy::Int),
            (ValueTy::Float, ValueTy::Float) => Some(ValueTy::Float),
            _ => None,
        },
        BinaryOp::Rem => match (left, right) {
            (ValueTy::Int, ValueTy::Int) => Some(ValueTy::Int),
            _ => None,
        },
        // §A.4: strict same-type value equality.
        BinaryOp::Eq | BinaryOp::Ne => (left == right).then_some(ValueTy::Bool),
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => match (left, right) {
            (ValueTy::Int, ValueTy::Int) | (ValueTy::Float, ValueTy::Float) => Some(ValueTy::Bool),
            _ => None,
        },
        BinaryOp::And | BinaryOp::Or => match (left, right) {
            (ValueTy::Bool, ValueTy::Bool) => Some(ValueTy::Bool),
            _ => None,
        },
    }
}

/// §A.7: a compound assignment is exactly the corresponding binary
/// operator applied to the target and the right-hand side.
fn compound_to_binary(op: CompoundOp) -> BinaryOp {
    match op {
        CompoundOp::Add => BinaryOp::Add,
        CompoundOp::Sub => BinaryOp::Sub,
        CompoundOp::Mul => BinaryOp::Mul,
        CompoundOp::Div => BinaryOp::Div,
        CompoundOp::Rem => BinaryOp::Rem,
    }
}

fn compound_op_symbol(op: CompoundOp) -> &'static str {
    match op {
        CompoundOp::Add => "+=",
        CompoundOp::Sub => "-=",
        CompoundOp::Mul => "*=",
        CompoundOp::Div => "/=",
        CompoundOp::Rem => "%=",
    }
}

fn binary_op_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Or => "||",
        BinaryOp::And => "&&",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
    }
}

fn unary_op_symbol(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
    }
}

/// True when executing `block` cannot fall off its end: some statement in
/// it transfers control (§2.14 plus the owner ruling). `while` never
/// counts; `if` without `else` does not count; `if`/`else` counts only
/// when both branches transfer.
fn block_returns(block: &Block) -> bool {
    block.stmts.iter().any(stmt_transfers)
}

fn stmt_transfers(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Return { .. } => true,
        StmtKind::Block(block) => block_returns(block),
        StmtKind::If {
            then_branch,
            else_branch,
            ..
        } => else_branch
            .as_ref()
            .is_some_and(|else_stmt| block_returns(then_branch) && stmt_transfers(else_stmt)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::check;
    use crate::diagnostics::Diagnostic;
    use crate::parse_source;
    use cme_core::Span;
    use cme_core::ast::{Block, Expr, ExprKind, Stmt, StmtKind, Type};

    const BASIC_CM: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../basic.cm"));

    fn check_source(source: &str) -> Vec<Diagnostic> {
        check(&parse_source(source).statements)
    }

    /// Parses + checks, asserting exactly one diagnostic whose message
    /// contains `substring` and whose span equals `span`.
    fn assert_error(source: &str, substring: &str, span: Span) {
        let diagnostics = check_source(source);
        assert_eq!(
            diagnostics.len(),
            1,
            "expected exactly one diagnostic: {diagnostics:#?}"
        );
        assert!(
            diagnostics[0].to_string().contains(substring),
            "message {:?} should contain {substring:?}",
            diagnostics[0].to_string()
        );
        assert_eq!(diagnostics[0].span(), span);
    }

    /// Span helper: `source[start..end]` located by substring.
    fn span_of(source: &str, needle: &str) -> Span {
        let start = source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in source"));
        Span::new(start, start + needle.len())
    }

    #[test]
    fn basic_cm_type_checks_clean() {
        let diagnostics = check_source(BASIC_CM);
        assert!(
            diagnostics.is_empty(),
            "check(parse_source(basic.cm)) must be empty: {diagnostics:#?}"
        );
    }

    /// The full pipeline the CLI runs: parse (lexer, recovery, post-parse
    /// validation) plus the type checker. A healthy program produces an
    /// empty list; a program with one defect produces exactly one
    /// diagnostic, no matter where in the tree the defect hides.
    fn full_pipeline(source: &str) -> Vec<Diagnostic> {
        let outcome = parse_source(source);
        let mut diagnostics = outcome.diagnostics;
        diagnostics.extend(check(&outcome.statements));
        diagnostics
    }

    #[test]
    fn mixed_logic_inside_a_function_is_reported_exactly_once() {
        // Regression for the silent-validator hotfix: `return a || b && c`
        // inside a function body used to produce NO diagnostic from the
        // full pipeline because the validator never descended into
        // function bodies (Appendix A §A.3 Rule 1 must fire there).
        let source = "bool f(bool a, bool b, bool c) {\nreturn a || b && c\n}\n";
        let diagnostics = full_pipeline(source);
        assert_eq!(
            diagnostics.len(),
            1,
            "expected exactly one diagnostic: {diagnostics:#?}"
        );
        assert_eq!(
            diagnostics[0].to_string(),
            "mixed && and || require parentheses"
        );
        assert_eq!(diagnostics[0].span(), span_of(source, "b && c"));
    }

    #[test]
    fn forward_references_and_recursion_resolve() {
        let source =
            "int main() {\nreturn helper(2) + main()\n}\nint helper(int n) {\nreturn n\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn string_concatenation_stringifies_per_a6() {
        // ": " + value with value: int is legal (§A.6).
        let source = "str label(int value) {\nstr s = \": \" + value\nreturn s\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn compound_assignment_with_stringification_is_legal() {
        // status += 100 ≡ status = status + 100 → str + int → str (§A.7, §A.6).
        let source = "int f() {\nstr status = \"start\"\nstatus += 100\nreturn 0\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn nested_scopes_with_distinct_names_are_legal() {
        let source = "int f() {\nint x = 1\nif (true) {\nint y = 2\ny = y + x\n}\nreturn x\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn if_else_where_both_branches_return_does_not_need_a_tail_return() {
        let source = "int f(bool b) {\nif (b) {\nreturn 1\n} else {\nreturn 2\n}\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn void_function_without_return_is_legal() {
        let source = "void f(int x) {\nx += 1\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn unknown_function() {
        let source = "int f() {\nreturn boom(1)\n}\n";
        assert_error(
            source,
            "unknown function `boom`",
            span_of(source, "boom(1)"),
        );
    }

    #[test]
    fn wrong_arity() {
        let source = "int add(int a, int b) {\nreturn a + b\n}\nint f() {\nreturn add(1)\n}\n";
        assert_error(
            source,
            "wrong number of arguments to `add`: expected 2, found 1",
            span_of(source, "add(1)"),
        );
    }

    #[test]
    fn wrong_argument_type() {
        let source =
            "int add(int a, int b) {\nreturn a + b\n}\nint f() {\nreturn add(1, \"x\")\n}\n";
        assert_error(
            source,
            "wrong argument type in call to `add`: expected `int`, found `str`",
            span_of(source, "\"x\""),
        );
    }

    #[test]
    fn undeclared_variable() {
        let source = "int f() {\nreturn x\n}\n";
        assert_error(source, "unknown name `x`", span_of(source, "x"));
    }

    #[test]
    fn use_before_declaration() {
        // A declaration enters scope after its own initializer: the `a`
        // initializer sits at byte 18 and is unknown at that point.
        let source = "int f() {\nint a = a\nreturn a\n}\n";
        assert_error(source, "unknown name `a`", Span::new(18, 19));
    }

    #[test]
    fn shadowing_is_forbidden() {
        let source = "int f() {\nint x = 1\nif (true) {\nint x = 2\n}\nreturn x\n}\n";
        assert_error(
            source,
            "declaration of `x` shadows a declaration in an enclosing scope",
            span_of(source, "int x = 2"),
        );
    }

    #[test]
    fn redeclaration_in_the_same_scope_is_forbidden() {
        let source = "int f() {\nint x = 1\nint x = 2\nreturn x\n}\n";
        assert_error(
            source,
            "duplicate declaration of `x`",
            span_of(source, "int x = 2"),
        );
    }

    #[test]
    fn duplicate_function() {
        let source = "int f() {\nreturn 1\n}\nint f() {\nreturn 2\n}\n";
        assert_error(
            source,
            "duplicate function `f`",
            span_of(source, "int f() {\nreturn 2\n}"),
        );
    }

    #[test]
    fn duplicate_function_body_is_not_checked() {
        // The first registration is authoritative: the duplicate is
        // reported, but its body is skipped, so a broken second body adds
        // no cascade on top of the duplicate error.
        let source = "int f() {\nreturn 1\n}\nint f() {\nreturn \"nope\"\n}\n";
        assert_error(
            source,
            "duplicate function `f`",
            span_of(source, "int f() {\nreturn \"nope\"\n}"),
        );
    }

    #[test]
    fn calls_to_a_duplicate_function_use_the_first_signature() {
        // f is int (first registration), so `f() + 1` is int and the
        // return type-checks. Had the second (str) signature won instead,
        // f() + 1 would crystallize to str and the return would mismatch.
        let source =
            "int f() {\nreturn 1\n}\nstr f() {\nreturn \"x\"\n}\nint main() {\nreturn f() + 1\n}\n";
        assert_error(
            source,
            "duplicate function `f`",
            span_of(source, "str f() {\nreturn \"x\"\n}"),
        );
    }

    #[test]
    fn duplicate_parameter() {
        let source = "int f(int a, int a) {\nreturn a\n}\n";
        assert_error(
            source,
            "duplicate parameter `a`",
            span_of(source, "int f(int a, int a) {\nreturn a\n}"),
        );
    }

    #[test]
    fn assignment_type_mismatch() {
        let source = "int f() {\nint x = 1\nx = 2.5\nreturn x\n}\n";
        assert_error(
            source,
            "type mismatch in assignment to `x`: expected `int`, found `float`",
            span_of(source, "x = 2.5"),
        );
    }

    #[test]
    fn assignment_target_must_be_declared() {
        let source = "int f() {\nx = 1\nreturn 0\n}\n";
        assert_error(source, "unknown name `x`", span_of(source, "x = 1"));
    }

    #[test]
    fn int_plus_float_is_rejected() {
        let source = "int f() {\ninfer x = 1 + 2.5\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot apply `+` to `int` and `float`",
            span_of(source, "1 + 2.5"),
        );
    }

    #[test]
    fn string_repetition_is_rejected() {
        let source = "int f() {\ninfer x = \"a\" * 3\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot apply `*` to `str` and `int`",
            span_of(source, "\"a\" * 3"),
        );
    }

    #[test]
    fn if_condition_must_be_bool() {
        let source = "int f() {\nif (1) {\nreturn 1\n}\nreturn 0\n}\n";
        assert_error(
            source,
            "if condition must be `bool`, found `int`",
            span_of(source, "1"),
        );
    }

    #[test]
    fn while_condition_must_be_bool() {
        let source = "int f() {\nwhile (2.5) {\nreturn 1\n}\nreturn 0\n}\n";
        assert_error(
            source,
            "while condition must be `bool`, found `float`",
            span_of(source, "2.5"),
        );
    }

    #[test]
    fn logical_not_requires_bool() {
        let source = "int f() {\ninfer x = !5\nreturn 0\n}\n";
        assert_error(source, "cannot apply `!` to `int`", span_of(source, "!5"));
    }

    #[test]
    fn unary_minus_rejects_bool() {
        let source = "int f() {\ninfer x = -true\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot apply `-` to `bool`",
            span_of(source, "-true"),
        );
    }

    #[test]
    fn cross_type_equality_is_rejected() {
        let source = "int f() {\ninfer x = 1 == \"1\"\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot apply `==` to `int` and `str`",
            span_of(source, "1 == \"1\""),
        );
    }

    #[test]
    fn missing_return() {
        let source = "int f() {\nint x = 1\n}\n";
        assert_error(
            source,
            "missing return in non-void function `f`",
            span_of(source, "{\nint x = 1\n}"),
        );
    }

    #[test]
    fn missing_return_when_if_has_no_else() {
        // Ruling: if without else does not count as returning.
        let source = "int f(bool b) {\nif (b) {\nreturn 1\n}\n}\n";
        assert_error(
            source,
            "missing return in non-void function `f`",
            span_of(source, "{\nif (b) {\nreturn 1\n}\n}"),
        );
    }

    #[test]
    fn missing_return_when_only_a_while_returns() {
        // Ruling: while never counts, no while (true) special-casing.
        let source = "int f() {\nwhile (true) {\nreturn 1\n}\n}\n";
        assert_error(
            source,
            "missing return in non-void function `f`",
            span_of(source, "{\nwhile (true) {\nreturn 1\n}\n}"),
        );
    }

    #[test]
    fn value_return_in_void_function() {
        let source = "void f() {\nreturn 1\n}\n";
        assert_error(
            source,
            "void function `f` cannot return a value",
            span_of(source, "return 1"),
        );
    }

    #[test]
    fn bare_return_in_non_void_function() {
        let source = "int f() {\nreturn\n}\n";
        assert_error(
            source,
            "non-void function `f` must return a value",
            span_of(source, "return"),
        );
    }

    #[test]
    fn wrong_return_type() {
        let source = "int f() {\nreturn \"nope\"\n}\n";
        assert_error(
            source,
            "wrong return type in `f`: expected `int`, found `str`",
            span_of(source, "return \"nope\""),
        );
    }

    #[test]
    fn void_call_as_value() {
        let source = "void g() {\nreturn\n}\nint f() {\nint x = g()\nreturn x\n}\n";
        assert_error(
            source,
            "type mismatch in declaration of `x`: expected `int`, found `void`",
            span_of(source, "int x = g()"),
        );
    }

    #[test]
    fn infer_from_void_call() {
        let source = "void g() {\nreturn\n}\nint f() {\ninfer x = g()\nreturn 0\n}\n";
        assert_error(
            source,
            "cannot infer type for 'x'; void initializer",
            span_of(source, "infer x = g()"),
        );
    }

    #[test]
    fn infer_crystallizes_to_the_initializer_type() {
        // §2.16: declared type = the initializer's type; assignments must
        // then match the crystallized type.
        let source = "int f() {\ninfer x = 1\nx = 2\nreturn x\n}\n";
        assert!(check_source(source).is_empty());
        let bad = "int f() {\ninfer x = 1\nx = 2.5\nreturn x\n}\n";
        assert_error(
            bad,
            "type mismatch in assignment to `x`: expected `int`, found `float`",
            span_of(bad, "x = 2.5"),
        );
    }

    #[test]
    fn top_level_statement() {
        let source = "int x = 1\n";
        assert_error(
            source,
            "only function declarations are allowed at top level",
            span_of(source, "int x = 1"),
        );
    }

    #[test]
    fn nested_function_declaration_is_rejected() {
        // Functions parse at any statement position (tolerance), but the
        // subset only allows top-level declarations; the nested one errors
        // and its body is never checked.
        let source = "int f() {\nint g() {\nreturn 1\n}\nreturn 1\n}\n";
        assert_error(
            source,
            "function declarations are only allowed at top level",
            span_of(source, "int g() {\nreturn 1\n}"),
        );
    }

    #[test]
    fn nested_function_declaration_in_a_block_is_rejected() {
        // Same rule inside a nested block: parse keeps the declaration,
        // the checker reports it where it appears.
        let source = "int f(bool b) {\nif (b) {\nint g() {\nreturn 1\n}\n}\nreturn 1\n}\n";
        assert_error(
            source,
            "function declarations are only allowed at top level",
            span_of(source, "int g() {\nreturn 1\n}"),
        );
    }

    #[test]
    fn calling_a_nested_function_still_reports_unknown_function() {
        // The nested declaration is illegal and never registers, so a call
        // to it resolves to no function; both facts are reported.
        let source = "int f() {\nint g() {\nreturn 1\n}\nreturn g()\n}\n";
        let diagnostics = check_source(source);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
        assert!(
            diagnostics.iter().any(|d| {
                d.to_string() == "function declarations are only allowed at top level"
            })
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.to_string().contains("unknown function `g`"))
        );
    }

    #[test]
    fn non_call_expression_statement() {
        // The parser only produces call expression statements; the ruling
        // is enforced at check time, so the AST is built by hand. The
        // statement must sit inside a function body, or the top-level rule
        // fires instead.
        let inner = Stmt::new(
            StmtKind::Expression {
                expr: Expr::new(ExprKind::IntLit(1), Span::new(12, 13)),
            },
            Span::new(12, 13),
        );
        let func = Stmt::new(
            StmtKind::FuncDecl {
                name: "f".to_string(),
                params: vec![],
                return_ty: Type::Void,
                body: Block {
                    span: Span::new(8, 15),
                    stmts: vec![inner],
                },
            },
            Span::new(0, 15),
        );
        let diagnostics = check(&[func]);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].to_string(),
            "expression statements must be function calls"
        );
        assert_eq!(diagnostics[0].span(), Span::new(12, 13));
    }

    #[test]
    fn infer_return_type_is_rejected_at_parse_level() {
        // Part 0 pins the parse-level error; the checker stays silent on
        // the poisoned signature (no cascade).
        let outcome = parse_source("infer f() {\nreturn 1\n}\n");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(
            outcome.diagnostics[0].to_string(),
            "`infer` is only valid for local declarations"
        );
        assert_eq!(outcome.diagnostics[0].span(), Span::new(0, 5));
        assert!(check(&outcome.statements).is_empty());
    }

    #[test]
    fn surviving_declaration_with_invalid_initializer_still_declares() {
        // The recovery design: `int x = )` keeps a VarDecl with an Invalid
        // initializer; the checker must not report anything for it.
        let outcome = parse_source("int f() {\nint x = )\nreturn x\n}\n");
        assert!(!outcome.diagnostics.is_empty());
        assert!(check(&outcome.statements).is_empty());
    }

    #[test]
    fn function_and_variable_namespaces_are_separate() {
        let source = "int f() {\nreturn 1\n}\nint main() {\nint f = f()\nreturn f\n}\n";
        assert!(check_source(source).is_empty());
    }

    #[test]
    fn checker_never_panics_on_the_stress_fixture() {
        let outcome = parse_source(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../boom.cm"
        )));
        // Runs to completion on any input; output volume is not pinned.
        let _ = check(&outcome.statements);
    }
}
