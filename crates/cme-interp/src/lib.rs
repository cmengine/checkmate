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
    BinaryOp, Block, CallArg, CompoundOp, Expr, ExprKind, InterpPart, LValue, Pattern,
    PrimitiveType, Stmt, StmtKind, Type, UnaryOp,
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
    /// An unsigned 8-bit value (0–255): the runtime shape of the `byte`
    /// primitive (§2.4). The checker guarantees a byte slot receives a
    /// byte value or an in-range integer literal, so the walker
    /// crystallizes the literal at every byte-typed slot it fills.
    Byte(u8),
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
            // Decimal for ints and bytes, shortest-round-trip for floats
            // (Rust's `{}` is exactly the §A.6 canonical form), raw text
            // for str.
            Value::Int(value) => write!(formatter, "{value}"),
            Value::Byte(value) => write!(formatter, "{value}"),
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
            (Value::Byte(a), Value::Byte(b)) => a == b,
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
            Value::Byte(_) => "byte".into(),
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

    /// The `byte` payload, or `None` for any other kind (§13.1).
    pub fn as_byte(&self) -> Option<u8> {
        match self {
            Value::Byte(value) => Some(*value),
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

impl From<u8> for Value {
    fn from(value: u8) -> Self {
        Value::Byte(value)
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

/// The host capability boundary (WHITEPAPER §1, §7.2, §9, §13.1): the
/// runtime half of the schema contract. When a script calls a capability
/// member (`engine.graphics.LoadTexture(...)`), the evaluator hands the
/// call to the registered host — the host owns every provider (§9.1:
/// capabilities are functions the HOST provides), packs its native state
/// into [`Value`]s, and returns the result.
///
/// This trait is the deliberate seam between the schema system and the
/// execution engine: the tree walker calls through it today, and the
/// bytecode VM and LLVM AOT engines (§5) will call through the same shape
/// tomorrow, so swapping how Checkmate scripts run never touches host
/// providers. Purity holds by construction — the megaprogram evaluator
/// never attaches a capability host (§8.5), so compile-time code cannot
/// reach host capabilities.
pub trait CapabilityHost {
    /// Invokes `member` of the capability at `path` (`["engine",
    /// "graphics"]`) with positional [`Value`] arguments. Arguments arrive
    /// by value (§2.13: the script's values are cloned into the call, so
    /// the host can never alias script state). Errors are ordinary strings
    /// — the interpreter anchors them to the call site.
    fn call(&self, path: &[&str], member: &str, args: &[Value]) -> Result<Value, String>;
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
    /// The §9/§13.1 capability surface, when the host registered providers.
    capabilities: Option<&'a dyn CapabilityHost>,
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
            capabilities: None,
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

    /// Attaches the host capability surface (§9, §13.1): the providers a
    /// script's `import`ed capabilities dispatch to at runtime. The
    /// megaprogram evaluator never attaches one, which is what keeps
    /// compile-time evaluation pure (§8.5).
    pub fn with_capabilities(mut self, capabilities: &'a dyn CapabilityHost) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    /// Registers SUPPLEMENTARY type declarations — the schema boundary
    /// types (§9.3) a host synthesizes from its registered schemas.
    /// Scripts construct schema structs and enums exactly like local ones,
    /// so the runtime needs their shapes; the supplement is scanned like
    /// the program's own declarations, except that a program-local
    /// declaration always wins (the checker rejects the collision
    /// upstream anyway).
    pub fn with_declarations(mut self, declarations: &'a [Stmt]) -> Self {
        for statement in declarations {
            match &statement.kind {
                StmtKind::StructDecl { name, .. } => {
                    self.structs.entry(name.as_str()).or_insert(statement);
                }
                StmtKind::EnumDecl { name, .. } => {
                    self.enums.entry(name.as_str()).or_insert(statement);
                }
                _ => {}
            }
        }
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
            capabilities: self.capabilities,
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
    capabilities: Option<&'env dyn CapabilityHost>,
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
        // frame below it. Parameter slots crystallize byte literals (§2.4).
        let mut frame = HashMap::with_capacity(params.len());
        for (param, value) in params.iter().zip(args) {
            let value = self.coerce_to_declared(value, &param.ty);
            frame.insert(param.name.clone(), value);
        }
        self.scopes.push(frame);
        let flow = self.exec_stmts(&body.stmts);
        self.scopes.pop();
        self.depth -= 1;

        match flow {
            Ok(Flow::Return(value)) => Ok(self.coerce_to_declared(value, return_ty)),
            // The `?` control signal: the enclosing function returns the
            // carried value (an `Err` construction) at this boundary.
            Err(error) if error.control.is_some() => {
                let value = error.control.map(|boxed| *boxed).unwrap_or(Value::Void);
                Ok(self.coerce_to_declared(value, return_ty))
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
            // Type-aware at byte slots (§2.4): the checker admits only
            // byte-typed values or in-range integer literals, so the
            // walker crystallizes the literal via the declared type.
            StmtKind::VarDecl { ty, name, expr } => {
                let value = self.eval(expr)?;
                let value = self.coerce_to_declared(value, ty);
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
                // Byte slot: the checker guarantees the right operand is a
                // byte value or an in-range integer literal — crystallize
                // the literal against the slot's runtime kind (§2.4).
                let right = match (&current, right) {
                    (Value::Byte(_), Value::Int(v)) if (0..=u8::MAX as i64).contains(&v) => {
                        Value::Byte(v as u8)
                    }
                    (_, other) => other,
                };
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
                elem_ty,
                elem_name,
                iterable,
                body,
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
                    let element = self.coerce_to_declared(element, elem_ty);
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
                Some((_, expr)) => {
                    let value = self.eval(expr)?;
                    let value = self.coerce_to_declared(value, &field.ty);
                    values.push((field.name.clone(), value))
                }
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
        // §9/§13.1: a capability call — `engine.graphics.LoadTexture(...)`
        // — dispatches to the registered host when one is attached. The
        // checker (schema active) has already gated import, version, and
        // `requires` visibility; the host is the authority on what is
        // actually provided.
        if let Some(capabilities) = self.capabilities {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                let expr = match arg {
                    CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
                };
                values.push(self.eval(expr)?);
            }
            let segments: Vec<&str> = path.iter().map(String::as_str).collect();
            let (capability_path, member) = segments.split_at(segments.len() - 1);
            return capabilities
                .call(capability_path, member[0], &values)
                .map_err(|message| InterpError::new(message, span));
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
        for (arg, field) in args.iter().zip(&variant_def.fields) {
            let expr = match arg {
                CallArg::Positional(expr) | CallArg::Named { expr, .. } => expr,
            };
            let value = self.eval(expr)?;
            let value = self.coerce_to_declared(value, &field.ty);
            values.push(value);
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

    /// Crystallizes an integer literal's runtime shape at a byte-typed
    /// slot (§2.4): the checker admits only byte-typed values or in-range
    /// integer literals into byte positions, so this converts `Int → Byte`
    /// at every byte-typed slot the walker fills — declarations, params,
    /// returns, struct fields, enum payloads, for-each elements — and
    /// recurses through arrays, maps, and the §2.8 built-in generics.
    fn coerce_to_declared(&self, value: Value, ty: &Type) -> Value {
        match (value, ty) {
            // §2.4 byte rules: crystallize an in-range int literal into a
            // byte slot, and widen a byte value losslessly into an int
            // slot — the two directions the checker admits.
            (Value::Int(v), Type::Prim(PrimitiveType::Byte))
                if (0..=u8::MAX as i64).contains(&v) =>
            {
                Value::Byte(v as u8)
            }
            (Value::Byte(v), Type::Prim(PrimitiveType::Int)) => Value::Int(v as i64),
            (Value::Array(items), Type::Array(elem)) => Value::Array(
                items
                    .into_iter()
                    .map(|item| self.coerce_to_declared(item, elem))
                    .collect(),
            ),
            (Value::Map(entries), Type::Map { key, value }) => Value::Map(
                entries
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            self.coerce_to_declared(k, key),
                            self.coerce_to_declared(v, value),
                        )
                    })
                    .collect(),
            ),
            (
                Value::Enum {
                    name,
                    variant,
                    payload,
                },
                Type::Named {
                    name: ty_name,
                    args,
                },
            ) if ty_name == "option" && args.len() == 1 => Value::Enum {
                name,
                variant,
                payload: payload
                    .into_iter()
                    .map(|p| self.coerce_to_declared(p, &args[0]))
                    .collect(),
            },
            (
                Value::Enum {
                    name,
                    variant,
                    payload,
                },
                Type::Named {
                    name: ty_name,
                    args,
                },
            ) if ty_name == "result" && args.len() == 2 => {
                let arg = if variant == "Ok" { &args[0] } else { &args[1] };
                Value::Enum {
                    name,
                    variant,
                    payload: payload
                        .into_iter()
                        .map(|p| self.coerce_to_declared(p, arg))
                        .collect(),
                }
            }
            (value, _) => value,
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
                (Value::Byte(a), Value::Byte(b)) => {
                    a.checked_add(b).map(Value::Byte).ok_or_else(overflow)
                }
                // A byte mixed with an int widens (lossless) and computes
                // as int (§2.4).
                (Value::Byte(a), Value::Int(b)) => Ok(Value::Int(a as i64 + b)),
                (Value::Int(a), Value::Byte(b)) => Ok(Value::Int(a + b as i64)),
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
                (Value::Byte(a), Value::Byte(b)) => {
                    a.checked_sub(b).map(Value::Byte).ok_or_else(overflow)
                }
                (Value::Byte(a), Value::Int(b)) => Ok(Value::Int(a as i64 - b)),
                (Value::Int(a), Value::Byte(b)) => Ok(Value::Int(a - b as i64)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Mul => match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    a.checked_mul(b).map(Value::Int).ok_or_else(overflow)
                }
                (Value::Byte(a), Value::Byte(b)) => {
                    a.checked_mul(b).map(Value::Byte).ok_or_else(overflow)
                }
                (Value::Byte(a), Value::Int(b)) => Ok(Value::Int(a as i64 * b)),
                (Value::Int(a), Value::Byte(b)) => Ok(Value::Int(a * b as i64)),
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
                // Unsigned byte division cannot overflow, so
                // `checked_div`'s only failure mode is the zero divisor.
                (Value::Byte(a), Value::Byte(b)) => a
                    .checked_div(b)
                    .map(Value::Byte)
                    .ok_or_else(|| InterpError::new("integer division by zero", span)),
                (Value::Byte(a), Value::Int(b)) => (a as i64)
                    .checked_div(b)
                    .map(Value::Int)
                    .ok_or_else(|| InterpError::new("integer division by zero", span)),
                (Value::Int(a), Value::Byte(b)) => a
                    .checked_div(b as i64)
                    .map(Value::Int)
                    .ok_or_else(|| InterpError::new("integer division by zero", span)),
                _ => Err(mismatch()),
            },
            BinaryOp::Rem => match (left, right) {
                // §A.5: remainder of truncated division, sign of the
                // dividend: -7 % 2 is -1, 7 % -2 is 1. Int-only (§A.4);
                // byte remainder is unsigned, so the dividend's sign is
                // its own value.
                (Value::Int(a), Value::Int(b)) => {
                    if b == 0 {
                        Err(InterpError::new("integer remainder by zero", span))
                    } else {
                        a.checked_rem(b).map(Value::Int).ok_or_else(overflow)
                    }
                }
                // As with division: unsigned byte remainder cannot
                // overflow, so `checked_rem` only fails on the zero
                // divisor.
                (Value::Byte(a), Value::Byte(b)) => a
                    .checked_rem(b)
                    .map(Value::Byte)
                    .ok_or_else(|| InterpError::new("integer remainder by zero", span)),
                (Value::Byte(a), Value::Int(b)) => (a as i64)
                    .checked_rem(b)
                    .map(Value::Int)
                    .ok_or_else(|| InterpError::new("integer remainder by zero", span)),
                (Value::Int(a), Value::Byte(b)) => a
                    .checked_rem(b as i64)
                    .map(Value::Int)
                    .ok_or_else(|| InterpError::new("integer remainder by zero", span)),
                _ => Err(mismatch()),
            },
            // §A.4: strict same-type value equality; float follows IEEE 754
            // (NaN == NaN is false); structs, enums, arrays, and maps are
            // structural.
            BinaryOp::Eq => Ok(Value::Bool(left == right)),
            BinaryOp::Ne => Ok(Value::Bool(left != right)),
            BinaryOp::Lt => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
                (Value::Byte(a), Value::Byte(b)) => Ok(Value::Bool(a < b)),
                (Value::Byte(a), Value::Int(b)) => Ok(Value::Bool((*a as i64) < *b)),
                (Value::Int(a), Value::Byte(b)) => Ok(Value::Bool(*a < *b as i64)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Le => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
                (Value::Byte(a), Value::Byte(b)) => Ok(Value::Bool(a <= b)),
                (Value::Byte(a), Value::Int(b)) => Ok(Value::Bool((*a as i64) <= *b)),
                (Value::Int(a), Value::Byte(b)) => Ok(Value::Bool(*a <= *b as i64)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Gt => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
                (Value::Byte(a), Value::Byte(b)) => Ok(Value::Bool(a > b)),
                (Value::Byte(a), Value::Int(b)) => Ok(Value::Bool((*a as i64) > *b)),
                (Value::Int(a), Value::Byte(b)) => Ok(Value::Bool(*a > *b as i64)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
                _ => Err(mismatch()),
            },
            BinaryOp::Ge => match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
                (Value::Byte(a), Value::Byte(b)) => Ok(Value::Bool(a >= b)),
                (Value::Byte(a), Value::Int(b)) => Ok(Value::Bool((*a as i64) >= *b)),
                (Value::Int(a), Value::Byte(b)) => Ok(Value::Bool(*a >= *b as i64)),
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
        // Byte slot write: the checker admits a byte-typed value or an
        // in-range integer literal, so crystallize the literal against the
        // slot's existing runtime kind (§2.4).
        *value = match (&*value, new) {
            (Value::Byte(_), Value::Int(v)) if (0..=u8::MAX as i64).contains(&v) => {
                Value::Byte(v as u8)
            }
            (_, new) => new,
        };
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
