//! Tree-walking interpreter for the full Checkmate language surface.
//!
//! Normative sources: WHITEPAPER §2.6–§2.16 (declarations, structs, enums,
//! generics, functions, control flow, pattern matching), §2.4 (overflow-
//! checked scalar types), §2.8 (option / result and `?`), §10.4 (impl
//! blocks and qualified member calls), §11 (arrays and
//! maps), and Appendix A (§A.4 operand typing, §A.5 evaluation semantics,
//! §A.6 string concatenation, §A.7 compound assignment).
//!
//! The interpreter is type-agnostic: the checker ran before it, so
//! [`StmtKind::VarDecl`] simply evaluates and binds, and declared types are
//! not tracked at runtime. The "refuse to run on diagnostics" gate is a
//! host concern (the CLI wires it); this crate never inspects diagnostics.
//! It depends only on `cme-core` — the spanned AST is the contract.
//!
//! Every abnormal outcome is a clean [`InterpError`] carrying a message and
//! a source [`Span`]: arithmetic overflow, integer division or remainder
//! by zero, out-of-bounds indexing, missing map keys, and exceeding
//! [`MAX_CALL_DEPTH`] terminate the invocation cleanly (§2.4, §A.5) —
//! never a panic. If the evaluator ever meets a value of the wrong shape
//! (a checker bug), it raises a runtime error the same way instead of
//! panicking.
//!
//! Runtime identity (§2.13 value semantics): assignment and parameter
//! passing clone — struct fields, array elements, and map entries included.
//! Equality is structural for structs, enums, arrays, and maps (§A.4): a
//! struct equals a struct with the same type name and equal fields, an
//! enum value equals the same variant with equal payloads, and maps
//! compare order-insensitively. The static type equality the checker
//! enforced upstream makes the runtime comparison safe.
//!
//! The `?` operator (§2.8) unwraps `Ok` and returns the enclosing
//! function early with the `Err` payload — the nearest function boundary
//! converts that control signal into an ordinary return value, so `?`
//! inside a called function propagates only through that call's result.
//!
//! ```
//! use cme_core::ast::{
//!     Block, Expr, ExprKind, LValue, PrimitiveType, Span, Stmt, StmtKind, Type,
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

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::time::Instant;

use cme_core::Span;
use cme_core::ast::{
    BinaryOp, Block, CallArg, CompoundOp, Expr, ExprKind, InterpPart, LValue, Pattern, Stmt,
    StmtKind, Type, UnaryOp,
};

/// The call-depth bound. [`MAX_CALL_DEPTH`] is the interpreter default; a
/// host embedding the interpreter can lower it per interpreter via
/// [`Interpreter::with_call_depth_limit`] (WHITEPAPER §5.5, §13.1).
/// Native Rust recursion is guarded by it, so runaway recursion terminates
/// with a clean [`InterpError`] instead of a stack overflow. A host running
/// programs that legitimately recurse near [`MAX_CALL_DEPTH`] must provide
/// adequate native stack (a dedicated thread), or configure a lower limit.
pub const MAX_CALL_DEPTH: usize = 1024;

/// A runtime value. Plain Rust types by design: no `Rc`, no copy-on-write —
/// sharing is deferred to the VM/runtime era and clones are fine (§2.13:
/// values behave as independently owned).
///
/// Struct fields are stored in declaration order and enum payloads in
/// variant order, so equality compares positionally. Maps keep insertion
/// order and compare order-insensitively.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// The absence of a value: what a void function returns (and what a
    /// value-less `return` yields).
    Void,
    Struct {
        name: String,
        fields: Vec<(String, Value)>,
    },
    Enum {
        name: String,
        variant: String,
        payload: Vec<Value>,
    },
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
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
            // CMON-style structural display (§11.1).
            Value::Struct { name, fields } => {
                let inner: Vec<String> = fields
                    .iter()
                    .map(|(field, value)| format!("{field}: {value}"))
                    .collect();
                write!(formatter, "{name}({})", inner.join(", "))
            }
            Value::Enum {
                name,
                variant,
                payload,
            } => {
                let inner: Vec<String> = payload.iter().map(|v| v.to_string()).collect();
                write!(formatter, "{name}.{variant}({})", inner.join(", "))
            }
            Value::Array(elements) => {
                let inner: Vec<String> = elements.iter().map(|v| v.to_string()).collect();
                write!(formatter, "[{}]", inner.join(", "))
            }
            Value::Map(entries) => {
                let inner: Vec<String> = entries
                    .iter()
                    .map(|(key, value)| format!("{key}: {value}"))
                    .collect();
                write!(formatter, "{{{}}}", inner.join(", "))
            }
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Void, Value::Void) => true,
            // §A.4 structural equality: same type name and equal fields
            // (both normalized to declaration order at construction).
            (
                Value::Struct { name, fields },
                Value::Struct {
                    name: other_name,
                    fields: other_fields,
                },
            ) => name == other_name && fields == other_fields,
            (
                Value::Enum {
                    name,
                    variant,
                    payload,
                },
                Value::Enum {
                    name: other_name,
                    variant: other_variant,
                    payload: other_payload,
                },
            ) => name == other_name && variant == other_variant && payload == other_payload,
            (Value::Array(a), Value::Array(b)) => a == b,
            // Maps compare order-insensitively (set semantics).
            (Value::Map(a), Value::Map(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, value)| b.iter().any(|(k, v)| key == k && value == v))
            }
            _ => false,
        }
    }
}

impl Value {
    /// The scalar type name, for defensive error messages.
    fn kind_name(&self) -> String {
        match self {
            Value::Int(_) => "int".into(),
            Value::Float(_) => "float".into(),
            Value::Str(_) => "str".into(),
            Value::Bool(_) => "bool".into(),
            Value::Void => "void".into(),
            Value::Struct { name, .. } => format!("struct {name}"),
            Value::Enum { name, .. } => format!("enum {name}"),
            Value::Array(_) => "array".into(),
            Value::Map(_) => "map".into(),
        }
    }

    /// The `int` payload, or `None` for any other kind. The scalar
    /// accessors let hosts unpack results without pattern-matching the
    /// whole enum (WHITEPAPER §13.1).
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The `float` payload, or `None` for any other kind.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(value) => Some(*value),
            _ => None,
        }
    }

    /// The `bool` payload, or `None` for any other kind.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The `str` payload, or `None` for any other kind.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(value) => Some(value),
            _ => None,
        }
    }

    /// The array elements, or `None` for any other kind.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(elements) => Some(elements),
            _ => None,
        }
    }

    /// The map entries in insertion order, or `None` for any other kind.
    pub fn as_map(&self) -> Option<&[(Value, Value)]> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// True for the absence value a `void` function returns.
    pub fn is_void(&self) -> bool {
        matches!(self, Value::Void)
    }
}

/// Host-side conversions: building call arguments from plain Rust values
/// without hand-constructing variants (WHITEPAPER §13.1).
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Str(value)
    }
}

/// A runtime error: what went wrong, and where. Terminates the invocation
/// cleanly; the host renders the span.
///
/// `control` marks the one non-error signal: the `?` operator's early
/// return (§2.8). It is consumed at the nearest function boundary and
/// never escapes to the host.
#[derive(Debug, Clone, PartialEq)]
pub struct InterpError {
    pub message: String,
    pub span: Span,
    /// Coarse classification used by embedding layers (WHITEPAPER §5.5, §13):
    /// limit-family failures surface differently from ordinary runtime
    /// failures so hosts can report budget exhaustion distinctly.
    kind: InterpErrorKind,
    control: Option<Box<Value>>,
}

/// The coarse failure family of an [`InterpError`].
///
/// - [`InterpErrorKind::Runtime`] — everything the language itself treats as
///   a runtime failure: overflow, division by zero, out-of-bounds indexing,
///   a missing map key, a checker-shape violation raised defensively.
/// - [`InterpErrorKind::Budget`] — the §5.5 fuel meter reached zero.
/// - [`InterpErrorKind::Deadline`] — a host-configured wall-clock deadline
///   (§5.5) passed at a safepoint.
/// - [`InterpErrorKind::CallDepth`] — the call-depth limit (§5.5) was hit.
/// - [`InterpErrorKind::UnknownEntry`] — the host asked for a function or
///   impl member the program does not declare (§2.1: hosts target specific
///   entry points; a miss is a host-side mistake, reported defensively).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpErrorKind {
    Runtime,
    Budget,
    Deadline,
    CallDepth,
    UnknownEntry,
}

impl InterpError {
    fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
            kind: InterpErrorKind::Runtime,
            control: None,
        }
    }

    /// A host-invoked entry point that does not exist (§2.1): the request
    /// itself is the failure, so it classifies distinctly from script bugs.
    fn unknown_entry(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
            kind: InterpErrorKind::UnknownEntry,
            control: None,
        }
    }

    /// The §5.5 fuel meter reached zero: one more operation was attempted
    /// than the budget allows.
    fn budget(span: Span) -> Self {
        Self {
            message: "fuel budget exhausted (§5.5: execution is metered by \
                      operation count)"
                .into(),
            span,
            kind: InterpErrorKind::Budget,
            control: None,
        }
    }

    /// A host-configured wall-clock deadline (§5.5) passed at a safepoint.
    fn deadline(span: Span) -> Self {
        Self {
            message: "execution deadline exceeded (§5.5: the host's \
                      wall-clock deadline passed at a safepoint)"
                .into(),
            span,
            kind: InterpErrorKind::Deadline,
            control: None,
        }
    }

    /// The §5.5 call-depth limit: recursion outlived the configured bound.
    fn call_depth(limit: usize, span: Span) -> Self {
        Self {
            message: format!("call depth limit of {limit} exceeded"),
            span,
            kind: InterpErrorKind::CallDepth,
            control: None,
        }
    }

    /// The failure family, for hosts that report budget exhaustion
    /// differently from ordinary runtime failures (WHITEPAPER §5.5, §13).
    pub fn kind(&self) -> InterpErrorKind {
        self.kind
    }

    /// The `?` early-return signal (§2.8): the enclosing function returns
    /// `value` (an `Err` construction) immediately.
    fn early_return(value: Value, span: Span) -> Self {
        Self {
            message: String::new(),
            span,
            kind: InterpErrorKind::Runtime,
            control: Some(Box::new(value)),
        }
    }
}

/// A host-provided compile-time builtin surface (§8.5): the megaprogram
/// evaluator answers `cm.*` calls issued from compile-time Checkmate code
/// (`cm.parseExpr`, `cm.parseStmts`, `cm.parse`, `cm.code.*`). The hook
/// fires only for paths the interpreter itself cannot resolve, so ordinary
/// programs never touch it. Returns `None` when the path is not a host
/// builtin; `Some(Err)` reports a clean compile-time error.
pub trait CtHost {
    fn ct_call(&self, path: &str, args: &[Value]) -> Option<Result<Value, String>>;
}

/// A program ready to invoke: the function, struct, and enum declarations
/// of a parsed (and, in the host's pipeline, checked) statement list,
/// collected into name maps. The first registration of a name wins,
/// matching the checker.
pub struct Interpreter<'a> {
    functions: HashMap<&'a str, &'a Stmt>,
    structs: HashMap<&'a str, &'a Stmt>,
    enums: HashMap<&'a str, &'a Stmt>,
    /// Impl member declarations (§10.4), keyed by the joined target path
    /// then by member name — the runtime mirror of the checker's registry.
    impls: HashMap<String, HashMap<String, &'a Stmt>>,
    /// The §8.5 host builtin surface, if this interpreter runs under the
    /// megaprogram evaluator.
    host: Option<&'a dyn CtHost>,
    /// The §5.5 fuel meter, when this run is budgeted: a shared,
    /// deterministic OPERATION counter charged by every statement and
    /// expression evaluation. The compile-time evaluator shares its cell so
    /// a runaway `@`-function (an infinite loop, a memory bomb) terminates
    /// with a clean [`InterpError`] instead of hanging the compiler.
    fuel: Option<&'a Cell<u64>>,
    /// The §5.5 call-depth bound: the maximum CME call frames a single
    /// invocation may nest. Defaults to [`MAX_CALL_DEPTH`]; hosts embedding
    /// the interpreter may lower it (WHITEPAPER §5.5, §13.1).
    call_depth_limit: usize,
    /// The §5.5 wall-clock deadline, when the host configures one: checked
    /// at the same safepoints as fuel, so runaway execution ends with a
    /// clean [`InterpError`] instead of holding the host forever.
    deadline: Option<Instant>,
}

impl<'a> Interpreter<'a> {
    /// Collects every top-level `FuncDecl`, `StructDecl`, `EnumDecl`, and
    /// impl member into the runtime registries. Impl blocks for the same
    /// target union; the checker guarantees no duplicate members.
    pub fn new(statements: &'a [Stmt]) -> Self {
        let mut functions = HashMap::new();
        let mut structs = HashMap::new();
        let mut enums = HashMap::new();
        let mut impls: HashMap<String, HashMap<String, &'a Stmt>> = HashMap::new();
        for statement in statements {
            match &statement.kind {
                StmtKind::FuncDecl { name, .. } => {
                    functions.entry(name.as_str()).or_insert(statement);
                }
                StmtKind::StructDecl { name, .. } => {
                    structs.entry(name.as_str()).or_insert(statement);
                }
                StmtKind::EnumDecl { name, .. } => {
                    enums.entry(name.as_str()).or_insert(statement);
                }
                StmtKind::ImplDecl { target, members } => {
                    let joined = target.join(".");
                    let registry = impls.entry(joined).or_default();
                    for member in members {
                        if let StmtKind::FuncDecl { name, .. } = &member.kind {
                            registry.entry(name.clone()).or_insert(member);
                        }
                    }
                }
                _ => {}
            }
        }
        Self {
            functions,
            structs,
            enums,
            impls,
            host: None,
            fuel: None,
            call_depth_limit: MAX_CALL_DEPTH,
            deadline: None,
        }
    }

    /// Attaches the §8.5 host builtin surface (used by the megaprogram
    /// evaluator; ordinary execution leaves it unset).
    pub fn with_host(mut self, host: &'a dyn CtHost) -> Self {
        self.host = Some(host);
        self
    }

    /// Attaches the §5.5 fuel meter: a deterministic operation count shared
    /// with the host (the compile-time evaluator). When the counter reaches
    /// zero, evaluation stops with a clean budget error — an infinite loop
    /// in compile-time code terminates instead of hanging the compiler.
    pub fn with_fuel(mut self, fuel: &'a Cell<u64>) -> Self {
        self.fuel = Some(fuel);
        self
    }

    /// Lowers the §5.5 call-depth bound below [`MAX_CALL_DEPTH`]. Hosts
    /// embedding the interpreter use this to bound recursion tighter than
    /// the interpreter default (WHITEPAPER §5.5, §13.1 `max_call_depth`);
    /// a depth at or above [`MAX_CALL_DEPTH`] keeps the default.
    pub fn with_call_depth_limit(mut self, limit: usize) -> Self {
        if limit > 0 {
            self.call_depth_limit = limit;
        }
        self
    }

    /// Attaches a §5.5 wall-clock deadline: once `deadline` has passed, the
    /// next safepoint (every statement and expression evaluation) ends the
    /// invocation with a clean deadline error. Real time is only observed
    /// at safepoints — never asynchronously.
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Invokes `name` with `args` (bound by value, cloned). Errors on an
    /// unknown function or an arity mismatch — defensively, since the
    /// checker normally guarantees both.
    pub fn invoke(&self, name: &str, args: &[Value]) -> Result<Value, InterpError> {
        let Some(&declaration) = self.functions.get(name) else {
            return Err(InterpError::unknown_entry(
                format!("unknown function `{name}`"),
                Span::new(0, 0),
            ));
        };
        let mut runner = self.make_runner();
        runner.call_function(declaration, name, args.to_vec(), Span::new(0, 0))
    }

    /// Invokes an impl member (§10.4) by target path and member name — the
    /// host-facing entry into interface implementations ("host execution
    /// targets specific interface functions", §1). Errors defensively on
    /// an unknown target or member.
    pub fn invoke_member(
        &self,
        target: &str,
        member: &str,
        args: &[Value],
    ) -> Result<Value, InterpError> {
        let Some(&declaration) = self
            .impls
            .get(target)
            .and_then(|registry| registry.get(member))
        else {
            return Err(InterpError::unknown_entry(
                format!("unknown impl member `{target}.{member}`"),
                Span::new(0, 0),
            ));
        };
        let display = format!("{target}.{member}");
        let mut runner = self.make_runner();
        runner.call_function(declaration, &display, args.to_vec(), Span::new(0, 0))
    }

    fn make_runner(&self) -> Runner<'_, 'a> {
        Runner {
            functions: &self.functions,
            structs: &self.structs,
            enums: &self.enums,
            impls: &self.impls,
            host: self.host,
            fuel: self.fuel,
            call_depth_limit: self.call_depth_limit,
            deadline: self.deadline,
            scopes: Vec::new(),
            depth: 0,
        }
    }
}

/// The control-flow signal that propagates up through blocks, `if`,
/// `while`, `for`, and `match` until a function boundary turns `Return`
/// into a result.
enum Flow {
    Normal,
    Return(Value),
}

/// One step of a resolved assignment path (§A.7: the target — including
/// every index expression — is evaluated exactly once per assignment).
enum PathOp {
    Field(String),
    Index(Value),
}

/// Mutable execution state for one invocation: the scope stack and the
/// call-depth counter. The declaration registries are borrowed from the
/// [`Interpreter`].
struct Runner<'env, 'a> {
    functions: &'env HashMap<&'a str, &'a Stmt>,
    structs: &'env HashMap<&'a str, &'a Stmt>,
    enums: &'env HashMap<&'a str, &'a Stmt>,
    impls: &'env HashMap<String, HashMap<String, &'a Stmt>>,
    host: Option<&'env dyn CtHost>,
    fuel: Option<&'env Cell<u64>>,
    /// The configured §5.5 call-depth bound (the [`Interpreter`]'s, which
    /// defaults to [`MAX_CALL_DEPTH`]).
    call_depth_limit: usize,
    /// The configured §5.5 wall-clock deadline, if any.
    deadline: Option<Instant>,
    scopes: Vec<HashMap<String, Value>>,
    depth: usize,
}

impl<'env, 'a> Runner<'env, 'a> {
    /// The §5.5 safepoint, charged by every statement and expression
    /// evaluation: a wall-clock deadline is observed first (real time is
    /// only ever read here — never asynchronously), then one deterministic
    /// operation is charged against the fuel meter. A budget attempted at
    /// zero remaining fuel, or after the deadline, is a clean [`InterpError`]
    /// — never a hang, never a panic, so both compile-time evaluation and
    /// host-driven execution stay bounded (§5.5).
    fn check_budget(&mut self, span: Span) -> Result<(), InterpError> {
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(InterpError::deadline(span));
        }
        if let Some(fuel) = self.fuel {
            let remaining = fuel.get();
            if remaining == 0 {
                return Err(InterpError::budget(span));
            }
            fuel.set(remaining - 1);
        }
        Ok(())
    }

    /// Enters a function: depth guard, arity check, parameter binding by
    /// value in the function frame, body execution, and `Flow` conversion.
    /// A `?` control signal (§2.8) becomes this function's return value.
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
        if self.depth >= self.call_depth_limit {
            return Err(InterpError::call_depth(self.call_depth_limit, call_span));
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

        match flow {
            Ok(Flow::Return(value)) => Ok(value),
            // The `?` control signal: the enclosing function returns the
            // carried value (an `Err` construction) at this boundary.
            Err(error) if error.control.is_some() => {
                Ok(error.control.map(|boxed| *boxed).unwrap_or(Value::Void))
            }
            Err(error) => Err(error),
            // Falling off the end of a void function is normal; falling
            // off a non-void one is a checker bug raised defensively.
            Ok(Flow::Normal) if *return_ty == Type::Void => Ok(Value::Void),
            Ok(Flow::Normal) => Err(InterpError::new(
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
        self.check_budget(stmt.span)?;
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
            StmtKind::Assign { target, expr } => {
                let value = self.eval(expr)?;
                self.assign_path(target, value, stmt.span)?;
                Ok(Flow::Normal)
            }
            // §A.7: `x op= e` is exactly `x = x op e`, with the target
            // (base and indices) evaluated exactly once.
            StmtKind::CompoundAssign { target, op, expr } => {
                let (base_name, ops) = self.resolve_lvalue_path(target, stmt.span)?;
                let current = self.read_variable(&base_name, stmt.span)?;
                let current = read_path(&current, &ops, stmt.span)?;
                let right = self.eval(expr)?;
                let result =
                    self.apply_binary(compound_to_binary(*op), current, right, stmt.span)?;
                let mut base = self.read_variable(&base_name, stmt.span)?;
                write_path(&mut base, &ops, result, stmt.span)?;
                self.write_variable(&base_name, base, stmt.span)?;
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
            // §2.14: iterate the array in order, binding each element to a
            // fresh per-iteration declaration.
            StmtKind::For {
                elem_name,
                iterable,
                body,
                ..
            } => {
                let collection = self.eval(iterable)?;
                // §1.4.10 (plan): iterating a map yields its KEYS in
                // insertion order — the compile-time helpers walk capture
                // record fields this way.
                let keys_or_elements: Vec<Value> = match &collection {
                    Value::Array(elements) => elements.clone(),
                    Value::Map(entries) => entries.iter().map(|(key, _)| key.clone()).collect(),
                    other => {
                        return Err(InterpError::new(
                            format!("cannot iterate `{}`", other.kind_name()),
                            iterable.span,
                        ));
                    }
                };
                for element in keys_or_elements {
                    let mut scope = HashMap::new();
                    scope.insert(elem_name.clone(), element);
                    self.scopes.push(scope);
                    let flow = self.exec_stmts(&body.stmts);
                    self.scopes.pop();
                    if let Flow::Return(value) = flow? {
                        return Ok(Flow::Return(value));
                    }
                }
                Ok(Flow::Normal)
            }
            // §2.15: dispatch on the variant; the first matching arm (or
            // the wildcard) runs with the pattern's payload bindings.
            StmtKind::Match { scrutinee, arms } => {
                let value = self.eval(scrutinee)?;
                let Value::Enum {
                    variant, payload, ..
                } = value
                else {
                    return Err(InterpError::new(
                        format!(
                            "match scrutinee must be an enum value, found `{}`",
                            value.kind_name()
                        ),
                        scrutinee.span,
                    ));
                };
                for arm in arms {
                    let matched = match &arm.pattern {
                        Pattern::Wildcard => Some(Vec::new()),
                        Pattern::Variant { variant: v, .. } if *v == variant => {
                            Some(payload.clone())
                        }
                        Pattern::Variant { .. } => None,
                    };
                    if let Some(values) = matched {
                        let mut scope = HashMap::new();
                        if let Pattern::Variant { bindings, .. } = &arm.pattern {
                            for (binding, value) in bindings.iter().zip(values) {
                                scope.insert(binding.name.clone(), value);
                            }
                        }
                        self.scopes.push(scope);
                        let flow = self.exec_stmts(&arm.body.stmts);
                        self.scopes.pop();
                        if let Flow::Return(value) = flow? {
                            return Ok(Flow::Return(value));
                        }
                        return Ok(Flow::Normal);
                    }
                }
                // The checker guarantees exhaustiveness; reaching here is
                // a defensive error, not a panic.
                Err(InterpError::new(
                    "match fell through without a matching arm",
                    stmt.span,
                ))
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
            StmtKind::StructDecl { .. } | StmtKind::EnumDecl { .. } => Err(InterpError::new(
                "type declarations are not executable statements",
                stmt.span,
            )),
            // Only reachable through a hand-built (unchecked) tree: the
            // parser produces impl blocks at top level only, where
            // registration consumes them before execution (§10.4).
            StmtKind::ImplDecl { .. } => Err(InterpError::new(
                "impl blocks are only allowed at top level",
                stmt.span,
            )),
            // Imports carry no executable code (§2.3); the checker rejects a
            // nested one, and top-level imports never reach execution.
            StmtKind::Import { .. } => Err(InterpError::new(
                "imports are only allowed at top level",
                stmt.span,
            )),
            StmtKind::Invalid { .. } => Err(InterpError::new(
                "cannot execute an invalid statement",
                stmt.span,
            )),
        }
    }

    fn eval(&mut self, expr: &'a Expr) -> Result<Value, InterpError> {
        self.check_budget(expr.span)?;
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
            ExprKind::VariantCall {
                enum_name,
                variant,
                args,
            } => self.eval_variant_call(enum_name, variant, args, expr.span),
            ExprKind::PathCall { path, args } => self.eval_path_call(path, args, expr.span),
            ExprKind::Field { obj, name } => {
                let value = self.eval(obj)?;
                read_field(&value, name, expr.span)
            }
            ExprKind::Index { obj, index } => {
                let value = self.eval(obj)?;
                let key = self.eval(index)?;
                read_index(&value, &key, expr.span)
            }
            // §2.8: unwrap `Ok`, or return the enclosing function early
            // with the `Err` value.
            ExprKind::Try { expr: inner } => {
                let operand = self.eval(inner)?;
                match operand {
                    Value::Enum {
                        name,
                        variant: v,
                        payload,
                    } if name == "result" && v == "Ok" && payload.len() == 1 => {
                        Ok(payload.into_iter().next().unwrap_or(Value::Void))
                    }
                    Value::Enum {
                        name,
                        variant: v,
                        payload,
                    } if name == "result" && v == "Err" && payload.len() == 1 => {
                        let error_value = Value::Enum {
                            name,
                            variant: v,
                            payload,
                        };
                        Err(InterpError::early_return(error_value, expr.span))
                    }
                    other => Err(InterpError::new(
                        format!(
                            "the `?` operator requires `result<T, E>`, found `{}`",
                            other.kind_name()
                        ),
                        expr.span,
                    )),
                }
            }
            // §2.15: the arm value of the first matching pattern.
            ExprKind::Match { scrutinee, arms } => {
                let value = self.eval(scrutinee)?;
                let Value::Enum {
                    variant, payload, ..
                } = value
                else {
                    return Err(InterpError::new(
                        format!(
                            "match scrutinee must be an enum value, found `{}`",
                            value.kind_name()
                        ),
                        scrutinee.span,
                    ));
                };
                for arm in arms {
                    let matched = match &arm.pattern {
                        Pattern::Wildcard => Some(Vec::new()),
                        Pattern::Variant { variant: v, .. } if *v == variant => {
                            Some(payload.clone())
                        }
                        Pattern::Variant { .. } => None,
                    };
                    if let Some(values) = matched {
                        let mut scope = HashMap::new();
                        if let Pattern::Variant { bindings, .. } = &arm.pattern {
                            for (binding, value) in bindings.iter().zip(values) {
                                scope.insert(binding.name.clone(), value);
                            }
                        }
                        self.scopes.push(scope);
                        let result = self.eval(&arm.body);
                        self.scopes.pop();
                        return result;
                    }
                }
                Err(InterpError::new(
                    "match fell through without a matching arm",
                    expr.span,
                ))
            }
            ExprKind::ArrayLit { elements } => {
                let mut values = Vec::with_capacity(elements.len());
                for element in elements {
                    values.push(self.eval(element)?);
                }
                Ok(Value::Array(values))
            }
            ExprKind::MapLit { entries } => {
                let mut pairs = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    let key = self.eval(key)?;
                    let value = self.eval(value)?;
                    // Index assignment replaces; a literal with duplicate
                    // keys keeps the last value (set semantics).
                    if let Some(existing) = pairs.iter_mut().find(|(k, _)| *k == key) {
                        existing.1 = value;
                    } else {
                        pairs.push((key, value));
                    }
                }
                Ok(Value::Map(pairs))
            }
            // §A.6 canonical forms for scalar islands; other values are a
            // checker-gated defensive error.
            ExprKind::Interpolated { parts } => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        InterpPart::Literal(text) => out.push_str(text),
                        InterpPart::Expr(island) => {
                            let value = self.eval(island)?;
                            match stringify(&value) {
                                Some(text) => out.push_str(&text),
                                None => {
                                    return Err(InterpError::new(
                                        format!("cannot interpolate `{}`", value.kind_name()),
                                        island.span,
                                    ));
                                }
                            }
                        }
                    }
                }
                Ok(Value::Str(out))
            }
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

    /// Evaluates call arguments for a function-shaped declaration
    /// (§2.12): positional arguments keep their positions; named arguments
    /// bind by parameter NAME — evaluation itself still runs in source
    /// order so side effects stay left-to-right. Mixing the forms was
    /// rejected by the parser; a stray positional among named arguments
    /// (only reachable through a hand-built tree) is a defensive error.
    fn eval_args_for(
        &mut self,
        declaration: &'a Stmt,
        args: &'a [CallArg],
        span: Span,
    ) -> Result<Vec<Value>, InterpError> {
        if args.iter().all(|arg| matches!(arg, CallArg::Positional(_))) {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                if let CallArg::Positional(expr) = arg {
                    values.push(self.eval(expr)?);
                }
            }
            return Ok(values);
        }
        let StmtKind::FuncDecl { params, .. } = &declaration.kind else {
            return Err(InterpError::new(
                "internal error: not a function declaration",
                span,
            ));
        };
        let mut provided: Vec<(&str, Value)> = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                CallArg::Named { name, expr } => {
                    provided.push((name.as_str(), self.eval(expr)?));
                }
                CallArg::Positional(expr) => {
                    return Err(InterpError::new(
                        "positional and named arguments cannot mix",
                        expr.span,
                    ));
                }
            }
        }
        let mut values = Vec::with_capacity(params.len());
        for param in params {
            match provided
                .iter()
                .find(|(arg_name, _)| *arg_name == param.name.as_str())
            {
                Some((_, value)) => values.push(value.clone()),
                None => {
                    return Err(InterpError::new(
                        format!("missing argument `{}`", param.name),
                        span,
                    ));
                }
            }
        }
        Ok(values)
    }

    fn eval_call(
        &mut self,
        name: &str,
        args: &'a [CallArg],
        span: Span,
    ) -> Result<Value, InterpError> {
        // Resolution order mirrors the checker: user functions, built-in
        // constructors, then struct constructions.
        if let Some(&declaration) = self.functions.get(name) {
            let values = self.eval_args_for(declaration, args, span)?;
            return self.call_function(declaration, name, values, span);
        }
        if let Some(value) = self.eval_builtin_ctor(name, args, span)? {
            return Ok(value);
        }
        if let Some(&declaration) = self.structs.get(name) {
            return self.construct_struct(declaration, args, span);
        }
        Err(InterpError::new(format!("unknown function `{name}`"), span))
    }

    /// The built-in constructors `Ok`, `Err`, `Some`, `None` (§2.8);
    /// `None` when `name` is not one of them.
    fn eval_builtin_ctor(
        &mut self,
        name: &str,
        args: &'a [CallArg],
        span: Span,
    ) -> Result<Option<Value>, InterpError> {
        let (enum_name, variant, arity) = match name {
            "Ok" => ("result", "Ok", 1),
            "Err" => ("result", "Err", 1),
            "Some" => ("option", "Some", 1),
            "None" => ("option", "None", 0),
            _ => return Ok(None),
        };
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            let expr = match arg {
                CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
            };
            values.push(self.eval(expr)?);
        }
        if values.len() != arity {
            return Err(InterpError::new(
                format!(
                    "wrong number of payload values for `{enum_name}.{variant}`: expected {arity}, found {}",
                    values.len()
                ),
                span,
            ));
        }
        Ok(Some(Value::Enum {
            name: enum_name.to_string(),
            variant: variant.to_string(),
            payload: values,
        }))
    }

    /// A struct construction: named arguments bind to the declared fields,
    /// values evaluate in declaration order, and the runtime stores fields
    /// in declaration order (§2.6).
    fn construct_struct(
        &mut self,
        declaration: &'a Stmt,
        args: &'a [CallArg],
        span: Span,
    ) -> Result<Value, InterpError> {
        let StmtKind::StructDecl { name, fields, .. } = &declaration.kind else {
            return Err(InterpError::new(
                "internal error: not a struct declaration",
                span,
            ));
        };
        // Named arguments, checked for duplicates.
        let mut provided: Vec<(&str, &'a Expr)> = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                CallArg::Named { name, expr } => {
                    if provided
                        .iter()
                        .any(|(existing, _)| *existing == name.as_str())
                    {
                        return Err(InterpError::new(
                            format!("duplicate field `{name}` in construction of `{name}`"),
                            expr.span,
                        ));
                    }
                    provided.push((name.as_str(), expr));
                }
                CallArg::Positional(expr) => {
                    return Err(InterpError::new(
                        format!("construction of struct `{name}` requires named arguments"),
                        expr.span,
                    ));
                }
            }
        }
        let mut values = Vec::with_capacity(fields.len());
        for field in fields {
            match provided
                .iter()
                .find(|(arg_name, _)| *arg_name == field.name.as_str())
            {
                Some((_, expr)) => values.push((field.name.clone(), self.eval(expr)?)),
                None => {
                    return Err(InterpError::new(
                        format!("missing field `{}` in construction of `{name}`", field.name),
                        span,
                    ));
                }
            }
        }
        for (arg_name, expr) in &provided {
            if !fields.iter().any(|field| field.name == *arg_name) {
                return Err(InterpError::new(
                    format!("unknown field `{arg_name}` in construction of `{name}`"),
                    expr.span,
                ));
            }
        }
        Ok(Value::Struct {
            name: name.clone(),
            fields: values,
        })
    }

    /// A qualified call `Target.name(args)` (§2.7, §10.4): an enum variant
    /// construction — including the qualified builtins `option.Some` /
    /// `result.Ok` — when `Target` is an enum declaring that variant, or an
    /// impl member call otherwise. The resolution order mirrors the
    /// checker: variant first, member second.
    fn eval_variant_call(
        &mut self,
        enum_name: &str,
        variant: &str,
        args: &'a [CallArg],
        span: Span,
    ) -> Result<Value, InterpError> {
        // The built-in enums construct directly; the checker rejects impls
        // on them, so no member fallback applies here.
        if enum_name == "option" || enum_name == "result" {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                values.push(self.eval(expr)?);
            }
            return Ok(Value::Enum {
                name: enum_name.to_string(),
                variant: variant.to_string(),
                payload: values,
            });
        }
        // §2.7: a declared variant of a declared enum wins.
        if let Some(&declaration) = self.enums.get(enum_name)
            && let StmtKind::EnumDecl { variants, .. } = &declaration.kind
            && let Some(variant_def) = variants.iter().find(|v| v.name == variant)
        {
            return self.construct_variant(enum_name, variant, variant_def, args, span);
        }
        // §10.4: an impl member on the same target.
        if let Some(&declaration) = self
            .impls
            .get(enum_name)
            .and_then(|registry| registry.get(variant))
        {
            let display = format!("{enum_name}.{variant}");
            let values = self.eval_args_for(declaration, args, span)?;
            return self.call_function(declaration, &display, values, span);
        }
        // §8.5: `cm.*` calls from compile-time Checkmate code fall through
        // to the host builtin surface (only when one is attached).
        if enum_name == "cm"
            && let Some(host) = self.host
        {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                values.push(self.eval(expr)?);
            }
            let joined = format!("{enum_name}.{variant}");
            return match host.ct_call(&joined, &values) {
                Some(Ok(value)) => Ok(value),
                Some(Err(message)) => Err(InterpError::new(
                    format!("compile-time builtin `{joined}`: {message}"),
                    span,
                )),
                None => Err(InterpError::new(
                    format!("unknown compile-time builtin `{joined}`"),
                    span,
                )),
            };
        }
        // Error shapes mirror the checker's diagnostics.
        if self.enums.contains_key(enum_name) {
            Err(InterpError::new(
                format!("unknown variant `{variant}` in `{enum_name}`"),
                span,
            ))
        } else if self.structs.contains_key(enum_name) {
            Err(InterpError::new(
                format!("unknown member `{variant}` in `{enum_name}`"),
                span,
            ))
        } else {
            Err(InterpError::new(
                format!("unknown enum `{enum_name}`"),
                span,
            ))
        }
    }

    /// A call through a dotted path of three or more segments (§2.3,
    /// §10.4): the last segment names an impl member; the leading segments
    /// name its target.
    fn eval_path_call(
        &mut self,
        path: &[String],
        args: &'a [CallArg],
        span: Span,
    ) -> Result<Value, InterpError> {
        if path.len() >= 2 {
            let member = &path[path.len() - 1];
            let target = path[..path.len() - 1].join(".");
            if let Some(&declaration) = self
                .impls
                .get(&target)
                .and_then(|registry| registry.get(member.as_str()))
            {
                let display = format!("{target}.{member}");
                let values = self.eval_args_for(declaration, args, span)?;
                return self.call_function(declaration, &display, values, span);
            }
        }
        // §8.5: `cm.*` calls from compile-time Checkmate code fall through
        // to the host builtin surface (only when one is attached).
        if path.first().map(String::as_str) == Some("cm")
            && let Some(host) = self.host
        {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                values.push(self.eval(expr)?);
            }
            let joined = path.join(".");
            return match host.ct_call(&joined, &values) {
                Some(Ok(value)) => Ok(value),
                Some(Err(message)) => Err(InterpError::new(
                    format!("compile-time builtin `{joined}`: {message}"),
                    span,
                )),
                None => Err(InterpError::new(
                    format!("unknown compile-time builtin `{joined}`"),
                    span,
                )),
            };
        }
        Err(InterpError::new(
            format!("unknown function `{}`", path.join(".")),
            span,
        ))
    }

    /// Evaluates a variant's positional payload values and checks their
    /// count (§2.7).
    fn construct_variant(
        &mut self,
        enum_name: &str,
        variant: &str,
        variant_def: &cme_core::ast::VariantDecl,
        args: &'a [CallArg],
        span: Span,
    ) -> Result<Value, InterpError> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            let expr = match arg {
                CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
            };
            values.push(self.eval(expr)?);
        }
        if values.len() != variant_def.fields.len() {
            return Err(InterpError::new(
                format!(
                    "wrong number of payload values for `{enum_name}.{variant}`: expected {}, found {}",
                    variant_def.fields.len(),
                    values.len()
                ),
                span,
            ));
        }
        Ok(Value::Enum {
            name: enum_name.to_string(),
            variant: variant.to_string(),
            payload: values,
        })
    }

    /// Resolves an lvalue to its base variable and the operations along
    /// the chain, evaluating every index exactly once (§A.7).
    fn resolve_lvalue_path(
        &mut self,
        target: &'a LValue,
        _span: Span,
    ) -> Result<(String, Vec<PathOp>), InterpError> {
        let mut ops = Vec::new();
        let mut cursor = target;
        loop {
            match cursor {
                LValue::Var { name } => {
                    ops.reverse();
                    return Ok((name.clone(), ops));
                }
                LValue::Field { base, name } => {
                    ops.push(PathOp::Field(name.clone()));
                    cursor = base;
                }
                LValue::Index { base, index } => {
                    let key = self.eval(index)?;
                    ops.push(PathOp::Index(key));
                    cursor = base;
                }
            }
        }
    }

    /// Assigns through an lvalue: resolve the path, clone the base, mutate
    /// the clone, write it back (§2.13 value semantics keep the clone
    /// isolated).
    fn assign_path(
        &mut self,
        target: &'a LValue,
        value: Value,
        span: Span,
    ) -> Result<(), InterpError> {
        let (base_name, ops) = self.resolve_lvalue_path(target, span)?;
        if !self
            .scopes
            .iter()
            .any(|frame| frame.contains_key(&base_name))
        {
            return Err(InterpError::new(
                format!("assignment to undeclared name `{base_name}`"),
                span,
            ));
        }
        let mut base = self.read_variable(&base_name, span)?;
        write_path(&mut base, &ops, value, span)?;
        self.write_variable(&base_name, base, span)
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
            // (NaN == NaN is false); structs, enums, arrays, and maps are
            // structural.
            BinaryOp::Eq => Ok(Value::Bool(left == right)),
            BinaryOp::Ne => Ok(Value::Bool(left != right)),
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
}

/// Reads one field of a struct value, or `.length` of an array (§2.6, §11).
fn read_field(value: &Value, name: &str, span: Span) -> Result<Value, InterpError> {
    match value {
        Value::Array(elements) if name == "length" => Ok(Value::Int(elements.len() as i64)),
        Value::Struct { fields, .. } => match fields.iter().find(|(f, _)| f == name) {
            Some((_, value)) => Ok(value.clone()),
            None => Err(InterpError::new(
                format!("unknown field `{name}` on `{}`", value.kind_name()),
                span,
            )),
        },
        _ => Err(InterpError::new(
            format!("unknown field `{name}` on `{}`", value.kind_name()),
            span,
        )),
    }
}

/// Reads one element of an array (bounds-checked) or one entry of a map
/// (§11).
fn read_index(value: &Value, key: &Value, span: Span) -> Result<Value, InterpError> {
    match value {
        Value::Array(elements) => match key {
            Value::Int(index) => match elements.get(*index as usize) {
                Some(element) => Ok(element.clone()),
                None => Err(InterpError::new(
                    format!(
                        "array index {index} out of bounds (length {})",
                        elements.len()
                    ),
                    span,
                )),
            },
            other => Err(InterpError::new(
                format!("array index must be `int`, found `{}`", other.kind_name()),
                span,
            )),
        },
        Value::Map(entries) => match entries.iter().find(|(k, _)| k == key) {
            Some((_, value)) => Ok(value.clone()),
            None => Err(InterpError::new(format!("map key {key} not found"), span)),
        },
        _ => Err(InterpError::new(
            format!("cannot index `{}`", value.kind_name()),
            span,
        )),
    }
}

/// Reads through a resolved path (§A.7: indices were evaluated once).
fn read_path(value: &Value, ops: &[PathOp], span: Span) -> Result<Value, InterpError> {
    let mut current = value.clone();
    for op in ops {
        current = match op {
            PathOp::Field(name) => read_field(&current, name, span)?,
            PathOp::Index(key) => read_index(&current, key, span)?,
        };
    }
    Ok(current)
}

/// Writes through a resolved path, mutating `value` in place. A map index
/// write inserts a new entry at the leaf; writing through a missing key
/// is an error (§11).
fn write_path(
    value: &mut Value,
    ops: &[PathOp],
    new: Value,
    span: Span,
) -> Result<(), InterpError> {
    let Some((op, rest)) = ops.split_first() else {
        *value = new;
        return Ok(());
    };
    match (op, value) {
        (PathOp::Field(name), Value::Struct { fields, .. }) => {
            match fields.iter_mut().find(|(f, _)| f == name) {
                Some((_, field)) => write_path(field, rest, new, span),
                None => Err(InterpError::new(
                    format!("unknown field `{name}` on struct"),
                    span,
                )),
            }
        }
        (PathOp::Index(Value::Int(index)), Value::Array(elements)) => {
            let index = *index;
            match elements.get_mut(index as usize) {
                Some(element) => write_path(element, rest, new, span),
                None => Err(InterpError::new(
                    format!(
                        "array index {index} out of bounds (length {})",
                        elements.len()
                    ),
                    span,
                )),
            }
        }
        (PathOp::Index(key), Value::Map(entries)) => {
            match entries.iter_mut().find(|(k, _)| k == key) {
                Some((_, entry)) => write_path(entry, rest, new, span),
                None if rest.is_empty() => {
                    // Insertion through index assignment (§11).
                    entries.push((key.clone(), new));
                    Ok(())
                }
                None => Err(InterpError::new(format!("map key {key} not found"), span)),
            }
        }
        (op, value) => Err(InterpError::new(
            format!(
                "cannot assign through `{}` on `{}`",
                op.describe(),
                value.kind_name()
            ),
            span,
        )),
    }
}

impl PathOp {
    fn describe(&self) -> &'static str {
        match self {
            PathOp::Field(_) => "field",
            PathOp::Index(_) => "index",
        }
    }
}

/// §A.6 canonical string form, used only inside concatenation and
/// interpolation. `Void` and structured values are not stringifiable.
fn stringify(value: &Value) -> Option<String> {
    match value {
        Value::Void => None,
        Value::Struct { .. } | Value::Enum { .. } | Value::Array(_) | Value::Map(_) => None,
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
    use super::{InterpError, InterpErrorKind, Interpreter, MAX_CALL_DEPTH, Value};
    use cme_core::Span;
    use std::cell::Cell;

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

    /// Parse-only statements for tests that invoke something other than
    /// `main` (fuel metering, direct function calls).
    fn parse_statements_for_interp(source: &str) -> Vec<cme_core::ast::Stmt> {
        let outcome = cme_compiler::parse_source(source);
        assert!(
            outcome.is_clean(),
            "test source must parse clean: {:?}",
            outcome.diagnostics
        );
        outcome.statements
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

    // ------------------------------------------------------------------
    // Full-surface runtime: structs, enums, match, for, collections,
    // interpolation, and ?.
    // ------------------------------------------------------------------

    fn ok_full(source: &str) -> Value {
        run_main(source).expect("test program should run to completion")
    }

    #[test]
    fn struct_construction_and_field_semantics() {
        // §2.6 / §2.13: named-field construction, field mutation, and the
        // caller-isolated copy behavior of damage.
        let source = "struct vec2 {\n    float x\n    float y\n}\nstruct player {\n    str name\n    int health\n    bool alive\n}\nplayer damage(player p, int amount) {\np.health = p.health - amount\nif (p.health <= 0) {\np.alive = false\n}\nreturn p\n}\nint main() {\nplayer hero = player(name: \"Hero\", health: 100, alive: true)\nplayer hurt = damage(hero, 30)\nif (hero.health == 100) {\nif (hurt.health == 70) {\nif (hurt.alive == hero.alive) {\nreturn 1\n}\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn deep_field_and_index_assignment() {
        // §2.13 / §A.7: nested chains assign through the resolved path.
        let source = "struct vec2 {\n    float x\n    float y\n}\nstruct party {\n    int[] scores\n    vec2 base\n}\nint main() {\nparty squad = party(\n    scores: [10, 20, 30]\n    base: vec2(x: 1.0, y: 2.0)\n)\nsquad.scores[1] += 5\nsquad.base.x = 40.0\nif (squad.scores[1] == 25) {\nif (squad.base.x == 40.0) {\nreturn 1\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn struct_equality_is_structural() {
        let source = "struct vec2 {\n    float x\n    float y\n}\nint main() {\nvec2 a = vec2(x: 1.0, y: 2.0)\nvec2 b = vec2(x: 1.0, y: 2.0)\nvec2 c = vec2(x: 9.0, y: 2.0)\nif (a == b) {\nif (a != c) {\nreturn 1\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn enum_match_dispatches_and_destructures() {
        let source = "struct vec2 {\n    float x\n    float y\n}\nenum gameEvent {\n    Damage(int amount)\n    Spawn(str kind, vec2 position)\n    PlayerDied()\n}\nstr describe(gameEvent evt) {\nreturn match (evt) {\n    Damage(int amount) => \"d:\" + amount\n    Spawn(str kind, vec2 position) => \"s:\" + kind + \":\" + position.x\n    PlayerDied() => \"dead\"\n}\n}\nint main() {\nstr a = describe(gameEvent.Damage(25))\nstr b = describe(gameEvent.Spawn(\"orc\", vec2(x: 3.0, y: 1.0)))\nstr c = describe(gameEvent.PlayerDied())\nif (a == \"d:25\") {\nif (b == \"s:orc:3\") {\nif (c == \"dead\") {\nreturn 1\n}\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn try_operator_propagates_through_call_boundaries() {
        // §2.8: ? inside chain returns Err from chain; the caller observes
        // it as an ordinary value.
        let source = "result<int, str> safeDiv(int a, int b) {\nif (b == 0) {\nreturn Err(\"div0\")\n}\nreturn Ok(a / b)\n}\nresult<int, str> chain(int a, int b) {\nint v = safeDiv(a, b)?\nreturn Ok(v * 10)\n}\nint main() {\nresult<int, str> good = chain(8, 2)\nresult<int, str> bad = chain(8, 0)\nmatch (good) {\n    Ok(int v) => {\n        match (bad) {\n            Ok(int w) => { return 0 }\n            Err(str reason) => { if (reason == \"div0\") { return 1 } }\n        }\n    }\n    Err(str reason) => { return 0 }\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn arrays_iterate_index_and_clone() {
        // §11 / §2.13 / §2.14.
        let source = "int sum(int[] xs) {\nint total = 0\nfor (int v in xs) {\ntotal += v\n}\nreturn total\n}\nint main() {\nint[] a = [1, 2, 3, 4]\nint[] copy = a\ncopy[0] = 99\nif (a[0] == 1) {\nif (a.length == 4) {\nif (sum(a) == 10) {\nreturn 1\n}\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn map_entries_read_write_and_insert() {
        let source = "int main() {\nmap<str, int> m = {\n\"gold\": 120\n\"gems\": 3\n}\nm[\"gold\"] += 30\nm[\"arrows\"] = 60\nif (m[\"gold\"] == 150) {\nif (m[\"arrows\"] == 60) {\nreturn 1\n}\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn map_equality_is_order_insensitive() {
        let source = "int main() {\nmap<str, int> a = {\"x\": 1\n\"y\": 2\n}\nmap<str, int> b = {\"y\": 2\n\"x\": 1\n}\nif (a == b) {\nreturn 1\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn interpolated_strings_evaluate_islands() {
        // §2.8/§4.1 + §A.6 canonical forms.
        let source = "struct vec2 {\n    float x\n    float y\n}\nint fib(int n) {\nif (n <= 1) {\nreturn n\n}\nreturn fib(n - 1) + fib(n - 2)\n}\nstr main() {\nint hp = 100\nvec2 p = vec2(x: 3.5, y: -1.5)\nbool armed = true\nstr s = $\"hp={hp} pos=({p.x},{p.y}) next={fib(7)} armed={armed}\"\nreturn s\n}\n";
        assert_eq!(
            ok_full(source),
            Value::Str("hp=100 pos=(3.5,-1.5) next=13 armed=true".to_string())
        );
    }

    #[test]
    fn out_of_bounds_and_missing_key_are_clean_errors() {
        let source = "int main() {\nint[] a = [1, 2]\nreturn a[5]\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "array index 5 out of bounds (length 2)");

        let source = "int main() {\nmap<str, int> m = {\"a\": 1\n}\nreturn m[\"b\"]\n}\n";
        let error = run_main(source).unwrap_err();
        assert_eq!(error.message, "map key b not found");
    }

    #[test]
    fn struct_values_display_cmon_style() {
        // §11.1: the Display of a struct value is CMON-shaped. Structs do
        // not interpolate (§A.6), so the value comes back directly.
        let source = "struct vec2 {\n    float x\n    float y\n}\nstruct box {\n    vec2 corner\n    int[] items\n}\nbox main() {\nbox b = box(\n    corner: vec2(x: 1.0, y: 2.0)\n    items: [7, 8]\n)\nreturn b\n}\n";
        let value = ok_full(source);
        assert_eq!(
            value.to_string(),
            "box(corner: vec2(x: 1, y: 2), items: [7, 8])"
        );
    }

    #[test]
    fn enum_and_array_values_display_cmon_style() {
        let source = "enum gameEvent {\n    Damage(int amount)\n}\ngameEvent main() {\nreturn gameEvent.Damage(25)\n}\n";
        assert_eq!(ok_full(source).to_string(), "gameEvent.Damage(25)");

        let source = "int[] main() {\nreturn [1, 2]\n}\n";
        assert_eq!(ok_full(source).to_string(), "[1, 2]");

        let source = "map<str, int> main() {\nreturn {\"a\": 1\n\"b\": 2\n}\n}\n";
        assert_eq!(ok_full(source).to_string(), "{a: 1, b: 2}");
    }

    #[test]
    fn value_semantics_for_arrays_passed_to_functions() {
        // resetFirst mutates its own copy; the caller's array is untouched.
        let source = "void resetFirst(int[] xs) {\nxs[0] = 0\n}\nint main() {\nint[] a = [1, 2, 3]\nresetFirst(a)\nif (a[0] == 1) {\nreturn 1\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    // ------------------------------------------------------------------
    // §10.4 — impl blocks
    // ------------------------------------------------------------------

    #[test]
    fn impl_members_on_structs_execute() {
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\nint main() {\ncounter c = counter(value: 41)\nreturn counter.peek(c)\n}\n";
        assert_eq!(ok_full(source), Value::Int(41));
    }

    #[test]
    fn impl_member_value_semantics_never_escape_the_caller() {
        // bump mutates its own clone; the caller's counter is untouched
        // (§2.13), and bump returns the incremented copy.
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    counter bump(counter c) {\n        c.value += 1\n        return c\n    }\n}\nint main() {\ncounter c = counter(value: 41)\ncounter bumped = counter.bump(c)\nif (c.value != 41) {\nreturn 0\n}\nreturn bumped.value\n}\n";
        assert_eq!(ok_full(source), Value::Int(42));
    }

    #[test]
    fn impl_blocks_union_and_members_call_each_other() {
        // Two blocks for the same target; a member of the second calls a
        // member of the first (forward reference across blocks, §10.4).
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\nimpl counter {\n    int peekTwice(counter c) {\n        return counter.peek(c) + counter.peek(c)\n    }\n}\nint main() {\ncounter c = counter(value: 21)\nreturn counter.peekTwice(c)\n}\n";
        assert_eq!(ok_full(source), Value::Int(42));
    }

    #[test]
    fn impl_members_on_enums_execute() {
        let source = "enum suit {\n    Clubs()\n    Hearts()\n}\nimpl suit {\n    str label(suit s) {\nstr name = match (s) {\n    Clubs() => \"clubs\"\n    Hearts() => \"hearts\"\n}\nreturn name\n    }\n}\nint main() {\nif (suit.label(suit.Clubs()) == \"clubs\") {\nreturn 1\n}\nreturn 0\n}\n";
        assert_eq!(ok_full(source), Value::Int(1));
    }

    #[test]
    fn host_style_path_impl_members_execute() {
        // §10.4 + §2.13: a member that updates its state parameter returns
        // the updated value (a void member's mutation would be silently
        // lost — the checker now errors on exactly that shape), and the
        // caller reassigns to observe it.
        let source = "struct GameConfig {\n    int startingScore\n}\nstruct GameState {\n    int score\n}\nimpl engine.gamemode {\n    GameState InitGame(GameConfig config) {\n        return GameState(score: config.startingScore)\n    }\n    GameState OnTick(GameState state) {\n        state.score += 1\n        return state\n    }\n}\nint main() {\nGameConfig config = GameConfig(startingScore: 100)\nGameState state = engine.gamemode.InitGame(config)\nstate = engine.gamemode.OnTick(state)\nreturn state.score\n}\n";
        assert_eq!(ok_full(source), Value::Int(101));
    }

    #[test]
    fn impl_members_accept_named_arguments_bound_by_name() {
        // Named arguments bind by NAME (§2.12): the out-of-order call still
        // binds low/high correctly, for impl members and plain functions.
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int clampAround(counter c, int low, int high) {\nif (c.value < low) {\n    return low\n}\nif (c.value > high) {\n    return high\n}\nreturn c.value\n    }\n}\nint main() {\ncounter c = counter(value: 50)\nreturn counter.clampAround(high: 10, low: 0, c: c)\n}\n";
        assert_eq!(ok_full(source), Value::Int(10));
    }

    #[test]
    fn plain_function_named_arguments_bind_by_name() {
        // The same fix applies to top-level functions: evaluation order
        // stays source-left-to-right, binding follows parameter names.
        let source = "int clamp(int value, int low, int high) {\nif (value < low) {\nreturn low\n}\nif (value > high) {\nreturn high\n}\nreturn value\n}\nint main() {\nreturn clamp(high: 10, value: 50, low: 0)\n}\n";
        assert_eq!(ok_full(source), Value::Int(10));
    }

    #[test]
    fn impl_member_recursion_runs() {
        let source = "struct counter {\n    int value\n}\nimpl counter {\n    int sumTo(counter c) {\nif (c.value <= 0) {\n    return 0\n}\nc.value -= 1\nreturn c.value + counter.sumTo(c)\n    }\n}\nint main() {\ncounter c = counter(value: 5)\nreturn counter.sumTo(c)\n}\n";
        assert_eq!(ok_full(source), Value::Int(10));
    }

    #[test]
    fn unknown_impl_members_are_clean_errors() {
        // Gated off at check time in the host pipeline; the interpreter
        // still errors cleanly (never panics) on the unregistered member.
        let source =
            "struct counter {\n    int value\n}\nint main() {\nreturn counter.peek(1)\n}\n";
        let error = run_ungated(source).expect_err("unknown member must error");
        assert!(error.message.contains("unknown member `peek` in `counter`"));

        let source = "int main() {\nreturn engine.graphics.DrawTexture(1)\n}\n";
        let error = run_ungated(source).expect_err("unknown path must error");
        assert!(
            error
                .message
                .contains("unknown function `engine.graphics.DrawTexture`")
        );
    }

    #[test]
    fn fuel_exhaustion_is_a_clean_budget_error() {
        // An infinite loop used to hang the caller forever: the tree-walker
        // had no operation meter. With a fuel meter attached (§5.5 — a
        // deterministic operation count, never wall-clock time), the loop
        // terminates with a clean budget error, which is what keeps the
        // compile-time evaluator (§8.7.3) from hanging the compiler on a
        // runaway `@`-function.
        let spin = "int spin() {\nint x = 0\nwhile (true) {\nx = x\n}\nreturn x\n}\n";
        let statements = parse_statements_for_interp(spin);
        let fuel = Cell::new(100);
        let interpreter = Interpreter::new(&statements).with_fuel(&fuel);
        let error = interpreter
            .invoke("spin", &[])
            .expect_err("100 operations must not finish an infinite loop");
        assert!(
            error.message.starts_with("fuel budget exhausted"),
            "{error:?}"
        );
        assert_eq!(error.kind(), InterpErrorKind::Budget);

        // The same meter on a bounded program runs to completion with the
        // correct result — it charges, it does not interfere.
        let bounded = "int bounded() {\nint total = 0\nfor (int i in [1, 2, 3, 4, 5]) {\ntotal = total + i\n}\nreturn total\n}\n";
        let statements = parse_statements_for_interp(bounded);
        let fuel = Cell::new(1_000_000);
        let interpreter = Interpreter::new(&statements).with_fuel(&fuel);
        assert_eq!(interpreter.invoke("bounded", &[]), Ok(Value::Int(15)));
    }

    #[test]
    fn error_kinds_classify_entry_misses_and_runtime_failures() {
        // An unknown entry point is a host-side mistake (§2.1: hosts target
        // specific entry points), classified apart from script failures.
        let statements = parse_statements_for_interp("int main() {\nreturn 1\n}\n");
        let interpreter = Interpreter::new(&statements);
        let error = interpreter
            .invoke("nope", &[])
            .expect_err("unknown functions must fail");
        assert_eq!(error.kind(), InterpErrorKind::UnknownEntry);

        let error = interpreter
            .invoke_member("engine.gamemode", "OnTick", &[])
            .expect_err("unknown impl members must fail");
        assert_eq!(error.kind(), InterpErrorKind::UnknownEntry);

        // Ordinary script failures stay Runtime.
        let source = "int main() {\nreturn 1 / 0\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let interpreter = Interpreter::new(&outcome.statements);
        let error = interpreter.invoke("main", &[]).unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::Runtime);
    }

    #[test]
    fn host_conversions_build_scalars_and_extractors_unpack_them() {
        assert_eq!(Value::from(7i64), Value::Int(7));
        assert_eq!(Value::from(0.5f64), Value::Float(0.5));
        assert_eq!(Value::from(true), Value::Bool(true));
        assert_eq!(Value::from("hi"), Value::Str("hi".into()));
        assert_eq!(Value::from(String::from("hi")), Value::Str("hi".into()));

        assert_eq!(Value::Int(-3).as_int(), Some(-3));
        assert_eq!(Value::Int(-3).as_float(), None);
        assert_eq!(Value::Float(1.5).as_float(), Some(1.5));
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
        assert_eq!(Value::Str("s".into()).as_str(), Some("s"));
        assert_eq!(Value::Str("s".into()).as_int(), None);
        assert!(Value::Void.is_void());
        assert!(!Value::Int(0).is_void());

        // Non-scalar kinds never satisfy scalar accessors.
        assert_eq!(
            Value::Array(vec![Value::Int(1)]).as_array().unwrap()[0],
            Value::Int(1)
        );
        assert_eq!(Value::Array(vec![]).as_int(), None);
    }

    #[test]
    fn configured_call_depth_limit_bounds_recursion_tighter_than_the_default() {
        // The depth check is the FIRST thing a call does after the arity
        // check, so a limit of 1 stops the recursion after exactly one
        // nested frame: main is already running, spin must not enter.
        let source = "int spin(int n) {\nreturn spin(n)\n}\nint main() {\nreturn spin(1)\n}\n";
        let statements = parse_statements_for_interp(source);
        let interpreter = Interpreter::new(&statements).with_call_depth_limit(1);
        let error = interpreter.invoke("main", &[]).unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::CallDepth);
        assert_eq!(error.message, "call depth limit of 1 exceeded");

        // A limit of 0 is the unset sentinel: the default applies, so a
        // bounded program still runs.
        let bounded = "int id(int v) {\nreturn v\n}\nint main() {\nreturn id(3)\n}\n";
        let statements = parse_statements_for_interp(bounded);
        let interpreter = Interpreter::new(&statements).with_call_depth_limit(0);
        assert_eq!(interpreter.invoke("main", &[]), Ok(Value::Int(3)));
    }

    #[test]
    fn the_default_depth_limit_is_unchanged_by_a_limit_at_the_maximum() {
        // Configuring the builder with MAX_CALL_DEPTH itself must keep the
        // interpreter's pinned default behavior: runaway recursion dies at
        // the same limit with the same message. 1024 nested CME frames
        // occupy several native Rust frames each in a debug build, so this
        // runs on a dedicated thread with a generous stack — the depth
        // guard, not the native stack, must be what stops the recursion.
        let source = "int spin() {\nreturn spin()\n}\nint main() {\nreturn spin()\n}\n";
        let handle = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let statements = parse_statements_for_interp(source);
                Interpreter::new(&statements)
                    .with_call_depth_limit(MAX_CALL_DEPTH)
                    .invoke("main", &[])
            })
            .expect("spawn the default-limit thread");
        let result = handle.join().expect("default-limit thread must not panic");
        assert_eq!(
            result.unwrap_err().message,
            format!("call depth limit of {MAX_CALL_DEPTH} exceeded")
        );
    }

    #[test]
    fn a_low_depth_limit_runs_on_the_ordinary_test_stack() {
        // A handful of nested CME frames fit anywhere — this pins that a
        // lowered limit makes deep-recursion testing runnable without a
        // big-stack thread.
        let source = "int down(int n) {\nif (n <= 0) {\nreturn 0\n}\nreturn down(n - 1)\n}\nint main() {\nreturn down(4)\n}\n";
        let statements = parse_statements_for_interp(source);
        let interpreter = Interpreter::new(&statements).with_call_depth_limit(16);
        assert_eq!(interpreter.invoke("main", &[]), Ok(Value::Int(0)));

        let interpreter = Interpreter::new(&statements).with_call_depth_limit(4);
        let error = interpreter.invoke("main", &[]).unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::CallDepth);
        assert_eq!(error.message, "call depth limit of 4 exceeded");
    }

    #[test]
    fn an_expired_deadline_stops_at_the_first_safepoint() {
        // The deadline has already passed: the very first statement's
        // safepoint ends the invocation with a clean deadline error.
        let source = "int spin() {\nint x = 0\nwhile (true) {\nx = x + 1\n}\nreturn x\n}\n";
        let statements = parse_statements_for_interp(source);
        let expired = std::time::Instant::now() - std::time::Duration::from_millis(1);
        let interpreter = Interpreter::new(&statements).with_deadline(expired);
        let error = interpreter.invoke("spin", &[]).unwrap_err();
        assert_eq!(error.kind(), InterpErrorKind::Deadline);
        assert!(
            error.message.starts_with("execution deadline exceeded"),
            "{error:?}"
        );
    }

    #[test]
    fn a_future_deadline_lets_bounded_work_finish() {
        // A deadline 5 seconds out never fires on a bounded program: the
        // check observes real time only at safepoints and must not
        // interfere with ordinary execution.
        let source = "int sum() {\nint total = 0\nfor (int i in [1, 2, 3, 4, 5, 6, 7]) {\ntotal = total + i\n}\nreturn total\n}\n";
        let statements = parse_statements_for_interp(source);
        let soon = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let interpreter = Interpreter::new(&statements).with_deadline(soon);
        assert_eq!(interpreter.invoke("sum", &[]), Ok(Value::Int(28)));
    }
}
