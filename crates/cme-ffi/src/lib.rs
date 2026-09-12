//! The stable C ABI for Checkmate hosts (WHITEPAPER §13.2), defined in
//! `include/cme.h` and implemented over [`cme_api`]. See the header for
//! the ownership, lifetime, and threading contract; this file is the
//! implementation half of that document.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;

use cme_api::{
    CapabilityProvider, CompiledProgram, Context, Engine, ErrorKind, ExecutionError,
    ExecutionLimits, Value,
};

/// One opaque engine handle. `_unique` guarantees a real, freeable
/// address: the Rust [`Engine`] is zero-sized, and `Box::into_raw` on a
/// zero-sized value yields a dangling-aligned pointer, which no C caller
/// should ever observe.
pub struct CmEngine {
    engine: Engine,
    _unique: u64,
}

/// One opaque program handle: the compiled program plus precomputed C
/// strings so borrowed name accessors never allocate.
pub struct CmProgram {
    program: CompiledProgram,
    entries: Vec<CString>,
    interfaces: Vec<CString>,
}

/// One opaque context handle. The Rust [`Context`] borrows its program;
/// the C ABI cannot express that borrow, so the lifetime is erased here
/// and the contract ("the program must outlive the context") lives in the
/// header instead.
pub struct CmContext {
    inner: Context<'static>,
}

/// One opaque future handle: an invocation that has already resolved
/// (synchronously, over the current interpreter) or is — formally —
/// pending, for the continuation-splitting VM era.
pub struct CmFuture {
    state: FutureState,
}

enum FutureState {
    /// Unreachable today; the header reserves the poll result for the
    /// continuation-splitting VM era. Kept constructed-inert so the poll
    /// path shows the reserved arm explicitly.
    #[allow(dead_code)]
    Pending,
    Ready(ReadyState),
}

enum ReadyState {
    Ok(Option<Value>),
    Err(ExecutionError),
}

/// One opaque value handle.
pub struct CmValue(Value);

// The handles are owned exclusively by the C caller and every accessor is
// designed around single-threaded use of a given handle, so the raw
// pointer exchange requires Send (+ Sync for the immutable shareables the
// header promises: engine and program). Contexts are documented
// multi-thread-safe for invocation; they hold no interior mutability.
unsafe impl Send for CmEngine {}
unsafe impl Sync for CmEngine {}
unsafe impl Send for CmProgram {}
unsafe impl Sync for CmProgram {}
unsafe impl Send for CmContext {}
unsafe impl Sync for CmContext {}
unsafe impl Send for CmFuture {}
unsafe impl Send for CmValue {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Converts a Rust string to a C string, replacing NUL bytes (which a
/// value's text may legitimately contain) with U+FFFD so rendering never
/// truncates. Returns an owned allocation.
fn to_cstring(text: &str) -> CString {
    let cleaned: String = if text.contains('\0') {
        text.replace('\0', "\u{FFFD}")
    } else {
        text.to_string()
    };
    CString::new(cleaned).unwrap_or_else(|_| CString::default())
}

/// Borrows a C string argument, or `None` for NULL / non-UTF-8 (the
/// caller maps that to the documented misuse behavior). The returned
/// borrow is unbounded because it points into caller-owned C memory.
unsafe fn borrow_str(pointer: *const c_char) -> Option<&'static str> {
    if pointer.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(pointer) }.to_str().ok()
}

fn clamp_i32(value: i64) -> i32 {
    value.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// Fills `out_error` with the success report (kind NONE, empty strings) —
/// the header promises load functions ALWAYS write the report.
unsafe fn fill_success(out_error: *mut CmError) {
    if out_error.is_null() {
        return;
    }
    unsafe {
        ptr::write(
            out_error,
            CmError {
                kind: CmErrorKind::None as i32,
                message: to_cstring("").into_raw(),
                file: to_cstring("").into_raw(),
                line: 0,
                column: 0,
            },
        );
    }
}

/// Fill for load-level errors that never reached the pipeline (IO,
/// invalid arguments).
unsafe fn fill_plain_error(out_error: *mut CmError, kind: CmErrorKind, message: &str) {
    unsafe {
        if out_error.is_null() {
            return;
        }
        ptr::write(
            out_error,
            CmError {
                kind: kind as i32,
                message: to_cstring(message).into_raw(),
                file: to_cstring("").into_raw(),
                line: 0,
                column: 0,
            },
        );
    }
}

/// The C-side projection of `cm_error_t` (layout-matched to the header).
#[repr(C)]
pub struct CmError {
    pub kind: i32,
    pub message: *mut c_char,
    pub file: *mut c_char,
    pub line: i32,
    pub column: i32,
}

impl CmError {
    /// A freshly zeroed report for Rust-side callers exercising the ABI
    /// (the header's "freshly zero-initialized" convention). C callers
    /// write `cm_error_t err = CM_ERROR_INIT;`.
    pub fn zeroed() -> CmError {
        // SAFETY: every field is a plain integer or pointer; an all-zero
        // bit pattern is the documented initial state.
        unsafe { std::mem::zeroed() }
    }
}

/// The C-side projection of `cm_limits_t`.
#[repr(C)]
pub struct CmLimits {
    pub fuel: u64,
    pub deadline_ms: u64,
    pub max_call_depth: usize,
}

enum CmErrorKind {
    None = 0,
    Compile = 1,
    Runtime = 2,
    Limit = 3,
    UnknownEntry = 4,
    InvalidArg = 5,
    Io = 6,
}

fn limits_from_c(limits: *const CmLimits) -> ExecutionLimits {
    if limits.is_null() {
        return ExecutionLimits::default();
    }
    // SAFETY: the caller passes a pointer to a valid cm_limits_t per the header.
    let limits = unsafe { &*limits };
    ExecutionLimits {
        fuel: if limits.fuel == 0 {
            None
        } else {
            Some(limits.fuel)
        },
        deadline_ms: if limits.deadline_ms == 0 {
            None
        } else {
            Some(limits.deadline_ms)
        },
        max_call_depth: limits.max_call_depth,
    }
}

fn value_kind(value: Option<&Value>) -> i32 {
    match value {
        None => 0, // CM_VALUE_INVALID
        Some(value) => match value {
            Value::Void => 1,
            Value::Int(_) => 2,
            Value::Float(_) => 3,
            Value::Bool(_) => 4,
            Value::Str(_) => 5,
            Value::Struct { .. } => 6,
            Value::Enum { .. } => 7,
            Value::Array(_) => 8,
            Value::Map(_) => 9,
        },
    }
}

// ---------------------------------------------------------------------------
// Version
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn cm_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// # Safety
/// `error` may be NULL; otherwise it must be a valid `cm_error_t*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_error_free(error: *mut CmError) {
    if error.is_null() {
        return;
    }
    unsafe {
        let report = &mut *error;
        for text in [&raw mut report.message, &raw mut report.file] {
            if !(*text).is_null() {
                drop(CString::from_raw(*text));
                *text = ptr::null_mut();
            }
        }
        report.kind = CmErrorKind::None as i32;
        report.line = 0;
        report.column = 0;
    }
}

/// # Safety
/// `text` must be a pointer handed out by `cm_value_to_string`, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_string_free(text: *mut c_char) {
    if !text.is_null() {
        unsafe { drop(CString::from_raw(text)) };
    }
}

// ---------------------------------------------------------------------------
// Engine and program lifecycle
// ---------------------------------------------------------------------------

/// # Safety
/// Pure constructor.
#[unsafe(no_mangle)]
pub extern "C" fn cm_engine_new() -> *mut CmEngine {
    Box::into_raw(Box::new(CmEngine {
        engine: Engine::new(),
        _unique: 0,
    }))
}

/// # Safety
/// `engine` must be a live handle or NULL; never use it again afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_engine_destroy(engine: *mut CmEngine) {
    if !engine.is_null() {
        unsafe { drop(Box::from_raw(engine)) };
    }
}

// ---------------------------------------------------------------------------
// Schema contract (§9) and capability providers (§9.1, §13.2)
// ---------------------------------------------------------------------------

/// One opaque schema handle: a parsed `.cm` schema file.
pub struct CmSchema(cme_compiler::schema::SchemaFile);

/// # Safety
/// `text` must be a valid NUL-terminated string (or NULL); `out_error`
/// follows the header fill convention.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_schema_parse(
    text: *const c_char,
    out_error: *mut CmError,
) -> *mut CmSchema {
    unsafe {
        let Some(text) = borrow_str(text) else {
            fill_plain_error(out_error, CmErrorKind::InvalidArg, "schema text is NULL");
            return ptr::null_mut();
        };
        let _ = &text;
        let outcome = cme_compiler::schema::parse_schema_file(text);
        if !outcome.is_clean() || outcome.file.is_none() {
            let listed: String = outcome
                .diagnostics
                .iter()
                .map(|d| d.message())
                .collect::<Vec<_>>()
                .join("; ");
            fill_plain_error(out_error, CmErrorKind::Compile, &listed);
            return ptr::null_mut();
        }
        fill_success(out_error);
        Box::into_raw(Box::new(CmSchema(
            outcome.file.expect("clean parse yields the file"),
        )))
    }
}

/// # Safety
/// `path` must be a valid NUL-terminated string (or NULL); `out_error`
/// follows the header fill convention.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_schema_parse_file(
    path: *const c_char,
    out_error: *mut CmError,
) -> *mut CmSchema {
    unsafe {
        let Some(path) = borrow_str(path) else {
            fill_plain_error(out_error, CmErrorKind::InvalidArg, "schema path is NULL");
            return ptr::null_mut();
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                fill_plain_error(
                    out_error,
                    CmErrorKind::Io,
                    &format!("cannot read {path}: {error}"),
                );
                return ptr::null_mut();
            }
        };
        let outcome = cme_compiler::schema::parse_schema_file(&text);
        if !outcome.is_clean() || outcome.file.is_none() {
            let listed: String = outcome
                .diagnostics
                .iter()
                .map(|d| d.message())
                .collect::<Vec<_>>()
                .join("; ");
            fill_plain_error(out_error, CmErrorKind::Compile, &listed);
            return ptr::null_mut();
        }
        fill_success(out_error);
        Box::into_raw(Box::new(CmSchema(
            outcome.file.expect("clean parse yields the file"),
        )))
    }
}

/// # Safety
/// `schema` must be a live handle or NULL; never use it again afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_schema_destroy(schema: *mut CmSchema) {
    if !schema.is_null() {
        unsafe { drop(Box::from_raw(schema)) };
    }
}

/// # Safety
/// `engine` and `schema` must be live handles (or NULL, which fails with
/// CM_ERR_NULL semantics as CM_ERROR_INVALID_ARG in `out_error`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_engine_register_schema(
    engine: *mut CmEngine,
    schema: *const CmSchema,
) -> c_int {
    unsafe {
        if engine.is_null() || schema.is_null() {
            return 5; // CM_ERR_INVALID_ARG
        }
        let engine = &mut (*engine).engine;
        match engine.register_schema((*schema).0.clone()) {
            Ok(()) => 0, // CM_OK
            // The set failed re-validation; the engine is unchanged.
            Err(_) => 5, // CM_ERR_INVALID_ARG
        }
    }
}

/// # Safety
/// `engine` must be live; `path` and `members` must be valid (or NULL);
/// `user` is passed back to every provider call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_engine_register_capability(
    engine: *mut CmEngine,
    path: *const c_char,
    members: *const CmCapabilityMember,
    count: usize,
    user: *mut core::ffi::c_void,
) -> c_int {
    unsafe {
        if engine.is_null() {
            return 5; // CM_ERR_INVALID_ARG
        }
        let Some(path) = borrow_str(path) else {
            return 5;
        };
        if members.is_null() || count == 0 {
            return 5;
        }
        let member_slice = std::slice::from_raw_parts(members, count);
        let mut converted: Vec<(String, CmCapabilityFn)> = Vec::with_capacity(count);
        for member in member_slice {
            let Some(name) = borrow_str(member.name) else {
                return 5;
            };
            let Some(c_fn) = member.fn_ else {
                return 5;
            };
            converted.push((name.to_string(), c_fn));
        }

        let provider = CProvider {
            user,
            members: converted,
        };
        match (*engine)
            .engine
            .register_capability(path, std::sync::Arc::new(provider))
        {
            Ok(()) => 0, // CM_OK
            Err(_) => 5, // CM_ERR_INVALID_ARG (bad path shape)
        }
    }
}

/// The C-side projection of `cm_capability_member_t`.
#[repr(C)]
pub struct CmCapabilityMember {
    pub name: *const c_char,
    pub fn_: Option<CmCapabilityFn>,
}

pub type CmCapabilityFn = unsafe extern "C" fn(
    user: *mut core::ffi::c_void,
    args: *const *mut CmValue,
    argc: usize,
    out_error: *mut CmError,
) -> *mut CmValue;

/// The Rust half of a registered C provider: dispatches member names to
/// the C function pointers, converting values across the ABI boundary.
struct CProvider {
    user: *mut core::ffi::c_void,
    members: Vec<(String, CmCapabilityFn)>,
}

// SAFETY: `user` is opaque C memory whose threading contract the header
// states ("the same provider function may run concurrently across
// contexts"); the member table is immutable after registration.
unsafe impl Send for CProvider {}
unsafe impl Sync for CProvider {}

impl CapabilityProvider for CProvider {
    fn call(&self, member: &str, args: &[Value]) -> Result<Value, String> {
        let Some((_, c_fn)) = self.members.iter().find(|(name, _)| name == member) else {
            return Err(format!("capability provider has no member `{member}`"));
        };
        // Borrowed argument handles: boxed clones the C side may inspect
        // (never free) for the duration of the call.
        let handles: Vec<*mut CmValue> = args
            .iter()
            .map(|value| Box::into_raw(Box::new(CmValue(value.clone()))))
            .collect();
        let out_error = Box::into_raw(Box::new(CmError::zeroed()));
        let result = unsafe { c_fn(self.user, handles.as_ptr(), handles.len(), out_error) };
        let report = unsafe { Box::from_raw(out_error) };
        // Free the borrowed argument boxes.
        for handle in handles {
            unsafe { drop(Box::from_raw(handle)) };
        }
        if result.is_null() {
            let report_message = report.message;
            let message = unsafe { cstr_to_string(report_message) }
                .unwrap_or_else(|| "capability call failed".to_string());
            return Err(message);
        }
        // The returned handle is OWNED: take the value, drop the box.
        let value = unsafe { *Box::from_raw(result) };
        Ok(value.0)
    }
}

unsafe fn cstr_to_string(pointer: *mut c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .ok()
        .map(String::from)
}

/// # Safety
/// `engine` must be live; `source` must be a valid NUL-terminated string
/// (or NULL); `out_error` follows the header fill convention.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_engine_load_source(
    engine: *mut CmEngine,
    source: *const c_char,
    out_error: *mut CmError,
) -> *mut CmProgram {
    unsafe {
        let Some(text) = borrow_str(source) else {
            fill_plain_error(
                out_error,
                CmErrorKind::InvalidArg,
                "source must be a non-NULL UTF-8 string",
            );
            return ptr::null_mut();
        };
        let Some(engine) = engine.as_ref() else {
            fill_plain_error(out_error, CmErrorKind::InvalidArg, "engine must be live");
            return ptr::null_mut();
        };
        match engine.engine.load_source(text) {
            Ok(program) => {
                fill_success(out_error);
                Box::into_raw(Box::new(CmProgram::new(program)))
            }
            Err(error) => {
                fill_plain_error(out_error, CmErrorKind::Compile, &error.message());
                ptr::null_mut()
            }
        }
    }
}

/// # Safety
/// Same contract as [`cm_engine_load_source`] with `path` naming a file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_engine_load_file(
    engine: *mut CmEngine,
    path: *const c_char,
    out_error: *mut CmError,
) -> *mut CmProgram {
    unsafe {
        let Some(path) = borrow_str(path) else {
            fill_plain_error(
                out_error,
                CmErrorKind::InvalidArg,
                "path must be a non-NULL UTF-8 string",
            );
            return ptr::null_mut();
        };
        let Some(engine) = engine.as_ref() else {
            fill_plain_error(out_error, CmErrorKind::InvalidArg, "engine must be live");
            return ptr::null_mut();
        };
        match engine.engine.load_file(path) {
            Ok(program) => {
                fill_success(out_error);
                Box::into_raw(Box::new(CmProgram::new(program)))
            }
            Err(cme_api::LoadError::Io(message)) => {
                fill_plain_error(out_error, CmErrorKind::Io, &message);
                ptr::null_mut()
            }
            Err(cme_api::LoadError::Compile(error)) => {
                fill_plain_error(out_error, CmErrorKind::Compile, &error.message());
                ptr::null_mut()
            }
        }
    }
}

/// # Safety
/// Same contract as [`cm_engine_load_source`] with `root` naming a mod
/// directory or its mod.toml.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_engine_load_mod(
    engine: *mut CmEngine,
    root: *const c_char,
    out_error: *mut CmError,
) -> *mut CmProgram {
    unsafe {
        let Some(root) = borrow_str(root) else {
            fill_plain_error(
                out_error,
                CmErrorKind::InvalidArg,
                "root must be a non-NULL UTF-8 string",
            );
            return ptr::null_mut();
        };
        let Some(engine) = engine.as_ref() else {
            fill_plain_error(out_error, CmErrorKind::InvalidArg, "engine must be live");
            return ptr::null_mut();
        };
        match engine.engine.load_mod(root) {
            Ok(program) => {
                fill_success(out_error);
                Box::into_raw(Box::new(CmProgram::new(program)))
            }
            Err(error) => {
                fill_plain_error(out_error, CmErrorKind::Compile, &error.message());
                ptr::null_mut()
            }
        }
    }
}

impl CmProgram {
    fn new(program: CompiledProgram) -> CmProgram {
        CmProgram {
            entries: program
                .entry_points()
                .iter()
                .map(|name| to_cstring(name))
                .collect(),
            interfaces: program
                .interface_targets()
                .iter()
                .map(|name| to_cstring(name))
                .collect(),
            program,
        }
    }
}

/// # Safety
/// `program` must be a live handle or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_program_destroy(program: *mut CmProgram) {
    if !program.is_null() {
        unsafe { drop(Box::from_raw(program)) };
    }
}

/// # Safety
/// `program` must be live (or NULL → 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_program_entry_count(program: *const CmProgram) -> usize {
    match unsafe { program.as_ref() } {
        Some(program) => program.entries.len(),
        None => 0,
    }
}

/// # Safety
/// `program` must be live; the returned pointer borrows from it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_program_entry_name(
    program: *const CmProgram,
    index: usize,
) -> *const c_char {
    match unsafe { program.as_ref() } {
        Some(program) => program
            .entries
            .get(index)
            .map_or(ptr::null(), |name| name.as_ptr()),
        None => ptr::null(),
    }
}

/// # Safety
/// `program` must be live (or NULL → 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_program_interface_count(program: *const CmProgram) -> usize {
    match unsafe { program.as_ref() } {
        Some(program) => program.interfaces.len(),
        None => 0,
    }
}

/// # Safety
/// `program` must be live; the returned pointer borrows from it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_program_interface_name(
    program: *const CmProgram,
    index: usize,
) -> *const c_char {
    match unsafe { program.as_ref() } {
        Some(program) => program
            .interfaces
            .get(index)
            .map_or(ptr::null(), |name| name.as_ptr()),
        None => ptr::null(),
    }
}

/// # Safety
/// `program` must be live (or NULL → 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_program_is_mod(program: *const CmProgram) -> c_int {
    match unsafe { program.as_ref() } {
        Some(program) => c_int::from(program.program.is_mod()),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Contexts
// ---------------------------------------------------------------------------

/// # Safety
/// `engine` must be live; `program` must be live and must outlive the
/// returned context; `limits` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_engine_create_context(
    engine: *mut CmEngine,
    program: *const CmProgram,
    limits: *const CmLimits,
) -> *mut CmContext {
    unsafe {
        let Some(engine) = engine.as_ref() else {
            return ptr::null_mut();
        };
        let Some(program) = program.as_ref() else {
            return ptr::null_mut();
        };
        let context = engine
            .engine
            .create_context(&program.program, limits_from_c(limits));
        // Erase the borrow: the header's lifetime contract ("programs
        // outlive contexts") covers what the type system tracked in Rust.
        let context: Context<'static> = std::mem::transmute(context);
        Box::into_raw(Box::new(CmContext { inner: context }))
    }
}

/// # Safety
/// `context` must be a live handle or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_context_destroy(context: *mut CmContext) {
    if !context.is_null() {
        unsafe { drop(Box::from_raw(context)) };
    }
}

// ---------------------------------------------------------------------------
// Invocation
// ---------------------------------------------------------------------------

/// # Safety
/// Per the header: `context` live, `member` non-NULL, `args` (when
/// non-NULL with argc > 0) a valid array of live value handles. The
/// arguments are borrowed; ownership stays with the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_invoke(
    context: *mut CmContext,
    target: *const c_char,
    member: *const c_char,
    args: *const *const CmValue,
    argc: usize,
) -> *mut CmFuture {
    unsafe {
        let Some(context) = context.as_ref() else {
            return ptr::null_mut();
        };
        let Some(member) = borrow_str(member) else {
            return ptr::null_mut();
        };
        let target = borrow_str(target).unwrap_or("");

        if args.is_null() && argc > 0 {
            return ptr::null_mut();
        }
        let mut arguments: Vec<Value> = Vec::with_capacity(argc.min(4096));
        if argc > 0 {
            let args = std::slice::from_raw_parts(args, argc);
            for &argument in args {
                let Some(value) = argument.as_ref() else {
                    return ptr::null_mut();
                };
                arguments.push(value.0.clone());
            }
        }

        let result = if target.is_empty() {
            context.inner.invoke(member, &arguments)
        } else {
            context.inner.invoke_member(target, member, &arguments)
        };
        match result {
            Ok(value) => Box::into_raw(Box::new(CmFuture {
                state: FutureState::Ready(ReadyState::Ok(Some(value))),
            })),
            Err(error) => Box::into_raw(Box::new(CmFuture {
                state: FutureState::Ready(ReadyState::Err(error)),
            })),
        }
    }
}

// ---------------------------------------------------------------------------
// Futures
// ---------------------------------------------------------------------------

/// # Safety
/// `future` must be live (or NULL → CM_ERROR); `out_value` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_future_poll(
    future: *mut CmFuture,
    out_value: *mut *mut CmValue,
) -> c_int {
    unsafe {
        let Some(future) = future.as_mut() else {
            return 2; // CM_ERROR: a dead future reports failure, never success.
        };
        if !out_value.is_null() {
            ptr::write(out_value, ptr::null_mut());
        }
        match &mut future.state {
            FutureState::Pending => 0, // CM_PENDING
            FutureState::Ready(ReadyState::Ok(value)) => {
                if !out_value.is_null() {
                    let taken = value
                        .take()
                        .map(|value| Box::into_raw(Box::new(CmValue(value))))
                        .unwrap_or(ptr::null_mut());
                    ptr::write(out_value, taken);
                }
                1 // CM_READY
            }
            FutureState::Ready(ReadyState::Err(_)) => 2, // CM_ERROR
        }
    }
}

/// # Safety
/// `future` must be live (or NULL → NULL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_future_take_value(future: *mut CmFuture) -> *mut CmValue {
    unsafe {
        let Some(future) = future.as_mut() else {
            return ptr::null_mut();
        };
        match &mut future.state {
            FutureState::Ready(ReadyState::Ok(value)) => value
                .take()
                .map(|value| Box::into_raw(Box::new(CmValue(value))))
                .unwrap_or(ptr::null_mut()),
            _ => ptr::null_mut(),
        }
    }
}

/// # Safety
/// `future` must be live (or NULL → a NONE report).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_future_get_error(future: *mut CmFuture) -> CmError {
    unsafe {
        match future.as_ref() {
            Some(future) => match &future.state {
                FutureState::Ready(ReadyState::Err(error)) => CmError {
                    kind: match error.kind {
                        ErrorKind::Runtime => CmErrorKind::Runtime,
                        ErrorKind::Budget | ErrorKind::Deadline | ErrorKind::CallDepth => {
                            CmErrorKind::Limit
                        }
                        ErrorKind::UnknownEntry => CmErrorKind::UnknownEntry,
                        // §5.7 reentrancy is host misuse: the INVALID_ARG
                        // family, with the full explanation in the message.
                        ErrorKind::Reentrant => CmErrorKind::InvalidArg,
                    } as i32,
                    message: to_cstring(&error.message).into_raw(),
                    file: to_cstring(error.file.as_deref().unwrap_or("")).into_raw(),
                    line: clamp_i32(error.line as i64),
                    column: clamp_i32(error.column as i64),
                },
                _ => CmError {
                    kind: CmErrorKind::None as i32,
                    message: to_cstring("").into_raw(),
                    file: to_cstring("").into_raw(),
                    line: 0,
                    column: 0,
                },
            },
            None => CmError {
                kind: CmErrorKind::None as i32,
                message: to_cstring("").into_raw(),
                file: to_cstring("").into_raw(),
                line: 0,
                column: 0,
            },
        }
    }
}

/// # Safety
/// `future` must be a live handle or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_future_destroy(future: *mut CmFuture) {
    if !future.is_null() {
        unsafe { drop(Box::from_raw(future)) };
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// # Safety
/// Pure constructor.
#[unsafe(no_mangle)]
pub extern "C" fn cm_value_void() -> *mut CmValue {
    Box::into_raw(Box::new(CmValue(Value::Void)))
}

/// # Safety
/// Pure constructor.
#[unsafe(no_mangle)]
pub extern "C" fn cm_value_int(value: i64) -> *mut CmValue {
    Box::into_raw(Box::new(CmValue(Value::Int(value))))
}

/// # Safety
/// Pure constructor.
#[unsafe(no_mangle)]
pub extern "C" fn cm_value_float(value: f64) -> *mut CmValue {
    Box::into_raw(Box::new(CmValue(Value::Float(value))))
}

/// # Safety
/// Pure constructor.
#[unsafe(no_mangle)]
pub extern "C" fn cm_value_bool(value: c_int) -> *mut CmValue {
    Box::into_raw(Box::new(CmValue(Value::Bool(value != 0))))
}

/// # Safety
/// `text` must be NUL-terminated valid UTF-8, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_str(text: *const c_char) -> *mut CmValue {
    unsafe {
        match borrow_str(text) {
            Some(text) => Box::into_raw(Box::new(CmValue(Value::Str(text.to_string())))),
            None => ptr::null_mut(),
        }
    }
}

/// # Safety
/// `text` must reference `length` valid UTF-8 bytes, or be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_str_len(text: *const c_char, length: usize) -> *mut CmValue {
    unsafe {
        if text.is_null() {
            return ptr::null_mut();
        }
        let bytes = std::slice::from_raw_parts(text as *const u8, length);
        match std::str::from_utf8(bytes) {
            Ok(text) => Box::into_raw(Box::new(CmValue(Value::Str(text.to_string())))),
            Err(_) => ptr::null_mut(),
        }
    }
}

/// # Safety
/// `type_name` must be NUL-terminated valid UTF-8, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_struct(type_name: *const c_char) -> *mut CmValue {
    unsafe {
        match borrow_str(type_name) {
            Some(name) => Box::into_raw(Box::new(CmValue(Value::Struct {
                name: name.to_string(),
                fields: Vec::new(),
            }))),
            None => ptr::null_mut(),
        }
    }
}

/// # Safety
/// Both names must be NUL-terminated valid UTF-8, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_enum(
    type_name: *const c_char,
    variant: *const c_char,
) -> *mut CmValue {
    unsafe {
        match (borrow_str(type_name), borrow_str(variant)) {
            (Some(type_name), Some(variant)) => Box::into_raw(Box::new(CmValue(Value::Enum {
                name: type_name.to_string(),
                variant: variant.to_string(),
                payload: Vec::new(),
            }))),
            _ => ptr::null_mut(),
        }
    }
}

/// # Safety
/// Pure constructor.
#[unsafe(no_mangle)]
pub extern "C" fn cm_value_array() -> *mut CmValue {
    Box::into_raw(Box::new(CmValue(Value::Array(Vec::new()))))
}

/// # Safety
/// Pure constructor.
#[unsafe(no_mangle)]
pub extern "C" fn cm_value_map() -> *mut CmValue {
    Box::into_raw(Box::new(CmValue(Value::Map(Vec::new()))))
}

/// # Safety
/// `strukt` must be a live struct handle; `name` valid UTF-8; `field` a
/// live value handle consumed on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_struct_set_field(
    strukt: *mut CmValue,
    name: *const c_char,
    field: *mut CmValue,
) -> c_int {
    unsafe {
        let Some(strukt) = strukt.as_mut() else {
            return 1; // CM_ERR_NULL
        };
        let Some(name) = borrow_str(name) else {
            return 1;
        };
        let Value::Struct { fields, .. } = &mut strukt.0 else {
            return 2; // CM_ERR_KIND
        };
        let Some(field_ref) = field.as_ref() else {
            return 1;
        };
        let field_value = field_ref.0.clone();
        match fields.iter_mut().find(|(existing, _)| existing == name) {
            Some(slot) => slot.1 = field_value,
            None => fields.push((name.to_string(), field_value)),
        }
        // Consumed on success: release the caller's handle.
        drop(Box::from_raw(field));
        0 // CM_OK
    }
}

/// # Safety
/// `enum_value` must be a live enum handle; `payload` consumed on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_enum_push(enum_value: *mut CmValue, payload: *mut CmValue) -> c_int {
    unsafe {
        let Some(enum_value) = enum_value.as_mut() else {
            return 1;
        };
        let Value::Enum {
            payload: payloads, ..
        } = &mut enum_value.0
        else {
            return 2;
        };
        let Some(payload_ref) = payload.as_ref() else {
            return 1;
        };
        payloads.push(payload_ref.0.clone());
        drop(Box::from_raw(payload));
        0
    }
}

/// # Safety
/// `array` must be a live array handle; `item` consumed on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_array_push(array: *mut CmValue, item: *mut CmValue) -> c_int {
    unsafe {
        let Some(array) = array.as_mut() else {
            return 1;
        };
        let Value::Array(elements) = &mut array.0 else {
            return 2;
        };
        let Some(item_ref) = item.as_ref() else {
            return 1;
        };
        elements.push(item_ref.0.clone());
        drop(Box::from_raw(item));
        0
    }
}

/// # Safety
/// `map` must be a live map handle; `key` and `value` consumed on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_map_set(
    map: *mut CmValue,
    key: *mut CmValue,
    value: *mut CmValue,
) -> c_int {
    unsafe {
        let Some(map) = map.as_mut() else {
            return 1;
        };
        let Value::Map(entries) = &mut map.0 else {
            return 2;
        };
        let (Some(key_ref), Some(value_ref)) = (key.as_ref(), value.as_ref()) else {
            return 1;
        };
        let key_value = key_ref.0.clone();
        let value_value = value_ref.0.clone();
        match entries
            .iter_mut()
            .find(|(existing, _)| *existing == key_value)
        {
            Some(slot) => slot.1 = value_value,
            None => entries.push((key_value, value_value)),
        }
        // Both child handles are consumed on success.
        drop(Box::from_raw(key));
        drop(Box::from_raw(value));
        0
    }
}

/// # Safety
/// `value` must be live (or NULL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_clone(value: *const CmValue) -> *mut CmValue {
    match unsafe { value.as_ref() } {
        Some(value) => Box::into_raw(Box::new(CmValue(value.0.clone()))),
        None => ptr::null_mut(),
    }
}

/// # Safety
/// `value` must be live (or NULL → CM_VALUE_INVALID).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_kind(value: *const CmValue) -> c_int {
    value_kind(unsafe { value.as_ref() }.map(|value| &value.0))
}

macro_rules! scalar_accessor {
    (
        $name:ident, $c_type:ty, $pattern:pat => $extract:expr
    ) => {
        /// # Safety
        /// `value` may be NULL; `out` must be writable when non-NULL.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(value: *const CmValue, out: *mut $c_type) -> c_int {
            unsafe {
                let Some(value) = value.as_ref() else {
                    return 1; // CM_ERR_NULL
                };
                if out.is_null() {
                    return 1;
                }
                match &value.0 {
                    $pattern => {
                        ptr::write(out, $extract);
                        0 // CM_OK
                    }
                    _ => 2, // CM_ERR_KIND
                }
            }
        }
    };
}

scalar_accessor!(cm_value_as_int, i64, Value::Int(v) => *v);
scalar_accessor!(cm_value_as_float, f64, Value::Float(v) => *v);
scalar_accessor!(cm_value_as_bool, c_int, Value::Bool(v) => c_int::from(*v));

/// # Safety
/// `value` may be NULL; `out` must be writable when non-NULL; `out_length`
/// may be NULL. The returned string is OWNED — free it with
/// `cm_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_as_str(
    value: *const CmValue,
    out: *mut *mut c_char,
    out_length: *mut usize,
) -> c_int {
    unsafe {
        let Some(value) = value.as_ref() else {
            return 1;
        };
        if out.is_null() {
            return 1;
        }
        match &value.0 {
            Value::Str(text) => {
                // Rust Strings are not NUL-terminated; hand out an owned
                // C copy instead of a raw borrow.
                ptr::write(out, to_cstring(text).into_raw());
                if !out_length.is_null() {
                    ptr::write(out_length, text.len());
                }
                0
            }
            _ => 2,
        }
    }
}

macro_rules! name_accessor {
    ($name:ident, $pattern:pat => $extract:expr) => {
        /// # Safety
        /// `value` may be NULL; `out` must be writable when non-NULL. The
        /// returned name is OWNED — free it with `cm_string_free`. (Rust
        /// Strings are not NUL-terminated, so a raw borrow would read
        /// past the end from C.)
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(value: *const CmValue, out: *mut *mut c_char) -> c_int {
            unsafe {
                let Some(value) = value.as_ref() else {
                    return 1;
                };
                if out.is_null() {
                    return 1;
                }
                match &value.0 {
                    $pattern => {
                        ptr::write(out, to_cstring($extract).into_raw());
                        0
                    }
                    _ => 2,
                }
            }
        }
    };
}

name_accessor!(cm_value_struct_name, Value::Struct { name, .. } => name);
name_accessor!(cm_value_enum_type, Value::Enum { name, .. } => name);
name_accessor!(cm_value_enum_variant, Value::Enum { variant, .. } => variant);

/// # Safety
/// `value` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_len(value: *const CmValue) -> usize {
    match unsafe { value.as_ref() }.map(|value| &value.0) {
        Some(Value::Str(text)) => text.len(),
        Some(Value::Struct { fields, .. }) => fields.len(),
        Some(Value::Enum { payload, .. }) => payload.len(),
        Some(Value::Array(elements)) => elements.len(),
        Some(Value::Map(entries)) => entries.len(),
        _ => 0,
    }
}

/// # Safety
/// `value` must be live (or NULL); the borrowed element lives as long as
/// the array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_array_get(value: *const CmValue, index: usize) -> *const CmValue {
    unsafe {
        match value.as_ref().map(|value| &value.0) {
            Some(Value::Array(elements)) => elements.get(index).map_or(ptr::null(), |element| {
                element as *const Value as *const CmValue
            }),
            _ => ptr::null(),
        }
    }
}

/// # Safety
/// `value` must be live (or NULL); the borrowed key lives as long as the map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_map_key(value: *const CmValue, index: usize) -> *const CmValue {
    unsafe {
        match value.as_ref().map(|value| &value.0) {
            Some(Value::Map(entries)) => entries.get(index).map_or(ptr::null(), |(key, _)| {
                key as *const Value as *const CmValue
            }),
            _ => ptr::null(),
        }
    }
}

/// # Safety
/// `value` must be live (or NULL); the borrowed payload lives as long as
/// the enum value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_enum_payload(
    value: *const CmValue,
    index: usize,
) -> *const CmValue {
    unsafe {
        match value.as_ref().map(|value| &value.0) {
            Some(Value::Enum { payload, .. }) => {
                payload.get(index).map_or(ptr::null(), |payload| {
                    payload as *const Value as *const CmValue
                })
            }
            _ => ptr::null(),
        }
    }
}

/// # Safety
/// `value` must be live (or NULL); the borrowed value lives as long as the map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_map_value(value: *const CmValue, index: usize) -> *const CmValue {
    unsafe {
        match value.as_ref().map(|value| &value.0) {
            Some(Value::Map(entries)) => entries.get(index).map_or(ptr::null(), |(_, value)| {
                value as *const Value as *const CmValue
            }),
            _ => ptr::null(),
        }
    }
}

/// # Safety
/// `value` must be live (or NULL); `name` must be valid UTF-8; the
/// borrowed field lives as long as the struct.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_struct_field(
    value: *const CmValue,
    name: *const c_char,
) -> *const CmValue {
    unsafe {
        let Some(name) = borrow_str(name) else {
            return ptr::null();
        };
        match value.as_ref().map(|value| &value.0) {
            Some(Value::Struct { fields, .. }) => fields
                .iter()
                .find(|(field_name, _)| field_name == name)
                .map_or(ptr::null(), |(_, field)| {
                    field as *const Value as *const CmValue
                }),
            _ => ptr::null(),
        }
    }
}

/// # Safety
/// `value` may be NULL. The returned string is owned; free it with
/// `cm_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_to_string(value: *const CmValue) -> *mut c_char {
    let text = match unsafe { value.as_ref() } {
        Some(value) => value.0.to_string(),
        None => String::new(),
    };
    to_cstring(&text).into_raw()
}

/// # Safety
/// `value` must be a live handle or NULL; never use it again afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cm_value_destroy(value: *mut CmValue) {
    if !value.is_null() {
        unsafe { drop(Box::from_raw(value)) };
    }
}
