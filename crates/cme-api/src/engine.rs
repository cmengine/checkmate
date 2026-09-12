//! The [`Engine`]: loads Checkmate programs and hands out execution
//! contexts (WHITEPAPER §13.1), against the schema contract and capability
//! providers the host registers (§9).

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use cme_compiler::check::check_with_schema;
use cme_compiler::mods::{self, LoadedModule, ModManifest, ModuleRange};
use cme_compiler::schema::{SchemaContext, SchemaFile, SchemaIssue, SchemaSet, parse_schema_file};
use cme_compiler::{Diagnostic, mega, parse_source};
use cme_core::ast::{Stmt, StmtKind};
use cme_interp::Value;

use crate::context::{Context, ExecutionLimits};
use crate::render::{SourceLayout, render_located};

/// The host side of a schema capability (§9.1, §13.1): the provider a
/// script's `engine.graphics.LoadTexture(...)` call dispatches to. The
/// generated bindings (WHITEPAPER §9.6) implement this over a typed trait
/// so the host's implementation is verified against the schema at host
/// compile time; a direct implementation works for ad-hoc providers.
///
/// Arguments arrive as positional [`Value`]s in schema declaration order,
/// cloned out of the script (§2.13 value semantics). A member the provider
/// does not implement returns `Err` — the interpreter anchors the message
/// at the call site.
pub trait CapabilityProvider: Send + Sync {
    /// Invokes schema member `member` of the capability this provider is
    /// registered for. `Ok` carries the member's declared return type
    /// (`Value::Void` for `void` members); `Err(message)` fails the
    /// invocation.
    fn call(&self, member: &str, args: &[Value]) -> Result<Value, String>;
}

/// A schema registration failure: the schema file parsed (or the set
/// reassembled) with defects, listed rendered and structured.
#[derive(Debug, Clone)]
pub struct SchemaError {
    messages: Vec<String>,
}

impl SchemaError {
    fn from_issues(issues: &[SchemaIssue]) -> SchemaError {
        SchemaError {
            messages: issues.iter().map(|issue| issue.message.clone()).collect(),
        }
    }

    /// One rendered line per defect.
    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    /// Every defect joined by newlines — the Display body.
    pub fn message(&self) -> String {
        self.messages.join("\n")
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for SchemaError {}

/// The host-facing entry to Checkmate: load programs, create contexts
/// (WHITEPAPER §13.1). The engine holds the host's side of the contract —
/// the registered schema files (§9.2) and the capability providers
/// (§9.1) — so every load checks programs against the ACTIVE schema and
/// every context dispatches capability calls to the registered providers.
///
/// ```
/// let mut engine = cme_api::Engine::new();
/// let program = engine.load_source("int main() {\nreturn 1\n}\n")?;
/// assert_eq!(program.entry_points(), ["main"]);
/// # Ok::<(), cme_api::CompileError>(())
/// ```
#[derive(Clone, Default)]
pub struct Engine {
    /// Every registered schema namespace (§9.2), kept in registration
    /// order and re-validated as a set on each registration.
    schemas: Vec<SchemaFile>,
    /// Capability providers keyed by their schema path
    /// (`engine.graphics`, §9.1).
    providers: HashMap<String, Arc<dyn CapabilityProvider>>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Providers are opaque; the registered paths say what matters.
        formatter
            .debug_struct("Engine")
            .field("schemas", &self.schemas)
            .field("capabilities", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Engine {
    /// A fresh engine handle: no schemas, no providers. With no schema
    /// registered, loads keep the pre-schema behavior (host-rooted imports
    /// are accepted as host-style paths and nothing is schema-gated).
    pub fn new() -> Engine {
        Engine::default()
    }

    /// Registers a parsed schema file (§9.2: one namespace root per file).
    /// The whole set is re-validated — duplicate namespaces, cross-schema
    /// type collisions, and unresolved `requires` edges (§9.4) fail the
    /// registration, leaving the engine unchanged.
    pub fn register_schema(&mut self, schema: SchemaFile) -> Result<(), SchemaError> {
        let mut schemas = self.schemas.clone();
        schemas.push(schema);
        SchemaSet::build(schemas)
            .map(|set| {
                self.schemas = set.namespaces().to_vec();
            })
            .map_err(|issues| SchemaError::from_issues(&issues))
    }

    /// Reads, parses, and registers a `.cm` schema file. Parse diagnostics
    /// render with `path:line:column` locations naming the file.
    pub fn load_schema_file(&mut self, path: impl AsRef<Path>) -> Result<(), SchemaError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|error| SchemaError {
            messages: vec![format!("cannot read {}: {error}", path.display())],
        })?;
        self.load_schema_text(&text, &path.display().to_string())
    }

    /// Parses and registers schema source text; `name` becomes the
    /// diagnostic prefix (empty for plain text).
    pub fn load_schema_text(&mut self, text: &str, name: &str) -> Result<(), SchemaError> {
        let outcome = parse_schema_file(text);
        if !outcome.is_clean() {
            let layout = SourceLayout::new(text);
            return Err(SchemaError {
                messages: outcome
                    .diagnostics
                    .iter()
                    .map(|diagnostic| {
                        render_located(
                            diagnostic.message(),
                            diagnostic.span(),
                            text,
                            if name.is_empty() { None } else { Some(name) },
                            &layout,
                        )
                    })
                    .collect(),
            });
        }
        self.register_schema(outcome.file.expect("a clean parse yields the file"))
    }

    /// Registers the provider for a schema capability. `path` is the full
    /// capability path (`engine.graphics`, §9.1) — two identifier
    /// segments. The schema itself may be registered later; the
    /// provider-presence check runs at LOAD time, when the contract is
    /// known.
    pub fn register_capability(
        &mut self,
        path: impl Into<String>,
        provider: Arc<dyn CapabilityProvider>,
    ) -> Result<(), String> {
        let path = path.into();
        let segments: Vec<&str> = path.split('.').collect();
        if segments.len() != 2
            || segments.iter().any(|segment| {
                segment.is_empty()
                    || !segment
                        .chars()
                        .next()
                        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                    || !segment
                        .chars()
                        .all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
            })
        {
            return Err(format!(
                "capability path must be `namespace.capability` (§9.1), got {path:?}"
            ));
        }
        self.providers.insert(path, provider);
        Ok(())
    }

    /// The registered schema namespaces, in registration order.
    pub fn schema_namespaces(&self) -> Vec<String> {
        self.schemas
            .iter()
            .map(|file| file.namespace.clone())
            .collect()
    }

    /// The registered capability paths (`engine.graphics`), sorted.
    pub fn capability_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.providers.keys().cloned().collect();
        paths.sort();
        paths
    }

    /// Compiles a single source text into a runnable program. The exact
    /// gate the CLI's `run` applies: megaprograms expand first when the
    /// source mentions the subsystem (§8), then the front end parses, the
    /// §10 standalone-import check rejects `import self.*` outside a mod,
    /// and the type checker runs — against the ACTIVE schema when one is
    /// registered (§9). Any diagnostic — from any stage — fails the load; a
    /// program that produced one never becomes invocable.
    pub fn load_source(&self, source: &str) -> Result<CompiledProgram, CompileError> {
        build_source(self, source, None)
    }

    /// Reads and compiles a `.cm` file. Identical to [`Engine::load_source`]
    /// except I/O failures surface as [`LoadError::Io`] and diagnostics
    /// render with `path:line:column` locations naming the file.
    pub fn load_file(&self, path: impl AsRef<Path>) -> Result<CompiledProgram, LoadError> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path)
            .map_err(|error| LoadError::Io(format!("cannot read {}: {error}", path.display())))?;
        build_source(self, &source, Some(&path.display().to_string())).map_err(LoadError::Compile)
    }

    /// Compiles a whole §10 mod: a directory holding `mod.toml` and a
    /// `src/` tree (the path may also name the `mod.toml` itself, as the
    /// CLI accepts). Modules expand individually, then link into one
    /// program; every compile diagnostic re-anchors to its owning module.
    /// The mod's `[schemas]` table (§10.2) narrows the schema grant: only
    /// listed namespaces are visible, at the declared target versions
    /// (§9.5).
    pub fn load_mod(&self, root: impl AsRef<Path>) -> Result<CompiledProgram, CompileError> {
        build_mod(self, root.as_ref())
    }

    /// Creates an execution context over `program` under §5.5 limits.
    /// The context borrows the program: destroy the context first, or —
    /// since borrows enforce it — never outlive the program with one.
    /// Programs are immutable after loading, so any number of contexts
    /// (and invocations) may share one concurrently. The context snapshots
    /// the currently registered capability providers; registering later
    /// affects only future contexts.
    pub fn create_context<'p>(
        &self,
        program: &'p CompiledProgram,
        limits: ExecutionLimits,
    ) -> Context<'p> {
        Context::new(program, limits, Arc::new(self.providers.clone()))
    }
}

/// Whether a program loaded from loose source text or a §10 mod tree.
/// The kind decides which text spans refer to and where diagnostics point.
#[derive(Debug, Clone)]
pub enum ProgramKind {
    /// One source text (possibly megaprogram-expanded). Spans and rendered
    /// positions refer to `source` — the EXPANDED text when expansion ran,
    /// matching the coordinates the checker itself produced.
    Source {
        source: String,
        /// The file name when the source came from [`Engine::load_file`].
        name: Option<String>,
    },
    /// A linked §10 mod tree. `assembled` is the virtual program text the
    /// checker saw; execution diagnostics carry re-anchored module-local
    /// spans, so hosts render positions from `modules`, not `assembled`.
    Mod {
        root: String,
        assembled: String,
        modules: Vec<LoadedModule>,
        ranges: Vec<ModuleRange>,
    },
}

/// A compiled, checked program ready for [`Engine::create_context`].
/// Immutable and `Send + Sync`: share it across threads and contexts
/// freely.
#[derive(Debug, Clone)]
pub struct CompiledProgram {
    statements: Vec<Stmt>,
    kind: ProgramKind,
    manifest: Option<ModManifest>,
    /// The §9.3 boundary types of the ACTIVE schema, synthesized as AST
    /// declarations so any execution engine (tree walker now, bytecode VM
    /// and AOT later) can construct schema structs and enums.
    schema_declarations: Vec<Stmt>,
}

impl CompiledProgram {
    /// The names of every top-level function, in declaration order — the
    /// entry points the host may [`Context::invoke`] (§2.1: there is no
    /// implicit entry point; hosts pick).
    pub fn entry_points(&self) -> Vec<String> {
        self.statements
            .iter()
            .filter_map(|stmt| match &stmt.kind {
                cme_core::ast::StmtKind::FuncDecl { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// The §10.4 impl target paths (`engine.gamemode`, `vec2`, …) the mod
    /// implements, in first-declaration order — the targets a host may
    /// [`Context::invoke_member`] into.
    pub fn interface_targets(&self) -> Vec<String> {
        let mut targets: Vec<String> = Vec::new();
        for stmt in &self.statements {
            if let cme_core::ast::StmtKind::ImplDecl { target, .. } = &stmt.kind {
                let joined = target.join(".");
                if !targets.contains(&joined) {
                    targets.push(joined);
                }
            }
        }
        targets
    }

    /// The §10.2 manifest, for mod builds that declared one.
    pub fn manifest(&self) -> Option<&ModManifest> {
        self.manifest.as_ref()
    }

    /// Whether this program came from a §10 mod tree.
    pub fn is_mod(&self) -> bool {
        matches!(self.kind, ProgramKind::Mod { .. })
    }

    /// The number of top-level statements after linking.
    pub fn statement_count(&self) -> usize {
        self.statements.len()
    }

    /// The program text spans refer to: the (expanded) source for loose
    /// sources, the assembled virtual text for mods. Execution errors from
    /// mod builds re-anchor to modules instead — see [`ProgramKind::Mod`].
    pub fn source(&self) -> &str {
        match &self.kind {
            ProgramKind::Source { source, .. } => source,
            ProgramKind::Mod { assembled, .. } => assembled,
        }
    }

    /// The linked modules of a mod build, with their display paths and
    /// original sources; `None` for loose sources.
    pub fn modules(&self) -> Option<&[LoadedModule]> {
        match &self.kind {
            ProgramKind::Mod { modules, .. } => Some(modules),
            ProgramKind::Source { .. } => None,
        }
    }

    /// The statements, for hosts that walk the AST directly (tooling,
    /// tests). Mutation is impossible by construction.
    pub fn statements(&self) -> &[Stmt] {
        &self.statements
    }

    /// The synthesized schema boundary-type declarations (§9.3): empty
    /// when no schema was active at load.
    pub fn schema_declarations(&self) -> &[Stmt] {
        &self.schema_declarations
    }

    /// The program kind and its associated text data.
    pub fn kind(&self) -> &ProgramKind {
        &self.kind
    }
}

/// A compile failure: every diagnostic the pipeline produced, both
/// structured (for tooling) and rendered (`path:line:column: message`, the
/// same shape the CLI reports).
#[derive(Debug, Clone)]
pub struct CompileError {
    diagnostics: Vec<Diagnostic>,
    rendered: Vec<String>,
}

impl CompileError {
    /// The structured diagnostics, with kinds and spans.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// One rendered line per diagnostic.
    pub fn messages(&self) -> &[String] {
        &self.rendered
    }

    /// Every rendered diagnostic joined by newlines — the Display body.
    pub fn message(&self) -> String {
        self.rendered.join("\n")
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for CompileError {}

/// A [`Engine::load_file`] failure: the file could not be read, or it read
/// but did not compile.
#[derive(Debug, Clone)]
pub enum LoadError {
    Io(String),
    Compile(CompileError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(message) => formatter.write_str(message),
            LoadError::Compile(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<CompileError> for LoadError {
    fn from(error: CompileError) -> Self {
        LoadError::Compile(error)
    }
}

// ---------------------------------------------------------------------------
// Build pipelines — the exact gates the CLI applies, shared by every entry.
// ---------------------------------------------------------------------------

/// The single-file pipeline (§8 expansion, parse, §10 standalone import
/// check, type check — against the active schema when one is registered).
/// `name` — when the source came from a file — becomes the `path:` prefix
/// of rendered diagnostics.
fn build_source(
    engine: &Engine,
    source: &str,
    name: Option<&str>,
) -> Result<CompiledProgram, CompileError> {
    let expanded =
        maybe_expand(source).map_err(|diagnostics| render_source(&diagnostics, source, name))?;
    let outcome = parse_source(&expanded);
    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(mods::standalone_import_diagnostics(&outcome.statements));
    let (check_diagnostics, schema_declarations) = check_program(engine, &outcome.statements, None);
    diagnostics.extend(check_diagnostics);
    if !diagnostics.is_empty() {
        return Err(render_source(&diagnostics, &expanded, name));
    }
    Ok(CompiledProgram {
        statements: outcome.statements,
        kind: ProgramKind::Source {
            source: expanded,
            name: name.map(str::to_string),
        },
        manifest: None,
        schema_declarations,
    })
}

/// The check gate, shared by both pipelines: the schema-aware checker when
/// a schema is registered (§9), the plain checker otherwise — followed by
/// the load-time capability-provider presence check (§9.1/§13.1: a call a
/// script can never dispatch to is a load failure, not a runtime
/// surprise).
/// Returns the check diagnostics plus the synthesized schema type
/// declarations for the load (empty without a schema).
fn check_program(
    engine: &Engine,
    statements: &[Stmt],
    targets: Option<Vec<(String, String)>>,
) -> (Vec<Diagnostic>, Vec<Stmt>) {
    let Some(schema_files) = (!engine.schemas.is_empty()).then(|| engine.schemas.clone()) else {
        return (cme_compiler::check::check(statements), Vec::new());
    };
    let set = match SchemaSet::build(schema_files) {
        Ok(set) => set,
        Err(issues) => {
            // register_schema validates eagerly, so this is unreachable in
            // practice; fail the load rather than panic.
            return (
                issues
                    .iter()
                    .map(|issue| {
                        Diagnostic::parse(
                            format!("schema contract error: {}", issue.message),
                            cme_core::Span::new(0, 0),
                        )
                    })
                    .collect(),
                Vec::new(),
            );
        }
    };
    let context = match targets {
        Some(targets) => match SchemaContext::grant_targets(set, targets) {
            Ok(context) => context,
            Err(issues) => {
                return (
                    issues
                        .iter()
                        .map(|issue| {
                            Diagnostic::parse(
                                format!("schema contract error: {}", issue.message),
                                cme_core::Span::new(0, 0),
                            )
                        })
                        .collect(),
                    Vec::new(),
                );
            }
        },
        None => SchemaContext::grant_all(set),
    };
    let schema_declarations = schema_declaration_statements(&context);
    let mut diagnostics = check_with_schema(statements, Some(&context));

    // Provider presence: every capability the program CALLS must have a
    // registered provider (§9.1 capabilities are host-provided). Imports
    // alone do not demand one — a program may compile against a capability
    // it never calls.
    for path in called_capability_paths(statements, &context) {
        if !engine.providers.contains_key(&path) {
            diagnostics.push(Diagnostic::parse(
                format!(
                    "capability `{path}` is called by this program, but the host has not \
                     registered a provider for it (§9.1, §13.1)"
                ),
                cme_core::Span::new(0, 0),
            ));
        }
    }
    (diagnostics, schema_declarations)
}

/// Synthesizes the §9.3 boundary-type declarations of every GRANTED
/// namespace into AST statements: scripts construct schema structs and
/// enums exactly like local ones, and every execution engine consumes the
/// same AST shape (the AST is the contract).
fn schema_declaration_statements(context: &SchemaContext) -> Vec<Stmt> {
    use cme_core::Span as S;
    let mut declarations = Vec::new();
    for file in context.set.namespaces() {
        if context.target(&file.namespace).is_none() {
            continue;
        }
        for item in &file.items {
            match item {
                cme_core::schema::SchemaItem::Struct(decl) => declarations.push(Stmt {
                    span: S::new(0, 0),
                    kind: StmtKind::StructDecl {
                        name: decl.name.clone(),
                        type_params: Vec::new(),
                        fields: decl.fields.clone(),
                    },
                }),
                cme_core::schema::SchemaItem::Enum(decl) => declarations.push(Stmt {
                    span: S::new(0, 0),
                    kind: StmtKind::EnumDecl {
                        name: decl.name.clone(),
                        type_params: Vec::new(),
                        variants: decl.variants.clone(),
                    },
                }),
                cme_core::schema::SchemaItem::Contract(_) => {}
            }
        }
    }
    declarations
}

/// Every capability path (`engine.graphics`) the program calls through a
/// `namespace.capability.Member` path call, in first-call order.
fn called_capability_paths(statements: &[Stmt], context: &SchemaContext) -> Vec<String> {
    let mut paths = Vec::new();
    for statement in statements {
        walk_statement_for_calls(statement, context, &mut paths);
    }
    paths
}

fn walk_statement_for_calls(statement: &Stmt, context: &SchemaContext, paths: &mut Vec<String>) {
    match &statement.kind {
        StmtKind::FuncDecl { body, .. } => walk_block_for_calls(body, context, paths),
        StmtKind::ImplDecl { members, .. } => {
            for member in members {
                walk_statement_for_calls(member, context, paths);
            }
        }
        StmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_expr_for_calls(cond, context, paths);
            walk_block_for_calls(then_branch, context, paths);
            if let Some(else_stmt) = else_branch {
                walk_statement_for_calls(else_stmt, context, paths);
            }
        }
        StmtKind::While { cond, body } => {
            walk_expr_for_calls(cond, context, paths);
            walk_block_for_calls(body, context, paths);
        }
        StmtKind::For { iterable, body, .. } => {
            walk_expr_for_calls(iterable, context, paths);
            walk_block_for_calls(body, context, paths);
        }
        StmtKind::Match { scrutinee, arms } => {
            walk_expr_for_calls(scrutinee, context, paths);
            for arm in arms {
                walk_block_for_calls(&arm.body, context, paths);
            }
        }
        StmtKind::Block(block) => walk_block_for_calls(block, context, paths),
        StmtKind::Expression { expr } => walk_expr_for_calls(expr, context, paths),
        StmtKind::VarDecl { expr, .. } => walk_expr_for_calls(expr, context, paths),
        StmtKind::Assign { target: _, expr } => walk_expr_for_calls(expr, context, paths),
        StmtKind::CompoundAssign {
            target: _, expr, ..
        } => walk_expr_for_calls(expr, context, paths),
        StmtKind::Return { value: Some(expr) } => walk_expr_for_calls(expr, context, paths),
        _ => {}
    }
}

fn walk_block_for_calls(
    block: &cme_core::ast::Block,
    context: &SchemaContext,
    paths: &mut Vec<String>,
) {
    for statement in &block.stmts {
        walk_statement_for_calls(statement, context, paths);
    }
}

fn walk_expr_for_calls(
    expr: &cme_core::ast::Expr,
    context: &SchemaContext,
    paths: &mut Vec<String>,
) {
    use cme_core::ast::ExprKind;
    if let ExprKind::PathCall { path, .. } = &expr.kind
        && path.len() == 3
        && context.target(&path[0]).is_some()
        && let Some(file) = context.set.namespace(&path[0])
        && file.capability(&path[1]).is_some()
    {
        let qualified = format!("{}.{}", path[0], path[1]);
        if !paths.contains(&qualified) {
            paths.push(qualified);
        }
    }
    match &expr.kind {
        ExprKind::Binary { lhs, rhs, .. } => {
            walk_expr_for_calls(lhs, context, paths);
            walk_expr_for_calls(rhs, context, paths);
        }
        ExprKind::Unary { expr: inner, .. }
        | ExprKind::Paren { expr: inner }
        | ExprKind::Try { expr: inner } => {
            walk_expr_for_calls(inner, context, paths);
        }
        ExprKind::Call { args, .. } | ExprKind::VariantCall { args, .. } => {
            for arg in args {
                match arg {
                    cme_core::ast::CallArg::Positional(inner)
                    | cme_core::ast::CallArg::Named { expr: inner, .. } => {
                        walk_expr_for_calls(inner, context, paths);
                    }
                }
            }
        }
        ExprKind::PathCall { args, .. } => {
            for arg in args {
                match arg {
                    cme_core::ast::CallArg::Positional(inner)
                    | cme_core::ast::CallArg::Named { expr: inner, .. } => {
                        walk_expr_for_calls(inner, context, paths);
                    }
                }
            }
        }
        ExprKind::Field { obj, .. } | ExprKind::Index { obj, .. } => {
            walk_expr_for_calls(obj, context, paths);
            if let ExprKind::Index { index, .. } = &expr.kind {
                walk_expr_for_calls(index, context, paths);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            walk_expr_for_calls(scrutinee, context, paths);
            for arm in arms {
                walk_expr_for_calls(&arm.body, context, paths);
            }
        }
        ExprKind::ArrayLit { elements } => {
            for element in elements {
                walk_expr_for_calls(element, context, paths);
            }
        }
        ExprKind::MapLit { entries } => {
            for (key, value) in entries {
                walk_expr_for_calls(key, context, paths);
                walk_expr_for_calls(value, context, paths);
            }
        }
        ExprKind::Interpolated { parts } => {
            for part in parts {
                if let cme_core::ast::InterpPart::Expr(inner) = part {
                    walk_expr_for_calls(inner, context, paths);
                }
            }
        }
        _ => {}
    }
}

/// Expands megaprograms when present; a plain source passes through
/// unchanged (§8: the expansion of a megaprogram-free file is the file).
fn maybe_expand(source: &str) -> Result<String, Vec<Diagnostic>> {
    if !mega::expand::mentions_megaprogram(source) {
        return Ok(source.to_string());
    }
    Ok(mega::expand::expand_source(source)?.expanded)
}

/// Renders diagnostics against one source text.
fn render_source(diagnostics: &[Diagnostic], source: &str, name: Option<&str>) -> CompileError {
    let layout = SourceLayout::new(source);
    CompileError {
        rendered: diagnostics
            .iter()
            .map(|diagnostic| {
                render_located(
                    diagnostic.message(),
                    diagnostic.span(),
                    source,
                    name,
                    &layout,
                )
            })
            .collect(),
        diagnostics: diagnostics.to_vec(),
    }
}

/// The §10 mod pipeline: load, per-module expansion, assembly, check —
/// mirroring the CLI's `mod_command`, with every failure re-anchored to its
/// owning module so hosts never see virtual-text coordinates. The mod's
/// `[schemas]` table narrows the active schema grant (§9.5, §10.2).
fn build_mod(engine: &Engine, root: &Path) -> Result<CompiledProgram, CompileError> {
    // A path naming the manifest itself selects its parent directory —
    // the CLI's mod-root resolution (§10.2).
    let root = if root.is_file() && root.file_name().is_some_and(|name| name == "mod.toml") {
        root.parent().unwrap_or_else(|| Path::new("."))
    } else {
        root
    };
    let display_root = root.display().to_string();
    let loaded = mods::load_mod(root);
    if !loaded.issues.is_empty() {
        // Manifest and structure defects carry their own file/line (often
        // relative to the mod root); render them like the CLI does.
        let rendered = loaded
            .issues
            .iter()
            .map(|issue| {
                let location = match (&issue.file, issue.line) {
                    (Some(file), Some(line)) => format!("{display_root}/{file}:{line}: "),
                    (Some(file), None) => format!("{display_root}/{file}: "),
                    (None, _) => String::new(),
                };
                format!("{location}{}", issue.message)
            })
            .collect();
        return Err(CompileError {
            diagnostics: Vec::new(),
            rendered,
        });
    }

    let mut modules = loaded.modules;
    for index in 0..modules.len() {
        if !mega::expand::mentions_megaprogram(&modules[index].source) {
            continue;
        }
        let source = modules[index].source.clone();
        let outcome = mega::expand::expand_source(&source).map_err(|errors| {
            render_module_diagnostics(&errors, index, &modules, &source, &display_root)
        })?;
        modules[index].source = outcome.expanded;
    }

    let program = mods::assemble(&modules);
    let mut failures = render_assembled_diagnostics(&program, &modules, &display_root);
    // The manifest's `[schemas]` table is the grant: namespace → target
    // version (§9.5). Without a manifest, no schema is active for mods.
    let targets = loaded
        .manifest
        .as_ref()
        .map(|manifest| manifest.schemas.clone());
    let (check_errors, schema_declarations) = check_program(engine, &program.statements, targets);
    if !check_errors.is_empty() {
        failures.extend(render_located_diagnostics(
            &check_errors,
            &program.ranges,
            &modules,
            &display_root,
        ));
    }
    if !failures.is_empty() {
        return Err(CompileError {
            diagnostics: Vec::new(),
            rendered: failures,
        });
    }

    Ok(CompiledProgram {
        statements: program.statements,
        kind: ProgramKind::Mod {
            root: display_root,
            assembled: program.source,
            modules,
            ranges: program.ranges,
        },
        manifest: loaded.manifest,
        schema_declarations,
    })
}

/// Renders module-expansion diagnostics against the module's ORIGINAL text.
pub(crate) fn render_module_diagnostics(
    diagnostics: &[Diagnostic],
    module_index: usize,
    modules: &[LoadedModule],
    original_source: &str,
    display_root: &str,
) -> CompileError {
    let module = &modules[module_index];
    let path = format!("{display_root}/{}", module.display_path);
    let layout = SourceLayout::new(original_source);
    CompileError {
        diagnostics: diagnostics.to_vec(),
        rendered: diagnostics
            .iter()
            .map(|diagnostic| {
                render_located(
                    diagnostic.message(),
                    diagnostic.span(),
                    original_source,
                    Some(&path),
                    &layout,
                )
            })
            .collect(),
    }
}

/// Renders assembled-program diagnostics (parse, import resolution, and
/// checker output), each re-anchored from virtual-text coordinates to its
/// owning module's own text.
pub(crate) fn render_located_diagnostics(
    diagnostics: &[Diagnostic],
    ranges: &[ModuleRange],
    modules: &[LoadedModule],
    display_root: &str,
) -> Vec<String> {
    let layouts: Vec<SourceLayout> = modules
        .iter()
        .map(|module| SourceLayout::new(&module.source))
        .collect();
    diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let (owner, local) = mods::attribute_span(ranges, diagnostic.span())?;
            let index = ranges.iter().position(|range| std::ptr::eq(range, owner))?;
            let path = format!("{display_root}/{}", owner.display_path);
            Some(render_located(
                diagnostic.message(),
                local,
                &modules[index].source,
                Some(&path),
                &layouts[index],
            ))
        })
        .collect()
}

/// `render_located_diagnostics` specialized to an assembled program.
fn render_assembled_diagnostics(
    program: &mods::AssembledProgram,
    modules: &[LoadedModule],
    display_root: &str,
) -> Vec<String> {
    render_located_diagnostics(&program.diagnostics, &program.ranges, modules, display_root)
}

/// Renders a runtime error against the module that was executing when it
/// happened. Shared with [`crate::context`] for execution errors. Returns
/// `(file, line, column, re-anchored span)`.
pub(crate) fn render_runtime_error(
    span: cme_core::Span,
    program: &CompiledProgram,
) -> (Option<String>, usize, usize, Option<cme_core::Span>) {
    match &program.kind {
        ProgramKind::Source { source, .. } => {
            let layout = SourceLayout::new(source);
            let (line, column) = layout.line_column(source, span.start);
            (None, line, column, Some(span))
        }
        ProgramKind::Mod {
            root,
            modules,
            ranges,
            ..
        } => match mods::attribute_span(ranges, span) {
            Some((owner, local)) => {
                let index = ranges
                    .iter()
                    .position(|range| std::ptr::eq(range, owner))
                    .unwrap_or(0);
                let layout = SourceLayout::new(&modules[index].source);
                let (line, column) = layout.line_column(&modules[index].source, local.start);
                (
                    Some(format!("{root}/{}", owner.display_path)),
                    line,
                    column,
                    Some(local),
                )
            }
            None => (None, 0, 0, None),
        },
    }
}
