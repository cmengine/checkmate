//! Execution contexts: §5.5 limits per invocation, the host-visible
//! error surface, and capability dispatch (WHITEPAPER §13).

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cme_core::Span;
use cme_interp::{
    CapabilityHost, InterpError, InterpErrorKind, Interpreter, MAX_CALL_DEPTH, Value,
};

use crate::engine::{CapabilityProvider, CompiledProgram, render_runtime_error};

/// The §5.5 execution constraints a host imposes on invocations created
/// from one context. Every field documents its own unset convention; the
/// defaults are "no fuel meter, no deadline, the interpreter's
/// [`MAX_CALL_DEPTH`] depth bound" — i.e. unconstrained except for the
/// fixed safety net.
///
/// ```
/// use cme_api::ExecutionLimits;
///
/// let limits = ExecutionLimits {
///     fuel: Some(1_000_000),
///     deadline_ms: Some(50),
///     max_call_depth: 64,
/// };
/// assert_eq!(limits.fuel, Some(1_000_000));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionLimits {
    /// A deterministic operation count (§5.5 fuel metering): every
    /// statement and expression evaluation charges one unit. `None` runs
    /// unmetered; `Some(0)` exhausts at the first safepoint.
    pub fuel: Option<u64>,
    /// Wall-clock budget in milliseconds, observed at safepoints only —
    /// real time is never read asynchronously (§5.5). `None` sets none;
    /// `Some(0)` expires immediately.
    pub deadline_ms: Option<u64>,
    /// Maximum nested call frames (§5.5 call-depth limits). `0` keeps the
    /// interpreter default [`MAX_CALL_DEPTH`]. Hosts running programs that
    /// legitimately recurse near the default must provide adequate native
    /// stack (a dedicated thread) or lower this — the depth guard, not the
    /// native stack, is what must stop runaway recursion.
    pub max_call_depth: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            fuel: None,
            deadline_ms: None,
            max_call_depth: MAX_CALL_DEPTH,
        }
    }
}

/// The coarse family of an execution failure — the host-facing projection
/// of [`InterpErrorKind`]. Limit-family failures (§5.5) surface distinctly
/// from script bugs so hosts can report them differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Ordinary script failure: overflow, division by zero, out-of-bounds
    /// indexing, a missing map key, a defensive shape violation.
    Runtime,
    /// The §5.5 fuel budget reached zero.
    Budget,
    /// The §5.5 wall-clock deadline passed at a safepoint.
    Deadline,
    /// The §5.5 call-depth limit was hit.
    CallDepth,
    /// The host asked for an entry point (§2.1) the program does not
    /// declare: an unknown function or impl member.
    UnknownEntry,
    /// A host capability tried to invoke back into this context while an
    /// invocation was still active on the calling thread — the §5.7
    /// reentrancy prohibition. The call is rejected before any script code
    /// runs; the original invocation is unaffected.
    Reentrant,
}

impl From<InterpErrorKind> for ErrorKind {
    fn from(kind: InterpErrorKind) -> Self {
        match kind {
            InterpErrorKind::Runtime => ErrorKind::Runtime,
            InterpErrorKind::Budget => ErrorKind::Budget,
            InterpErrorKind::Deadline => ErrorKind::Deadline,
            InterpErrorKind::CallDepth => ErrorKind::CallDepth,
            InterpErrorKind::UnknownEntry => ErrorKind::UnknownEntry,
        }
    }
}

/// A failed invocation: what went wrong, classified by [`ErrorKind`], and
/// where. For mod builds the position re-anchors to the executing module,
/// so `file` names it and `line`/`column` point into that module's OWN
/// text; loose sources leave `file` unset (the host knows its source) and
/// position into the expanded text. Entry-miss errors carry no position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionError {
    pub kind: ErrorKind,
    pub message: String,
    /// 1-based line of the failure, or `0` when there is no position.
    pub line: usize,
    /// 1-based character column of the failure, or `0` when there is none.
    pub column: usize,
    /// The owning module's display path for mod builds (`my_mod/src/main.cm`
    /// rooted at the load path); `None` for loose sources and positionless
    /// errors.
    pub file: Option<String>,
    /// The failure's source span. Loose sources: byte offsets into the
    /// expanded program text (see [`CompiledProgram::source`]). Mod builds:
    /// the span re-anchored into the owning module's original text.
    /// `None` when the failure has no position.
    pub span: Option<Span>,
}

impl ExecutionError {
    /// Renders `file:line:column: message` (or `line:column: message` for
    /// loose sources; bare `message` with no position) — one stable shape
    /// for logs.
    pub fn render(&self) -> String {
        match (&self.file, self.line) {
            (Some(file), 0) => format!("{file}: {message}", message = self.message),
            (Some(file), _) => format!(
                "{file}:{line}:{column}: {message}",
                line = self.line,
                column = self.column,
                message = self.message
            ),
            (None, 0) => self.message.clone(),
            (None, _) => format!(
                "line {line}, column {column}: {message}",
                line = self.line,
                column = self.column,
                message = self.message
            ),
        }
    }
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render())
    }
}

impl std::error::Error for ExecutionError {}

// The §5.7 reentrancy guard: the identity of the context currently
// executing an invocation on THIS thread (`0` when none). Capability
// dispatch is synchronous on the invoking thread, so a provider that
// calls back into the same context observes its own id here — concurrent
// invocations from other threads (legal per §5.7) never see it. (Doc
// comments on `thread_local!` items are swallowed by the macro, so the
// guard's contract lives in this plain comment.)
thread_local! {
    static ACTIVE_CONTEXT: Cell<usize> = const { Cell::new(0) };
}

/// Restores the previous guard value even when an invocation panics, so a
/// thread never stays poisoned after a host bug unwinds through it.
struct RestoreActiveContext(usize);

impl Drop for RestoreActiveContext {
    fn drop(&mut self) {
        ACTIVE_CONTEXT.with(|cell| cell.set(self.0));
    }
}

/// One execution context over a compiled program (WHITEPAPER §13.1
/// `create_context`). Contexts are cheap handles over an immutable
/// program: every invocation constructs its own interpreter frame, fuel
/// cell, and deadline, so
///
/// - invocations from the same context never share mutable state, and
/// - one context may issue concurrent invocations from many threads.
///
/// The context borrows its program (`create_context(&program, limits)`),
/// which is what makes program-before-context lifetimes a compile-time
/// guarantee. Capability calls (§9) dispatch to the provider snapshot the
/// context was created with.
#[derive(Clone)]
pub struct Context<'p> {
    program: &'p CompiledProgram,
    limits: ExecutionLimits,
    providers: Arc<HashMap<String, Arc<dyn CapabilityProvider>>>,
    /// The §5.7 reentrancy-guard identity: shared by clones (they are the
    /// same logical context), unique per [`Context::new`].
    identity: Arc<()>,
}

/// The constructor side of the §9.6/§13.1 interface-proxy flow: a schema
/// proxy type (generated by `cme_schema_bindings!`) implements this, and
/// [`Context::get_interface`] constructs it generically —
///
/// ```ignore
/// let gamemode = context.get_interface::<EngineGamemodeProxy>()?;
/// ```
///
/// Construction fails (with an [`ExecutionError`] of kind
/// [`ErrorKind::UnknownEntry`]) when the loaded program does not
/// implement the interface — the host-side guard over the completeness
/// the script compiler already enforced (§10.4).
pub trait InterfaceProxy<'a, 'p>: Sized {
    /// Builds the proxy over `context`, or fails when the program does
    /// not implement the interface the proxy targets.
    fn from_context(context: &'a Context<'p>) -> Result<Self, ExecutionError>;
}

impl<'p> fmt::Debug for Context<'p> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Context")
            .field("program", &self.program)
            .field("limits", &self.limits)
            .field("capabilities", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl<'p> Context<'p> {
    /// Creates a context; prefer [`crate::Engine::create_context`].
    pub fn new(
        program: &'p CompiledProgram,
        limits: ExecutionLimits,
        providers: Arc<HashMap<String, Arc<dyn CapabilityProvider>>>,
    ) -> Context<'p> {
        Context {
            program,
            limits,
            providers,
            identity: Arc::new(()),
        }
    }

    /// The program this context executes.
    pub fn program(&self) -> &'p CompiledProgram {
        self.program
    }

    /// The limits every invocation of this context runs under.
    pub fn limits(&self) -> &ExecutionLimits {
        &self.limits
    }

    /// Whether the program implements `target` — the check a generated
    /// interface proxy (§9.6) performs at construction so a host cannot
    /// silently call into an interface the program never implemented.
    pub fn has_interface(&self, target: &str) -> bool {
        self.program.interface_targets().iter().any(|t| t == target)
    }

    /// Constructs a schema-generated interface proxy over this context
    /// (§13.1 `get_interface`): the typed, compile-checked handle for the
    /// host's calls INTO the script's implementation of an interface
    /// (§9.6, §10.4). Fails when the program does not implement it.
    ///
    /// ```ignore
    /// let gamemode = context.get_interface::<EngineGamemodeProxy>()?;
    /// let state = gamemode.init_game(config)?; // typed args and result
    /// ```
    pub fn get_interface<'a, P>(&'a self) -> Result<P, ExecutionError>
    where
        P: InterfaceProxy<'a, 'p>,
    {
        P::from_context(self)
    }

    /// Invokes a top-level function by name with positional arguments
    /// (bound by value, cloned — §2.13 value semantics: the host's
    /// arguments are never aliased into the script).
    pub fn invoke(&self, name: &str, args: &[Value]) -> Result<Value, ExecutionError> {
        self.run(|interpreter| interpreter.invoke(name, args))
    }

    /// Invokes a §10.4 impl member — the host's entry into "interface
    /// functions" a mod implements (`impl engine.gamemode { … }`).
    /// `target` is the joined impl path (`"engine.gamemode"`), `member`
    /// the function name (`"OnTick"`).
    pub fn invoke_member(
        &self,
        target: &str,
        member: &str,
        args: &[Value],
    ) -> Result<Value, ExecutionError> {
        self.run(|interpreter| interpreter.invoke_member(target, member, args))
    }

    /// Runs one invocation under this context's §5.5 limits: a fresh fuel
    /// cell and deadline per call, the depth bound configured once. The
    /// interpreter never escapes — contexts expose values, not frames.
    /// Capability calls (§9) dispatch to the snapshot's providers through
    /// the engine-agnostic [`CapabilityHost`] seam.
    ///
    /// §5.7 reentrancy: while an invocation is active on a thread, a
    /// capability dispatched from it may not invoke back into the SAME
    /// context — the nested call fails with [`ErrorKind::Reentrant`] and
    /// the original invocation continues. Invoking a DIFFERENT context
    /// (or invoking from another thread) stays legal: independent
    /// invocations share no mutable state.
    fn run(
        &self,
        call: impl FnOnce(&Interpreter<'_>) -> Result<Value, InterpError>,
    ) -> Result<Value, ExecutionError> {
        // The identity is shared by clones (a clone IS the same logical
        // context) and distinct per `create_context` call. `0` means no
        // active invocation, and a real allocation never collides with it.
        let context_id = Arc::as_ptr(&self.identity).addr();
        if ACTIVE_CONTEXT.get() == context_id {
            return Err(ExecutionError {
                kind: ErrorKind::Reentrant,
                message: "reentrant invocation rejected (§5.7): a host capability called \
                          by this context may not invoke script functions on the same \
                          context before the original call returns"
                    .to_string(),
                line: 0,
                column: 0,
                file: None,
                span: None,
            });
        }
        let previous = ACTIVE_CONTEXT.with(|cell| cell.replace(context_id));
        let _restore = RestoreActiveContext(previous);

        let fuel = self.limits.fuel.map(Cell::new);
        let deadline = self
            .limits
            .deadline_ms
            .map(|ms| Instant::now() + Duration::from_millis(ms));

        let dispatch = ProviderDispatch {
            providers: &self.providers,
        };
        let mut interpreter = Interpreter::new(self.program.statements())
            .with_declarations(self.program.schema_declarations());
        if let Some(cell) = fuel.as_ref() {
            interpreter = interpreter.with_fuel(cell);
        }
        if let Some(deadline) = deadline {
            interpreter = interpreter.with_deadline(deadline);
        }
        interpreter = interpreter.with_call_depth_limit(self.limits.max_call_depth);
        if !self.providers.is_empty() {
            interpreter = interpreter.with_capabilities(&dispatch);
        }

        call(&interpreter).map_err(|error| self.execution_error(error))
    }

    /// Projects an [`InterpError`] into the host-facing [`ExecutionError`]:
    /// classify by kind, re-anchor the span (mods re-anchor to the owning
    /// module; entry misses have no position), and precompute line/column
    /// so hosts never touch span arithmetic.
    fn execution_error(&self, error: InterpError) -> ExecutionError {
        let kind = ErrorKind::from(error.kind());
        let has_position = !(kind == ErrorKind::UnknownEntry && error.span == Span::new(0, 0));
        let (file, line, column, span) = if has_position {
            render_runtime_error(error.span, self.program)
        } else {
            (None, 0, 0, None)
        };
        ExecutionError {
            kind,
            message: error.message,
            line,
            column,
            file,
            span,
        }
    }
}

/// The bridge from the interpreter's [`CapabilityHost`] seam to the
/// registered providers: `path` (`["engine", "graphics"]`) keys the
/// provider table, the member dispatches inside it.
struct ProviderDispatch<'a> {
    providers: &'a HashMap<String, Arc<dyn CapabilityProvider>>,
}

impl<'a> CapabilityHost for ProviderDispatch<'a> {
    fn call(&self, path: &[&str], member: &str, args: &[Value]) -> Result<Value, String> {
        let qualified = path.join(".");
        let provider = self
            .providers
            .get(&qualified)
            .ok_or_else(|| format!("capability `{qualified}` is not registered"))?;
        provider.call(member, args)
    }
}
