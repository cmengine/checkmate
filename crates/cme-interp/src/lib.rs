//! Tree-walking interpreter for the basic Checkmate subset.
//!
//! Normative sources: WHITEPAPER §2.10–§2.16 (declarations, functions,
//! control flow), §2.4 (overflow-checked scalar types), and Appendix A
//! (§A.4 operand typing, §A.5 evaluation semantics, §A.6 string
//! concatenation, §A.7 compound assignment).
//!
//! The interpreter is type-agnostic: the checker ran before it, so
//! [`StmtKind::VarDecl`] simply evaluates and binds, and declared types are
//! not tracked at runtime. The "refuse to run on diagnostics" gate is a
//! host concern (the CLI wires it); this crate never inspects diagnostics.
//! It depends only on `cme-core` — the spanned AST is the contract.
//!
//! Every abnormal outcome is a clean [`InterpError`] carrying a message and
//! a source [`Span`]: arithmetic overflow, integer division or remainder
//! by zero, and exceeding [`MAX_CALL_DEPTH`] terminate the invocation
//! cleanly (§2.4, §A.5) — never a panic. If the evaluator ever meets a
//! value of the wrong shape (a checker bug), it raises a runtime error the
//! same way instead of panicking.
//!
//! ```
//! use cme_core::ast::{
//!     Block, Expr, ExprKind, PrimitiveType, Span, Stmt, StmtKind, Type,
//! };
//! use cme_interp::{Interpreter, Value};
//!
//! let main = Stmt::new(
//!     StmtKind::FuncDecl {
//!         name: "main".to_string(),
//!         params: vec![],
//!         return_ty: Type::Prim(PrimitiveType::Int),
//!         body: Block {
//!             span: Span::new(0, 0),
//!             stmts: vec![Stmt::new(
//!                 StmtKind::Return {
//!                     value: Some(Expr::new(ExprKind::IntLit(41), Span::new(0, 0))),
//!                 },
//!                 Span::new(0, 0),
//!             )],
//!         },
//!     },
//!     Span::new(0, 0),
//! );
//! let statements = [main];
//! let interpreter = Interpreter::new(&statements);
//! assert_eq!(interpreter.invoke("main", &[]), Ok(Value::Int(41)));
//! ```
//!
//! The tree walker is the reference oracle the future bytecode VM will be
//! differential-tested against, so its observable behavior is exact:
//! plain Rust values, clones instead of sharing, short-circuit `&&`/`||`,
//! truncating integer division with the remainder taking the sign of the
//! dividend, and IEEE 754 float equality (NaN != NaN).

use std::collections::HashMap;
use std::fmt;

use cme_core::Span;
use cme_core::ast::{BinaryOp, Block, CompoundOp, Expr, ExprKind, Stmt, StmtKind, Type, UnaryOp};

/// The call-depth limit. A fixed constant per the current spec (§5.5 allows
/// host-configurable limits only in the future Engine API); native Rust
/// recursion is guarded by it, so runaway recursion terminates with a clean
/// [`InterpError`] instead of a stack overflow.
pub const MAX_CALL_DEPTH: usize = 1024;

/// A runtime value. Plain Rust types by design: no `Rc`, no copy-on-write —
/// sharing is deferred to the VM/runtime era and clones are fine.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// The absence of a value: what a void function returns (and what a
    /// value-less `return` yields).
    Void,
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Decimal for ints, shortest-round-trip for floats (Rust's
            // `{}` is exactly the §A.6 canonical form), raw text for str.
            Value::Int(value) => write!(formatter, "{value}"),
            Value::Float(value) => write!(formatter, "{value}"),
            Value::Str(text) => write!(formatter, "{text}"),
            Value::Bool(value) => write!(formatter, "{value}"),
            Value::Void => Ok(()),
        }
    }
}

impl Value {
    /// The scalar type name, for defensive error messages.
    fn kind_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::Bool(_) => "bool",
            Value::Void => "void",
        }
    }
}

/// A runtime error: what went wrong, and where. Terminates the invocation
/// cleanly; the host renders the span.
#[derive(Debug, Clone, PartialEq)]
pub struct InterpError {
    pub message: String,
    pub span: Span,
}

impl InterpError {
    fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }
}

/// A program ready to invoke: the function declarations of a parsed (and,
/// in the host's pipeline, checked) statement list, collected into a name
/// map. The first registration of a name wins, matching the checker.
pub struct Interpreter<'a> {
    functions: HashMap<&'a str, &'a Stmt>,
}

impl<'a> Interpreter<'a> {
    /// Collects every top-level `FuncDecl` into the function table.
    pub fn new(statements: &'a [Stmt]) -> Self {
        let mut functions = HashMap::new();
        for statement in statements {
            if let StmtKind::FuncDecl { name, .. } = &statement.kind {
                functions.entry(name.as_str()).or_insert(statement);
            }
        }
        Self { functions }
    }

    /// Invokes `name` with `args` (bound by value, cloned). Errors on an
    /// unknown function or an arity mismatch — defensively, since the
    /// checker normally guarantees both.
    pub fn invoke(&self, name: &str, args: &[Value]) -> Result<Value, InterpError> {
        let Some(&declaration) = self.functions.get(name) else {
            return Err(InterpError::new(
                format!("unknown function `{name}`"),
                Span::new(0, 0),
            ));
        };
        let mut runner = Runner {
            functions: &self.functions,
            scopes: Vec::new(),
            depth: 0,
        };
        runner.call_function(declaration, name, args.to_vec(), Span::new(0, 0))
    }
}

/// The control-flow signal that propagates up through blocks, `if`, and
/// `while` until a function boundary turns `Return` into a result.
enum Flow {
    Normal,
    Return(Value),
}

/// Mutable execution state for one invocation: the scope stack and the
/// call-depth counter. The function table is borrowed from the
/// [`Interpreter`].
struct Runner<'env, 'a> {
    functions: &'env HashMap<&'a str, &'a Stmt>,
    scopes: Vec<HashMap<String, Value>>,
    depth: usize,
}

impl<'env, 'a> Runner<'env, 'a> {
    /// Enters a function: depth guard, arity check, parameter binding by
    /// value in the function frame, body execution, and `Flow` conversion.
    fn call_function(
        &mut self,
        declaration: &'a Stmt,
        name: &str,
        args: Vec<Value>,
        call_span: Span,
    ) -> Result<Value, InterpError> {
        let StmtKind::FuncDecl {
            params,
            return_ty,
            body,
            ..
        } = &declaration.kind
        else {
            return Err(InterpError::new(
                "internal error: not a function declaration",
                call_span,
            ));
        };
        if args.len() != params.len() {
            return Err(InterpError::new(
                format!(
                    "wrong number of arguments to `{name}`: expected {}, found {}",
                    params.len(),
                    args.len()
                ),
                call_span,
            ));
        }
        if self.depth >= MAX_CALL_DEPTH {
            return Err(InterpError::new(
                format!("call depth limit of {MAX_CALL_DEPTH} exceeded"),
                call_span,
            ));
        }

        self.depth += 1;
        // The function frame holds the parameters together with the body's
        // top-level declarations; each nested block execution gets its own
        // frame below it.
        let mut frame = HashMap::with_capacity(params.len());
        for (param, value) in params.iter().zip(args) {
            frame.insert(param.name.clone(), value);
        }
        self.scopes.push(frame);
        let flow = self.exec_stmts(&body.stmts);
        self.scopes.pop();
        self.depth -= 1;

        match flow? {
            Flow::Return(value) => Ok(value),
            // Falling off the end of a void function is normal; falling
            // off a non-void one is a checker bug raised defensively.
            Flow::Normal if *return_ty == Type::Void => Ok(Value::Void),
            Flow::Normal => Err(InterpError::new(
                format!("non-void function `{name}` fell off the end without returning a value"),
                declaration.span,
            )),
        }
    }

    /// Executes statements in the current frame until one returns.
    fn exec_stmts(&mut self, statements: &'a [Stmt]) -> Result<Flow, InterpError> {
        for statement in statements {
            match self.exec_stmt(statement)? {
                Flow::Normal => {}
                flow @ Flow::Return(_) => return Ok(flow),
            }
        }
        Ok(Flow::Normal)
    }

    /// Executes a block: one fresh scope frame per execution. A `while`
    /// body is executed as a new block every iteration, so a `VarDecl`
    /// inside it rebinds in a fresh frame each time.
    fn exec_block(&mut self, block: &'a Block) -> Result<Flow, InterpError> {
        self.scopes.push(HashMap::new());
        let flow = self.exec_stmts(&block.stmts);
        self.scopes.pop();
        flow
    }

    fn exec_stmt(&mut self, stmt: &'a Stmt) -> Result<Flow, InterpError> {
        match &stmt.kind {
            // A call statement evaluates and discards its result — void or
            // not, silently (owner ruling).
            StmtKind::Expression { expr } => {
                self.eval(expr)?;
                Ok(Flow::Normal)
            }
            // Type-agnostic: the checker ran; evaluate and bind.
            StmtKind::VarDecl { name, expr, .. } => {
                let value = self.eval(expr)?;
                self.declare_variable(name, value, stmt.span)?;
                Ok(Flow::Normal)
            }
            StmtKind::Assign { name, expr } => {
                let value = self.eval(expr)?;
                self.write_variable(name, value, stmt.span)?;
                Ok(Flow::Normal)
            }
            // §A.7: `x op= e` is exactly `x = x op e`.
            StmtKind::CompoundAssign { target, op, expr } => {
                let current = self.read_variable(target, stmt.span)?;
                let right = self.eval(expr)?;
                let result =
                    self.apply_binary(compound_to_binary(*op), current, right, stmt.span)?;
                self.write_variable(target, result, stmt.span)?;
                Ok(Flow::Normal)
            }
            StmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let condition = self.eval(cond)?;
                let kind = condition.kind_name();
                let Value::Bool(take_then) = condition else {
                    return Err(InterpError::new(
                        format!("if condition must be `bool`, found `{kind}`"),
                        cond.span,
                    ));
                };
                if take_then {
                    self.exec_block(then_branch)
                } else if let Some(else_stmt) = else_branch {
                    self.exec_stmt(else_stmt)
                } else {
                    Ok(Flow::Normal)
                }
            }
            StmtKind::While { cond, body } => {
                loop {
                    let condition = self.eval(cond)?;
                    let kind = condition.kind_name();
                    let Value::Bool(keep_going) = condition else {
                        return Err(InterpError::new(
                            format!("while condition must be `bool`, found `{kind}`"),
                            cond.span,
                        ));
                    };
                    if !keep_going {
                        break;
                    }
                    if let Flow::Return(value) = self.exec_block(body)? {
                        return Ok(Flow::Return(value));
                    }
                }
                Ok(Flow::Normal)
            }
            StmtKind::Return { value } => {
                let value = match value {
                    Some(expr) => self.eval(expr)?,
                    None => Value::Void,
                };
                Ok(Flow::Return(value))
            }
            StmtKind::Block(block) => self.exec_block(block),
            // Only reachable through a hand-built (unchecked) tree; the
            // checker rejects both shapes.
            StmtKind::FuncDecl { .. } => Err(InterpError::new(
                "function declarations are only allowed at top level",
                stmt.span,
            )),
            StmtKind::Invalid { .. } => Err(InterpError::new(
                "cannot execute an invalid statement",
                stmt.span,
            )),
        }
    }

    fn eval(&mut self, expr: &'a Expr) -> Result<Value, InterpError> {
        match &expr.kind {
            ExprKind::IntLit(value) => Ok(Value::Int(*value)),
            ExprKind::FloatLit(value) => Ok(Value::Float(*value)),
            ExprKind::StrLit(text) => Ok(Value::Str(text.clone())),
            ExprKind::BoolLit(value) => Ok(Value::Bool(*value)),
            ExprKind::Ident(name) => self.read_variable(name, expr.span),
            ExprKind::Paren { expr: inner } => self.eval(inner),
            ExprKind::Unary { op, expr: inner } => self.eval_unary(*op, inner, expr.span),
            ExprKind::Binary { op, lhs, rhs } => self.eval_binary(*op, lhs, rhs, expr.span),
            ExprKind::Call { name, args } => self.eval_call(name, args, expr.span),
            // Only reachable through a hand-built (unchecked) tree: the
            // parser gates execution on a clean diagnostics list.
            ExprKind::Invalid { .. } => Err(InterpError::new(
                "cannot evaluate an invalid expression",
                expr.span,
            )),
        }
    }

    fn eval_unary(
        &mut self,
        op: UnaryOp,
        inner: &'a Expr,
        span: Span,
    ) -> Result<Value, InterpError> {
        let value = self.eval(inner)?;
        match (op, value) {
            (UnaryOp::Neg, Value::Int(a)) => a
                .checked_neg()
                .map(Value::Int)
                .ok_or_else(|| InterpError::new("integer overflow in `-`", span)),
            (UnaryOp::Neg, Value::Float(a)) => Ok(Value::Float(-a)),
            (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
            (op, value) => Err(InterpError::new(
                format!(
                    "cannot apply `{}` to `{}`",
                    unary_symbol(op),
                    value.kind_name()
                ),
                span,
            )),
        }
    }

    fn eval_binary(
        &mut self,
        op: BinaryOp,
        lhs: &'a Expr,
        rhs: &'a Expr,
        span: Span,
    ) -> Result<Value, InterpError> {
        match op {
            // §A.5: short-circuiting — the right operand is evaluated only
            // when the left one does not decide the result.
            BinaryOp::And => {
                let left = self.eval(lhs)?;
                let left_kind = left.kind_name();
                let Value::Bool(l) = left else {
                    return Err(InterpError::new(
                        format!("cannot apply `&&` to `{left_kind}` and `bool`"),
                        span,
                    ));
                };
                if !l {
                    return Ok(Value::Bool(false));
                }
                match self.eval(rhs)? {
                    Value::Bool(r) => Ok(Value::Bool(r)),
                    right => Err(InterpError::new(
                        format!("cannot apply `&&` to `bool` and `{}`", right.kind_name()),
                        span,
                    )),
                }
            }
            BinaryOp::Or => {
                let left = self.eval(lhs)?;
                let left_kind = left.kind_name();
                let Value::Bool(l) = left else {
                    return Err(InterpError::new(
                        format!("cannot apply `||` to `{left_kind}` and `bool`"),
                        span,
                    ));
                };
                if l {
                    return Ok(Value::Bool(true));
                }
                match self.eval(rhs)? {
                    Value::Bool(r) => Ok(Value::Bool(r)),
                    right => Err(InterpError::new(
                        format!("cannot apply `||` to `bool` and `{}`", right.kind_name()),
                        span,
                    )),
                }
            }
            _ => {
                let left = self.eval(lhs)?;
                let right = self.eval(rhs)?;
                self.apply_binary(op, left, right, span)
            }
        }
    }

    fn eval_call(
        &mut self,
        name: &str,
        args: &'a [Expr],
        span: Span,
    ) -> Result<Value, InterpError> {
        let Some(&declaration) = self.functions.get(name) else {
            return Err(InterpError::new(format!("unknown function `{name}`"), span));
        };
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval(arg)?);
        }
        self.call_function(declaration, name, values, span)
    }

    /// Strict binary arithmetic/comparison/equality on already-evaluated
    /// operands. `&&` and `||` never reach this path (they short-circuit).
    fn apply_binary(
        &mut self,
        op: BinaryOp,
        left: Value,
        right: Value,
        span: Span,
    ) -> Result<Value, InterpError> {
        let symbol = binary_symbol(op);
        let overflow = || InterpError::new(format!("integer overflow in `{symbol}`"), span);
        let name_l = left.kind_name();
        let name_r = right.kind_name();
        let mismatch = || {
            InterpError::new(
                format!("cannot apply `{symbol}` to `{name_l}` and `{name_r}`"),
                span,
            )
        };

        match op {
            BinaryOp::Add => match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    a.checked_add(b).map(Value::Int).ok_or_else(overflow)
                }
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                // §A.6: when either operand is a str, + concatenates with
                // the other side's canonical string form.
                (Value::Str(mut text), other) => {
                    let Some(suffix) = stringify(&other) else {
                        return Err(mismatch());
                    };
                    text.push_str(&suffix);
                    Ok(Value::Str(text))
                }
                (other, Value::Str(text)) => {
                    let Some(prefix) = stringify(&other) else {
                        return Err(mismatch());
                    };
                    Ok(Value::Str(prefix + &text))
                }
                _ => Err(mismatch()),
            },
            BinaryOp::Sub => match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    a.checked_sub(b).map(Value::Int).ok_or_else(overflow)
                }
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Mul => match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    a.checked_mul(b).map(Value::Int).ok_or_else(overflow)
                }
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Div => match (left, right) {
                // §A.5: integer division truncates toward zero; division
                // by zero terminates the invocation (i64::MIN / -1 is the
                // one overflowing case, caught by checked_div).
                (Value::Int(a), Value::Int(b)) => {
                    if b == 0 {
                        Err(InterpError::new("integer division by zero", span))
                    } else {
                        a.checked_div(b).map(Value::Int).ok_or_else(overflow)
                    }
                }
                // Float division is ordinary IEEE 754: a zero divisor is
                // inf/NaN, not an error.
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Rem => match (left, right) {
                // §A.5: remainder of truncated division, sign of the
                // dividend: -7 % 2 is -1, 7 % -2 is 1. Int-only (§A.4).
                (Value::Int(a), Value::Int(b)) => {
                    if b == 0 {
                        Err(InterpError::new("integer remainder by zero", span))
                    } else {
                        a.checked_rem(b).map(Value::Int).ok_or_else(overflow)
                    }
                }
                _ => Err(mismatch()),
            },
            // §A.4: strict same-type value equality; float follows IEEE 754
            // (NaN == NaN is false). Cross types are a checker bug.
            BinaryOp::Eq => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a == b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a == b)),
                (Value::Str(a), Value::Str(b)) => Ok(Value::Bool(a == b)),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a == b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Ne => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a != b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a != b)),
                (Value::Str(a), Value::Str(b)) => Ok(Value::Bool(a != b)),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a != b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Lt => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Le => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Gt => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Ge => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
                _ => Err(mismatch()),
            },
            // Unreachable: the short-circuit path in eval_binary handles
            // both before operands are strictly evaluated.
            BinaryOp::And | BinaryOp::Or => Err(InterpError::new(
                "internal error: logical operators must short-circuit",
                span,
            )),
        }
    }

    /// Reads the nearest binding with `name`.
    fn read_variable(&self, name: &str, span: Span) -> Result<Value, InterpError> {
        for frame in self.scopes.iter().rev() {
            if let Some(value) = frame.get(name) {
                return Ok(value.clone());
            }
        }
        Err(InterpError::new(format!("unknown name `{name}`"), span))
    }

    /// Mutates the nearest binding with `name`.
    fn write_variable(&mut self, name: &str, value: Value, span: Span) -> Result<(), InterpError> {
        for frame in self.scopes.iter_mut().rev() {
            if frame.contains_key(name) {
                frame.insert(name.to_string(), value);
                return Ok(());
            }
        }
        Err(InterpError::new(
            format!("assignment to undeclared name `{name}`"),
            span,
        ))
    }

    /// Binds `name` in the innermost frame.
    fn declare_variable(
        &mut self,
        name: &str,
        value: Value,
        span: Span,
    ) -> Result<(), InterpError> {
        match self.scopes.last_mut() {
            Some(frame) => {
                frame.insert(name.to_string(), value);
                Ok(())
            }
            None => Err(InterpError::new("internal error: no active scope", span)),
        }
    }
}

/// §A.6 canonical string form, used only inside concatenation. `Void` is
/// not a value and cannot be stringified.
fn stringify(value: &Value) -> Option<String> {
    match value {
        Value::Void => None,
        other => Some(other.to_string()),
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

fn binary_symbol(op: BinaryOp) -> &'static str {
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

fn unary_symbol(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
    }
}

#[cfg(test)]
mod tests {
    use super::{InterpError, Interpreter, MAX_CALL_DEPTH, Value};
    use cme_core::Span;

    /// The full pipeline with the same gate a host applies: the source must
    /// parse AND check clean before the interpreter runs.
    fn run_main(source: &str) -> Result<Value, InterpError> {
        let outcome = cme_compiler::parse_source(source);
        assert!(
            outcome.is_clean(),
            "test source must parse clean: {:?}",
            outcome.diagnostics
        );
        let diagnostics = cme_compiler::check::check(&outcome.statements);
        assert!(
            diagnostics.is_empty(),
            "test source must check clean: {diagnostics:?}"
        );
        Interpreter::new(&outcome.statements).invoke("main", &[])
    }

    /// Parse-only pipeline for defensive pins: the source parses clean but
    /// the checker would reject it, so the interpreter must raise a clean
    /// error (never panic) when it meets the bad shape.
    fn run_ungated(source: &str) -> Result<Value, InterpError> {
        let outcome = cme_compiler::parse_source(source);
        assert!(
            outcome.is_clean(),
            "test source must parse clean: {:?}",
            outcome.diagnostics
        );
        Interpreter::new(&outcome.statements).invoke("main", &[])
    }

    fn ok(source: &str) -> Value {
        run_main(source).expect("test program should run to completion")
    }

    /// Span helper: `source[start..end]` located by substring.
    fn span_of(source: &str, needle: &str) -> Span {
        let start = source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in source"));
        Span::new(start, start + needle.len())
    }

    #[test]
    fn value_display_matches_canonical_forms() {
        assert_eq!(Value::Int(-42).to_string(), "-42");
        assert_eq!(Value::Int(0).to_string(), "0");
        assert_eq!(Value::Float(3.75).to_string(), "3.75");
        // Shortest round-trip: trailing ".0" is dropped, imprecision is
        // shown exactly.
        assert_eq!(Value::Float(3.0).to_string(), "3");
        assert_eq!(Value::Float(0.1 + 0.2).to_string(), "0.30000000000000004");
        assert_eq!(Value::Bool(true).to_string(), "true");
        assert_eq!(Value::Bool(false).to_string(), "false");
        assert_eq!(Value::Str("hp".to_string()).to_string(), "hp");
        assert_eq!(Value::Void.to_string(), "");
    }

    #[test]
    fn integer_division_and_remainder_truncate_toward_zero() {
        // §A.5: 7 / 2 is 3, -7 / 2 is -3, -7 % 2 is -1, 7 % -2 is 1.
        assert_eq!(ok("int main() {\nreturn 7 / 2\n}\n"), Value::Int(3));
        assert_eq!(ok("int main() {\nreturn -7 / 2\n}\n"), Value::Int(-3));
        assert_eq!(ok("int main() {\nreturn -7 % 2\n}\n"), Value::Int(-1));
        assert_eq!(ok("int main() {\nreturn 7 % -2\n}\n"), Value::Int(1));
    }

    #[test]
    fn logical_operators_short_circuit() {
        // `boom()` errors at runtime if it is ever evaluated; the programs
        // only complete when && and || skip the right operand.
        let source = "bool boom() {\nreturn 1 / 0 == 1\n}\nint main() {\nbool both = false && boom()\nbool either = true || boom()\nif (both) {\nreturn 1\n}\nif (!either) {\nreturn 2\n}\nreturn 0\n}\n";
        assert_eq!(ok(source), Value::Int(0));

        // The right operand IS evaluated when the left one does not
        // decide: the error from boom() propagates.
        let source = "bool boom() {\nreturn 1 / 0 == 1\n}\nint main() {\nbool trapped = true && boom()\nreturn 0\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer division by zero");
    }

    #[test]
    fn string_concatenation_uses_canonical_forms() {
        // §A.6 examples, including the left-associativity consequences.
        assert_eq!(
            ok("str main() {\nreturn \"HP: \" + 100\n}\n"),
            Value::Str("HP: 100".to_string())
        );
        assert_eq!(
            ok("str main() {\nreturn \"ok: \" + true\n}\n"),
            Value::Str("ok: true".to_string())
        );
        assert_eq!(
            ok("str main() {\nreturn 1.5 + \"x\"\n}\n"),
            Value::Str("1.5x".to_string())
        );
        assert_eq!(
            ok("str main() {\nreturn \"a\" + 1 + 2\n}\n"),
            Value::Str("a12".to_string())
        );
        assert_eq!(
            ok("str main() {\nreturn 1 + 2 + \"a\"\n}\n"),
            Value::Str("3a".to_string())
        );
        // A str on either side stringifies the other; never the reverse.
        assert_eq!(
            ok("str main() {\nreturn 100 + \"!\"\n}\n"),
            Value::Str("100!".to_string())
        );
    }

    #[test]
    fn integer_division_and_remainder_by_zero_are_runtime_errors() {
        let source = "int main() {\nreturn 1 / 0\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer division by zero");
        assert_eq!(error.span, span_of(source, "1 / 0"));

        let source = "int main() {\nreturn 1 % 0\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer remainder by zero");
        assert_eq!(error.span, span_of(source, "1 % 0"));

        // Float division by zero is ordinary IEEE 754: not an error.
        assert_eq!(
            ok("int main() {\nfloat inf = 1.0 / 0.0\nif (inf > 0.0) {\nreturn 1\n}\nreturn 0\n}\n"),
            Value::Int(1)
        );
    }

    #[test]
    fn arithmetic_overflow_terminates_the_invocation() {
        // The pinned case: i64::MAX + 1 via compound assignment.
        let source = "int main() {\nint x = 9223372036854775807\nx += 1\nreturn x\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `+`");
        assert_eq!(error.span, span_of(source, "x += 1"));

        // Negating i64::MIN.
        let source = "int main() {\nint min = -9223372036854775807 - 1\nreturn -min\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `-`");

        // i64::MIN / -1 is the one overflowing division.
        let source = "int main() {\nint min = -9223372036854775807 - 1\nreturn min / -1\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `/`");

        // i64::MIN % -1 overflows the remainder too; checked, never a panic.
        let source = "int main() {\nint min = -9223372036854775807 - 1\nreturn min % -1\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `%`");

        // Multiplication wraps into checked territory as well.
        let source = "int main() {\nint big = 3037000500\nbig *= big\nreturn big\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "integer overflow in `*`");
    }

    #[test]
    fn while_bodies_rebind_fresh_each_iteration() {
        // One fresh frame per loop iteration: `per` is (re)declared in a
        // new frame every time the body executes, with the initializer
        // evaluated per iteration (0, 10, 20).
        let source = "int main() {\nint i = 0\nint total = 0\nwhile (i < 3) {\nint per = i * 10\ntotal = total + per\ni += 1\n}\nreturn total\n}\n";
        assert_eq!(ok(source), Value::Int(30));
    }

    #[test]
    fn return_early_from_inside_a_while() {
        let source = "int main() {\nint i = 0\nwhile (true) {\nif (i == 3) {\nreturn i\n}\ni += 1\n}\nreturn 0\n}\n";
        assert_eq!(ok(source), Value::Int(3));
    }

    #[test]
    fn recursion_computes_fibonacci() {
        let source = "int fib(int n) {\nif (n <= 1) {\nreturn n\n}\nreturn fib(n - 1) + fib(n - 2)\n}\nint main() {\nreturn fib(10)\n}\n";
        assert_eq!(ok(source), Value::Int(55));
    }

    #[test]
    fn infinite_recursion_hits_the_depth_limit_cleanly() {
        let source = "int spin() {\nreturn spin()\n}\nint main() {\nreturn spin()\n}\n";
        // 1024 nested CME frames each occupy several native Rust frames in
        // a debug build, more than a default test thread's stack can hold.
        // Run the invocation on a dedicated thread with a generous stack so
        // the depth guard — not the native stack — is what stops the
        // recursion. (A host embedding the interpreter must similarly
        // provide adequate stack for `MAX_CALL_DEPTH`-deep recursion, or
        // configure a lower limit once the Engine API allows it.)
        let handle = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || run_main(source))
            .expect("spawn the depth-limit thread");
        let result = handle.join().expect("depth-limit thread must not panic");
        let error = result.unwrap_err();
        assert_eq!(
            error.message,
            format!("call depth limit of {MAX_CALL_DEPTH} exceeded")
        );
    }

    #[test]
    fn call_statements_discard_results_silently() {
        // A call statement may discard a non-void result (owner ruling).
        let source = "int double(int value) {\nreturn value * 2\n}\nint main() {\ndouble(21)\nreturn 21\n}\n";
        assert_eq!(ok(source), Value::Int(21));

        // Void calls run their body and return nothing printable.
        let source = "void emit(int value) {\nint doubled = value * 2\n}\nint main() {\nemit(21)\nreturn 5\n}\n";
        assert_eq!(ok(source), Value::Int(5));
    }

    #[test]
    fn float_equality_follows_ieee_754() {
        // NaN == NaN is false; NaN arises from 0.0 / 0.0.
        let source = "int main() {\nfloat nan = 0.0 / 0.0\nif (nan == nan) {\nreturn 1\n}\nif (nan != nan) {\nreturn 2\n}\nreturn 0\n}\n";
        assert_eq!(ok(source), Value::Int(2));
    }

    #[test]
    fn invoke_rejects_unknown_functions_and_arity_mismatches() {
        let source = "int add(int a, int b) {\nreturn a + b\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let interpreter = Interpreter::new(&outcome.statements);

        let error = interpreter.invoke("missing", &[]).unwrap_err();
        assert_eq!(error.message, "unknown function `missing`");

        let error = interpreter.invoke("add", &[Value::Int(1)]).unwrap_err();
        assert_eq!(
            error.message,
            "wrong number of arguments to `add`: expected 2, found 1"
        );

        assert_eq!(
            interpreter.invoke("add", &[Value::Int(1), Value::Int(2)]),
            Ok(Value::Int(3))
        );
    }

    #[test]
    fn wrong_condition_shapes_raise_errors_not_panics() {
        // Checker-bug shapes: an int condition must produce a clean
        // InterpError, never a panic.
        let source = "int main() {\nif (7) {\nreturn 1\n}\nreturn 0\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "if condition must be `bool`, found `int`");

        let source = "int main() {\nwhile (7) {\nreturn 1\n}\nreturn 0\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "while condition must be `bool`, found `int`");
    }

    #[test]
    fn falling_off_a_non_void_function_raises_an_error() {
        // The checker's structural return analysis would reject this; the
        // interpreter defends itself anyway.
        let source = "int leak() {\nint unused = 1\n}\nint main() {\nreturn leak()\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(
            error.message,
            "non-void function `leak` fell off the end without returning a value"
        );
    }

    #[test]
    fn assignment_to_undeclared_name_is_a_defensive_error() {
        let source = "int main() {\nghost = 1\nreturn 0\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "assignment to undeclared name `ghost`");
    }

    #[test]
    fn calls_to_unknown_functions_are_defensive_errors() {
        let source = "int main() {\nreturn missing(1)\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "unknown function `missing`");

        let source = "int main() {\nreturn helper()\n}\nint helper() {\nreturn missing()\n}\n";
        let error = run_ungated(source).unwrap_err();
        assert_eq!(error.message, "unknown function `missing`");
    }
}
