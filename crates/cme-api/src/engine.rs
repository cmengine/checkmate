//! The [`Engine`]: loads Checkmate programs and hands out execution
//! contexts (WHITEPAPER §13.1).

use std::fmt;
use std::path::Path;

use cme_compiler::check::check as check_statements;
use cme_compiler::mods::{self, LoadedModule, ModManifest, ModuleRange};
use cme_compiler::{Diagnostic, mega, parse_source};
use cme_core::ast::Stmt;

use crate::context::{Context, ExecutionLimits};
use crate::render::{SourceLayout, render_located};

/// The host-facing entry to Checkmate: load programs, create contexts
/// (WHITEPAPER §13.1). The engine itself holds no state — loading is pure —
/// but hosts keep one anyway so future capability registration has a home.
///
/// ```
/// let engine = cme_api::Engine::new();
/// let program = engine.load_source("int main() {\nreturn 1\n}\n")?;
/// assert_eq!(program.entry_points(), ["main"]);
/// # Ok::<(), cme_api::CompileError>(())
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Engine;

impl Engine {
    /// A fresh engine handle.
    pub fn new() -> Engine {
        Engine
    }

    /// Compiles a single source text into a runnable program. The exact
    /// gate the CLI's `run` applies: megaprograms expand first when the
    /// source mentions the subsystem (§8), then the front end parses, the
    /// §10 standalone-import check rejects `import self.*` outside a mod,
    /// and the type checker runs. Any diagnostic — from any stage — fails
    /// the load; a program that produced one never becomes invocable.
    pub fn load_source(&self, source: &str) -> Result<CompiledProgram, CompileError> {
        build_source(source, None)
    }

    /// Reads and compiles a `.cm` file. Identical to [`Engine::load_source`]
    /// except I/O failures surface as [`LoadError::Io`] and diagnostics
    /// render with `path:line:column` locations naming the file.
    pub fn load_file(&self, path: impl AsRef<Path>) -> Result<CompiledProgram, LoadError> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path)
            .map_err(|error| LoadError::Io(format!("cannot read {}: {error}", path.display())))?;
        build_source(&source, Some(&path.display().to_string())).map_err(LoadError::Compile)
    }

    /// Compiles a whole §10 mod: a directory holding `mod.toml` and a
    /// `src/` tree (the path may also name the `mod.toml` itself, as the
    /// CLI accepts). Modules expand individually, then link into one
    /// program; every compile diagnostic re-anchors to its owning module.
    pub fn load_mod(&self, root: impl AsRef<Path>) -> Result<CompiledProgram, CompileError> {
        build_mod(root.as_ref())
    }

    /// Creates an execution context over `program` under §5.5 limits.
    /// The context borrows the program: destroy the context first, or —
    /// since borrows enforce it — never outlive the program with one.
    /// Programs are immutable after loading, so any number of contexts
    /// (and invocations) may share one concurrently.
    pub fn create_context<'p>(
        &self,
        program: &'p CompiledProgram,
        limits: ExecutionLimits,
    ) -> Context<'p> {
        Context::new(program, limits)
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
/// check, type check). `name` — when the source came from a file — becomes
/// the `path:` prefix of rendered diagnostics.
fn build_source(source: &str, name: Option<&str>) -> Result<CompiledProgram, CompileError> {
    let expanded =
        maybe_expand(source).map_err(|diagnostics| render_source(&diagnostics, source, name))?;
    let outcome = parse_source(&expanded);
    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(mods::standalone_import_diagnostics(&outcome.statements));
    diagnostics.extend(check_statements(&outcome.statements));
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
    })
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
/// owning module so hosts never see virtual-text coordinates.
fn build_mod(root: &Path) -> Result<CompiledProgram, CompileError> {
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
    let check_errors = check_statements(&program.statements);
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
