#[cfg(feature = "cli")]
use cme_compiler::diagnostics::Diagnostic;
#[cfg(feature = "cli")]
use cme_core::Span;
#[cfg(feature = "cli")]
use cme_core::ast::StmtKind;
#[cfg(feature = "cli")]
use cme_interp::{InterpError, Interpreter, Value};
#[cfg(feature = "cli")]
use std::path::{Path, PathBuf};
#[cfg(feature = "cli")]
use std::process::ExitCode;
#[cfg(feature = "cli")]
const USAGE: &str = "Usage: cme <lex|ast|check|run|expand> <file.cm> [--provenance] [--schema <schema.cm>]\
     \n       cme <check|ast|run> <mod_dir | path/to/mod.toml> [--schema <schema.cm>]\
     \n       cme schema <schema.cm>\
     \n       cme codegen-c <schema.cm>\
     \n       cme lsp\
     \n  (--provenance is an `expand` option: it annotates each root magic site \
       with `// @ magic(name) src:line:col`)\
     \n  (--schema registers a §9 schema contract; repeatable; a mod's [schemas] \
       table narrows the grant — §9.5)\
     \n  a mod directory holds a mod.toml and a src/ tree (WHITEPAPER §10); \
       lex and expand stay single-file commands\
     \n  `cme lsp` serves the Checkmate language server (§14) over stdio";

#[cfg(feature = "cli")]
enum CliError {
    Usage(String),
    Io(String),
    Compiler(Vec<Diagnostic>, String),
    Runtime(InterpError, String),
    /// Mod-build failures, carried as fully rendered `file:line:col`
    /// messages (each diagnostic is re-anchored to its own module before
    /// reporting, so no raw virtual-text spans leak out).
    Mod(Vec<String>),
    /// Schema contract failures: rendered defect lines from the §9
    /// front end or the set invariants.
    Schema(Vec<String>),
}

#[cfg(feature = "cli")]
fn run() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `--schema <path>` may appear anywhere (repeatable); the rest is the
    // command, its file, and `expand`'s optional flag.
    let mut schema_paths: Vec<String> = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--schema" {
            let Some(value) = iter.next() else {
                return Err(CliError::Usage(format!(
                    "--schema requires a .cm schema file path\n{USAGE}"
                )));
            };
            schema_paths.push(value);
        } else {
            positional.push(arg);
        }
    }
    // `cme lsp` runs the language server over stdio; it takes no file and
    // no schema arguments.
    if positional.as_slice() == ["lsp"] {
        return lsp_command();
    }
    // `expand` accepts an optional `--provenance` flag; other commands take
    // exactly one file argument.
    let (command, path, provenance) = match positional.as_slice() {
        [command, path] => (command.as_str(), path.as_str(), false),
        [command, path, flag] if command == "expand" && flag == "--provenance" => {
            (command.as_str(), path.as_str(), true)
        }
        _ => {
            return Err(CliError::Usage(format!(
                "expected a command, a file, and (for expand) an optional --provenance\n{USAGE}"
            )));
        }
    };

    // The active §9 contract, when the caller registered schema files.
    let schema = load_schema_context(&schema_paths, None)?;
    let schema = schema.as_ref();

    // A directory (or a path ending in mod.toml) selects mod mode: the
    // unit of compilation is the whole §10 mod tree.
    if let Some(mod_root) = resolve_mod_root(path) {
        return mod_command(command, &mod_root.root, &mod_root.display, schema);
    }

    let source = std::fs::read_to_string(path)
        .map_err(|error| CliError::Io(format!("failed to read {path}: {error}")))?;

    // Files that mention megaprogram constructs are expanded first; every
    // later stage works on the expanded (pure Checkmate) text. Expansion
    // diagnostics render against the ORIGINAL file (plan §2).
    let source = match command {
        "lex" | "expand" => source,
        _ => maybe_expand(&source)?,
    };

    match command {
        "lex" => {
            let (tokens, errors) = cme_compiler::lexer::lex_with_errors(&source);
            let errors = errors.into_iter().map(Diagnostic::lex).collect::<Vec<_>>();
            for token in tokens {
                println!("{token:?}");
            }
            render_diagnostics(errors, &source)
        }
        "ast" => {
            let outcome = cme_compiler::parse_source(&source);
            let mut errors = outcome.diagnostics;
            errors.extend(cme_compiler::mods::standalone_import_diagnostics(
                &outcome.statements,
            ));
            let ast = outcome.statements;

            println!("{ast:#?}");
            render_diagnostics(errors, &source)
        }
        "check" => {
            let outcome = cme_compiler::parse_source(&source);
            let mut errors = outcome.diagnostics;
            errors.extend(cme_compiler::mods::standalone_import_diagnostics(
                &outcome.statements,
            ));
            errors.extend(match schema {
                Some(context) => {
                    cme_compiler::check::check_with_schema(&outcome.statements, Some(context))
                }
                None => cme_compiler::check::check(&outcome.statements),
            });
            render_diagnostics(errors, &source)
        }
        "run" => run_program(&source, path, schema),
        "expand" => expand_command(&source, path, provenance),
        "schema" => schema_command(path),
        "codegen-c" => codegen_c_command(path),
        _ => Err(CliError::Usage(format!(
            "unknown command: {command}\n{USAGE}"
        ))),
    }
}

/// `cme lsp`: the language server (WHITEPAPER §12, §14). Serves JSON-RPC
/// over stdio until the client disconnects; the analysis is salsa-backed
/// (see `cme-lsp`).
#[cfg(feature = "cli")]
fn lsp_command() -> Result<(), CliError> {
    #[cfg(feature = "lsp")]
    return cme_lsp::run_stdio().map_err(CliError::Io);

    #[cfg(not(feature = "lsp"))]
    Err(CliError::Usage(
        "the 'lsp' command requires the 'lsp' feature on the root package\
         \nTry compiling with: cargo build --features cli"
            .to_string(),
    ))
}

/// The resolved root of a mod: where to load from and how to prefix module
/// paths in diagnostics (`my_mod/src/main.cm:3:1`).
#[cfg(feature = "cli")]
struct ModRoot {
    root: PathBuf,
    display: String,
}

/// A path argument selects mod mode when it is a directory (a mod tree) or
/// a file literally named `mod.toml` (the manifest itself). Everything
/// else keeps the single-file workflow.
#[cfg(feature = "cli")]
fn resolve_mod_root(path: &str) -> Option<ModRoot> {
    let candidate = Path::new(path);
    if candidate.is_dir() {
        let display = path.trim_end_matches('/');
        return Some(ModRoot {
            root: candidate.to_path_buf(),
            display: if display.is_empty() {
                ".".to_string()
            } else {
                display.to_string()
            },
        });
    }
    if candidate.is_file() && candidate.file_name().is_some_and(|name| name == "mod.toml") {
        let parent = candidate.parent().unwrap_or(Path::new(""));
        let (root, display) = if parent.as_os_str().is_empty() {
            (PathBuf::from("."), ".".to_string())
        } else {
            (parent.to_path_buf(), parent.display().to_string())
        };
        return Some(ModRoot { root, display });
    }
    None
}

/// The mod pipeline for `check|ast|run` (§10): load the mod, expand each
/// module's megaprograms, assemble the virtual program, check it, and —
/// for `run` — invoke `main`. Every diagnostic is re-anchored to the
/// module it came from before reporting.
#[cfg(feature = "cli")]
fn mod_command(
    command: &str,
    mod_root: &Path,
    display_root: &str,
    schema: Option<&cme_compiler::schema::SchemaContext>,
) -> Result<(), CliError> {
    if matches!(command, "lex" | "expand") {
        return Err(CliError::Usage(format!(
            "`{command}` works on a single .cm file; a mod directory accepts \
             check, ast, and run\n{USAGE}"
        )));
    }

    let loaded = cme_compiler::mods::load_mod(mod_root);
    if !loaded.issues.is_empty() {
        return Err(CliError::Mod(
            loaded
                .issues
                .iter()
                .map(|issue| render_mod_issue(issue, display_root))
                .collect(),
        ));
    }

    let mut modules = loaded.modules;
    // Megaprogram expansion stays per file (magics are module-scope
    // declarations resolved before linking); expansion diagnostics render
    // against the module's ORIGINAL text, like single-file mode.
    for index in 0..modules.len() {
        if !cme_compiler::mega::expand::mentions_megaprogram(&modules[index].source) {
            continue;
        }
        let source = modules[index].source.clone();
        let outcome = cme_compiler::mega::expand::expand_source(&source).map_err(|errors| {
            CliError::Mod(render_module_diagnostics(
                &errors,
                index,
                &modules,
                &source,
                display_root,
            ))
        })?;
        modules[index].source = outcome.expanded;
    }

    let program = cme_compiler::mods::assemble(&modules);
    let mut failures = render_assembled_diagnostics(&program, &modules, display_root);
    // With schemas registered, the mod's [schemas] manifest table narrows
    // the grant (§9.5, §10.2); a manifest listing nothing denies
    // everything — the §7.2 sandbox.
    let check_errors = match schema {
        Some(context) => {
            // A manifest listing nothing denies every namespace — the
            // §7.2 sandbox: only granted namespaces are visible.
            let targets = loaded
                .manifest
                .as_ref()
                .map(|m| m.schemas.clone())
                .unwrap_or_default();
            let grant =
                cme_compiler::schema::SchemaContext::grant_targets(context.set.clone(), targets);
            match grant {
                Ok(grant) => {
                    cme_compiler::check::check_with_schema(&program.statements, Some(&grant))
                }
                Err(issues) => {
                    return Err(CliError::Schema(
                        issues.iter().map(|issue| issue.message.clone()).collect(),
                    ));
                }
            }
        }
        None => cme_compiler::check::check(&program.statements),
    };
    if !check_errors.is_empty() {
        // Checker diagnostics live in virtual-text coordinates too: run
        // them through the same re-anchoring.
        failures.extend(render_located_diagnostics(
            &check_errors,
            &program.ranges,
            &modules,
            display_root,
        ));
    }
    if !failures.is_empty() {
        return Err(CliError::Mod(failures));
    }

    match command {
        // The compile gate above is the whole `check` command: a clean mod
        // prints nothing and exits successfully.
        "check" => Ok(()),
        "ast" => {
            println!("{:#?}", program.statements);
            Ok(())
        }
        "run" => {
            // §2.1: the host picks the entry point; `main` stays the CLI
            // convention, and any module may provide it.
            let has_main = program.statements.iter().any(
                |stmt| matches!(&stmt.kind, StmtKind::FuncDecl { name, .. } if name == "main"),
            );
            if !has_main {
                return Err(CliError::Usage(format!(
                    "no `main` function to run in mod {display_root}"
                )));
            }
            let interpreter = Interpreter::new(&program.statements);
            match interpreter.invoke("main", &[]) {
                Ok(value) => {
                    if !matches!(value, Value::Void) {
                        println!("{value}");
                    }
                    Ok(())
                }
                Err(error) => Err(CliError::Mod(vec![render_runtime_error(
                    &error,
                    &program,
                    &modules,
                    display_root,
                )])),
            }
        }
        _ => unreachable!("lex/expand are rejected above"),
    }
}

/// Renders a mod-level issue (manifest or structure defect).
#[cfg(feature = "cli")]
fn render_mod_issue(issue: &cme_compiler::mods::ModIssue, display_root: &str) -> String {
    let location = match (&issue.file, issue.line) {
        (Some(file), Some(line)) => format!("{display_root}/{file}:{line}: "),
        (Some(file), None) => format!("{display_root}/{file}: "),
        (None, _) => String::new(),
    };
    format!("{location}{}", issue.message)
}

/// Renders expansion diagnostics against their module's original text.
#[cfg(feature = "cli")]
fn render_module_diagnostics(
    diagnostics: &[Diagnostic],
    module_index: usize,
    modules: &[cme_compiler::mods::LoadedModule],
    original_source: &str,
    display_root: &str,
) -> Vec<String> {
    let module = &modules[module_index];
    let path = format!("{display_root}/{}", module.display_path);
    let layout = SourceLayout::new(original_source);
    diagnostics
        .iter()
        .map(|diagnostic| {
            render_message_indexed(
                diagnostic.message(),
                diagnostic.span(),
                original_source,
                &path,
                &layout,
            )
        })
        .collect()
}

/// Renders assembled-program diagnostics (parse, import resolution, and
/// checker output), each re-anchored from virtual-text coordinates to its
/// owning module's own text.
#[cfg(feature = "cli")]
fn render_located_diagnostics(
    diagnostics: &[Diagnostic],
    ranges: &[cme_compiler::mods::ModuleRange],
    modules: &[cme_compiler::mods::LoadedModule],
    display_root: &str,
) -> Vec<String> {
    let layouts: Vec<SourceLayout> = modules
        .iter()
        .map(|module| SourceLayout::new(&module.source))
        .collect();
    diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let (owner, local) = cme_compiler::mods::attribute_span(ranges, diagnostic.span())?;
            let index = ranges.iter().position(|range| std::ptr::eq(range, owner))?;
            let path = format!("{display_root}/{}", owner.display_path);
            Some(render_message_indexed(
                diagnostic.message(),
                local,
                &modules[index].source,
                &path,
                &layouts[index],
            ))
        })
        .collect()
}

/// `render_located_diagnostics` specialized to an assembled program.
#[cfg(feature = "cli")]
fn render_assembled_diagnostics(
    program: &cme_compiler::mods::AssembledProgram,
    modules: &[cme_compiler::mods::LoadedModule],
    display_root: &str,
) -> Vec<String> {
    render_located_diagnostics(&program.diagnostics, &program.ranges, modules, display_root)
}

/// Renders a runtime error against the module that was executing when it
/// happened.
#[cfg(feature = "cli")]
fn render_runtime_error(
    error: &InterpError,
    program: &cme_compiler::mods::AssembledProgram,
    modules: &[cme_compiler::mods::LoadedModule],
    display_root: &str,
) -> String {
    match cme_compiler::mods::attribute_span(&program.ranges, error.span) {
        Some((owner, local)) => {
            let index = program
                .ranges
                .iter()
                .position(|range| std::ptr::eq(range, owner))
                .unwrap_or(0);
            let path = format!("{display_root}/{}", owner.display_path);
            let layout = SourceLayout::new(&modules[index].source);
            render_message_indexed(
                &error.message,
                local,
                &modules[index].source,
                &path,
                &layout,
            )
        }
        None => error.message.clone(),
    }
}

/// Expands megaprograms when present. Used by `check|run|ast` so magic
/// sources behave like their expansions.
#[cfg(feature = "cli")]
fn maybe_expand(source: &str) -> Result<String, CliError> {
    if !cme_compiler::mega::expand::mentions_megaprogram(source) {
        return Ok(source.to_string());
    }
    let outcome = cme_compiler::mega::expand::expand_source(source)
        .map_err(|errors| CliError::Compiler(errors, source.to_string()))?;
    Ok(outcome.expanded)
}

/// `cme expand <file.cm>`: expands megaprograms in the places they were
/// called and writes the pure-Checkmate result to a labeled file side by
/// side with the original (`magic.cm` → `magic_expanded.cm`), then parses
/// and checks that file, reporting against it. With `--provenance`, each
/// root invocation site is annotated with a `// @ magic(name) src:L:C`
/// comment; without it, the output stays byte-deterministic.
#[cfg(feature = "cli")]
fn expand_command(source: &str, path: &str, provenance: bool) -> Result<(), CliError> {
    let outcome = cme_compiler::mega::expand::expand_source_with(
        source,
        cme_compiler::mega::expand::ExpandOptions { provenance },
    )
    .map_err(|errors| CliError::Compiler(errors, source.to_string()))?;

    let expanded_path = expanded_path_for(path);
    let mut text = String::new();
    text.push_str(&format!(
        "// Generated by `cme expand {path}` — megaprogram invocations \
         expanded in place; this file is plain Checkmate.\n"
    ));
    text.push_str(&outcome.expanded);
    std::fs::write(&expanded_path, &text)
        .map_err(|error| CliError::Io(format!("failed to write {expanded_path}: {error}")))?;
    println!(
        "wrote {expanded_path} ({} invocation{} expanded)",
        outcome.records.len(),
        if outcome.records.len() == 1 { "" } else { "s" }
    );

    // Parse + check the expanded file so the user immediately sees whether
    // the generated program is well formed (spans refer to that file).
    let parsed = cme_compiler::parse_source(&text);
    let mut errors = parsed.diagnostics;
    errors.extend(cme_compiler::check::check(&parsed.statements));
    render_diagnostics(errors, &text)
}

/// `magic.cm` → `magic_expanded.cm`, side by side with the original.
#[cfg(feature = "cli")]
fn expanded_path_for(path: &str) -> String {
    let stem = std::path::Path::new(path)
        .with_extension("")
        .into_os_string()
        .into_string()
        .unwrap_or_else(|_| path.to_string());
    format!("{stem}_expanded.cm")
}

#[cfg(feature = "cli")]
fn run_program(
    source: &str,
    path: &str,
    schema: Option<&cme_compiler::schema::SchemaContext>,
) -> Result<(), CliError> {
    let outcome = cme_compiler::parse_source(source);
    let mut errors = outcome.diagnostics;
    errors.extend(cme_compiler::mods::standalone_import_diagnostics(
        &outcome.statements,
    ));
    let declarations = schema
        .map(cme_compiler::schema::declaration_statements)
        .unwrap_or_default();
    errors.extend(match schema {
        Some(context) => cme_compiler::check::check_with_schema(&outcome.statements, Some(context)),
        None => cme_compiler::check::check(&outcome.statements),
    });
    // Never run broken code: refuse before invoking anything.
    render_diagnostics(errors, source)?;

    // §2.1: the host picks the entry point — `main` is the CLI convention,
    // not a language concept.
    let has_main = outcome
        .statements
        .iter()
        .any(|stmt| matches!(&stmt.kind, StmtKind::FuncDecl { name, .. } if name == "main"));
    if !has_main {
        return Err(CliError::Usage(format!(
            "no `main` function to run in {path}"
        )));
    }

    let interpreter = Interpreter::new(&outcome.statements).with_declarations(&declarations);
    match interpreter.invoke("main", &[]) {
        Ok(value) => {
            // Void prints nothing; every other value prints via Display.
            if !matches!(value, Value::Void) {
                println!("{value}");
            }
            Ok(())
        }
        Err(error) => Err(CliError::Runtime(error, source.to_string())),
    }
}

/// Loads every `--schema` file into one context. `targets` overrides the
/// per-namespace target versions (the mod path). Each file's diagnostics
/// render against its own text, so positions name the schema file.
#[cfg(feature = "cli")]
fn load_schema_context(
    paths: &[String],
    targets: Option<Vec<(String, String)>>,
) -> Result<Option<cme_compiler::schema::SchemaContext>, CliError> {
    if paths.is_empty() {
        return Ok(None);
    }
    let mut files = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(path)
            .map_err(|error| CliError::Io(format!("failed to read {path}: {error}")))?;
        let outcome = cme_compiler::schema::parse_schema_file(&text);
        if !outcome.is_clean() {
            let rendered: Vec<String> = outcome
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    render_message_at(diagnostic.message(), diagnostic.span(), &text, path)
                })
                .collect();
            return Err(CliError::Schema(rendered));
        }
        files.push(outcome.file.expect("clean parse yields the file"));
    }
    let set = cme_compiler::schema::SchemaSet::build(files).map_err(|issues| {
        CliError::Schema(issues.iter().map(|issue| issue.message.clone()).collect())
    })?;
    let context = match targets {
        Some(targets) => {
            cme_compiler::schema::SchemaContext::grant_targets(set, targets).map_err(|issues| {
                CliError::Schema(issues.iter().map(|issue| issue.message.clone()).collect())
            })?
        }
        None => cme_compiler::schema::SchemaContext::grant_all(set),
    };
    Ok(Some(context))
}

/// `cme schema <file.cm>`: validates one schema file against §9 and
/// prints every defect; a clean schema prints nothing and exits 0.
#[cfg(feature = "cli")]
fn schema_command(path: &str) -> Result<(), CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| CliError::Io(format!("failed to read {path}: {error}")))?;
    let outcome = cme_compiler::schema::parse_schema_file(&text);
    if !outcome.is_clean() {
        let rendered: Vec<String> = outcome
            .diagnostics
            .iter()
            .map(|diagnostic| {
                render_message_at(diagnostic.message(), diagnostic.span(), &text, path)
            })
            .collect();
        return Err(CliError::Schema(rendered));
    }
    // Cross-file invariants even for a single file (duplicate names,
    // unresolved requires — §9.2/§9.4).
    let set = cme_compiler::schema::SchemaSet::build(vec![outcome.file.expect("file")]).map_err(
        |issues| CliError::Schema(issues.iter().map(|issue| issue.message.clone()).collect()),
    )?;
    let _ = set;
    Ok(())
}

/// `cme codegen-c <schema.cm>`: writes the §9.6 C header to stdout.
#[cfg(feature = "cli")]
fn codegen_c_command(path: &str) -> Result<(), CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| CliError::Io(format!("failed to read {path}: {error}")))?;
    let outcome = cme_compiler::schema::parse_schema_file(&text);
    if !outcome.is_clean() {
        let rendered: Vec<String> = outcome
            .diagnostics
            .iter()
            .map(|diagnostic| {
                render_message_at(diagnostic.message(), diagnostic.span(), &text, path)
            })
            .collect();
        return Err(CliError::Schema(rendered));
    }
    let file = outcome.file.expect("a clean parse yields the file");
    print!("{}", cme_compiler::schema::codegen_c(&file, "cme.h"));
    Ok(())
}

#[cfg(feature = "cli")]
fn render_diagnostics(errors: Vec<Diagnostic>, source: &str) -> Result<(), CliError> {
    if !errors.is_empty() {
        return Err(CliError::Compiler(errors, source.to_string()));
    }
    Ok(())
}

// Single-error convenience wrapper; the batch path uses `SourceLayout`
// directly, and the test module exercises this one.
#[cfg(feature = "cli")]
#[allow(dead_code)]
fn render_error(error: &Diagnostic, source: &str, path: &str) -> String {
    render_message_at(error.message(), error.span(), source, path)
}

/// A one-shot index of the source's line starts. Building it once turns a
/// whole-diagnostics render from O(errors × file) byte scans into O(errors)
/// — recovery-heavy files produce thousands of diagnostics, and per-
/// diagnostic rescans made reporting the dominant cost.
#[cfg(feature = "cli")]
struct SourceLayout {
    line_starts: Vec<usize>,
}

#[cfg(feature = "cli")]
impl SourceLayout {
    fn new(source: &str) -> Self {
        let mut line_starts = vec![0usize];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Self { line_starts }
    }

    /// The 1-based line of `offset`.
    fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(index) => index + 1,
            Err(index) => index,
        }
        .max(1)
    }

    /// The byte offset where `line` (1-based) starts.
    fn line_start(&self, line: usize) -> usize {
        self.line_starts
            .get(line - 1)
            .copied()
            .unwrap_or_else(|| *self.line_starts.last().unwrap_or(&0))
    }

    /// The full text of `line` (1-based), without its terminator.
    fn line_text<'src>(&self, source: &'src str, line: usize) -> &'src str {
        let start = self.line_start(line);
        let end = source[start..]
            .find('\n')
            .map_or(source.len(), |relative| start + relative);
        &source[start..end]
    }

    /// The 1-based (line, column) of `offset`, columns counted in
    /// characters so carets stay aligned on multibyte lines.
    fn line_column(&self, source: &str, offset: usize) -> (usize, usize) {
        let line = self.line_of(offset);
        let line_start = self.line_start(line);
        let column_chars = source
            .get(line_start..offset.min(source.len()))
            .map_or(0, |text| text.chars().count());
        (line, column_chars + 1)
    }
}

/// Renders `message` located at `span` with the caret machinery. Shared by
/// compile-time diagnostics and interpreter runtime errors. Positions are
/// CHARACTER based, so carets stay aligned on lines containing multibyte
/// UTF-8 text (byte offsets would smear the column past its true position).
#[cfg(feature = "cli")]
fn render_message_at(message: &str, span: Span, source: &str, path: &str) -> String {
    let layout = SourceLayout::new(source);
    render_message_indexed(message, span, source, path, &layout)
}

/// The indexed variant: same output as [`render_message_at`], but the line
/// layout is provided by the caller so batch rendering stays linear.
#[cfg(feature = "cli")]
fn render_message_indexed(
    message: &str,
    span: Span,
    source: &str,
    path: &str,
    layout: &SourceLayout,
) -> String {
    let (line, column) = layout.line_column(source, span.start);
    let line_text = layout.line_text(source, line);
    let start_byte = layout.line_start(line);
    let leading = span.start.saturating_sub(start_byte);
    let prefix = String::from_utf8_lossy(&line_text.as_bytes()[..leading.min(line_text.len())])
        .chars()
        .count();
    // A span that crosses a line break renders only its first-line
    // portion; `...` marks that the span continues on a later line. A
    // span that ends with the line break itself is still single-line.
    let line_end = start_byte + line_text.len();
    let visible_end = span.end.min(line_end);
    let width = visible_end.saturating_sub(span.start).max(1);
    let mut caret = String::new();
    caret.push_str(&" ".repeat(prefix));
    caret.push_str(&"^".repeat(width));
    if span.end > line_end + 1 {
        caret.push_str("...");
    }
    format!("{path}:{line}:{column}: {message}\n{line_text}\n{caret}")
}

// This binary is only compiled if the user installs the CLI toolchain.
/// The display path for `Compiler`/`Runtime` diagnostics: the file
/// argument AFTER `--schema` pairs are consumed — the same positional
/// layout `run` parses, so `cme check --schema s.cm x.cm` renders
/// `x.cm:…`, not the literal `--schema` that `args().nth(2)` would name.
#[cfg(feature = "cli")]
fn rendered_source_path() -> String {
    let mut positionals: Vec<String> = Vec::new();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--schema" {
            // Skip the flag's value too.
            iter.next();
            continue;
        }
        positionals.push(arg);
    }
    positionals.get(1).cloned().unwrap_or_default()
}

#[cfg(feature = "cli")]
fn main() -> ExitCode {
    let source_path = rendered_source_path();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(message) | CliError::Io(message)) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
        Err(CliError::Schema(lines)) => {
            for line in lines {
                eprintln!("error: {line}");
            }
            ExitCode::FAILURE
        }
        Err(CliError::Compiler(errors, source)) => {
            // One layout for the whole batch: thousands of recovery
            // diagnostics render in linear time.
            let layout = SourceLayout::new(&source);
            for error in errors {
                eprintln!(
                    "error: {}",
                    render_message_indexed(
                        error.message(),
                        error.span(),
                        &source,
                        &source_path,
                        &layout
                    )
                );
            }
            ExitCode::FAILURE
        }
        Err(CliError::Runtime(error, source)) => {
            let rendered = render_message_at(&error.message, error.span, &source, &source_path);
            eprintln!("error: {rendered}");
            ExitCode::FAILURE
        }
        Err(CliError::Mod(lines)) => {
            for line in lines {
                eprintln!("error: {line}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "cli"))]
fn main() {
    eprintln!("Error: The 'cme' CLI was built without the 'cli' feature.");
    eprintln!("Try compiling with: cargo build --features cli");
    std::process::exit(1);
}

#[cfg(all(test, feature = "cli"))]
mod tests {
    use super::render_error;
    use cme_compiler::diagnostics::Diagnostic;
    use cme_core::Span;

    fn rendered(source: &str, start: usize, end: usize) -> String {
        render_error(
            &Diagnostic::type_error("boom", Span::new(start, end)),
            source,
            "t.cm",
        )
    }

    fn caret_line(rendered: &str) -> &str {
        rendered.lines().nth(2).expect("message, line, caret")
    }

    #[test]
    fn single_line_span_renders_an_exact_caret_run() {
        let source = "infer x = 1\n";
        let rendered = rendered(source, 10, 11);
        assert!(rendered.starts_with("t.cm:1:11: boom\n"));
        assert_eq!(caret_line(&rendered), "          ^");
    }

    #[test]
    fn span_swallowing_the_line_break_is_still_single_line() {
        // The span covers "x = 1" plus the trailing newline and nothing
        // beyond it: no continuation marker, carets stop at line end.
        let source = "infer x = 1\n";
        let rendered = rendered(source, 6, 12);
        assert_eq!(caret_line(&rendered), "      ^^^^^");
    }

    #[test]
    fn multi_line_span_clamps_its_carets_to_the_first_line() {
        // The span covers `{`, a newline, a whole statement, and `}`;
        // only the `{` sits on line 1, so one caret plus `...`.
        let source = "int f() {\nint x = 1\n}\n";
        let rendered = rendered(source, 8, 21);
        assert!(rendered.starts_with("t.cm:1:9: boom\n"));
        assert_eq!(caret_line(&rendered), "        ^...");
    }

    #[test]
    fn missing_return_over_a_multi_line_body_renders_clamped() {
        // End to end: the body block span is multi-line, and the caret
        // stays on the first line instead of one long `^` run.
        let source = "int f() {\nint x = 1\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let errors = cme_compiler::check::check(&outcome.statements);
        assert_eq!(errors.len(), 1);
        let rendered = render_error(&errors[0], source, "t.cm");
        assert!(rendered.starts_with("t.cm:1:9: missing return in non-void function `f`\n"));
        assert_eq!(caret_line(&rendered), "        ^...");
    }

    #[test]
    fn multibyte_lines_keep_the_caret_aligned() {
        // The prefix before the span contains two-character-wide multibyte
        // runes; the caret must align by CHARACTERS, not bytes.
        let source = "str s = \"日本語\" + x\n";
        let span_start = source.find("x").unwrap();
        let rendered = rendered(source, span_start, span_start + 1);
        let lines: Vec<&str> = rendered.lines().collect();
        let caret = lines[2];
        let target = lines[1].chars().position(|c| c == 'x').unwrap();
        assert_eq!(caret.chars().position(|c| c == '^').unwrap(), target);
        assert!(rendered.starts_with("t.cm:1:"));
    }
}
