//! Multi-file mod support (WHITEPAPER §10, §2.3).
//!
//! A mod is a self-contained directory holding a `mod.toml` manifest and a
//! `src/` tree of `.cm` files (§10.1). Every file path under `src/` maps
//! directly to an internal module path: `src/gamemode/rules.cm` is
//! `self.gamemode.rules`, and `self` is the reserved root of the local mod
//! tree (§10.3). Sources import each other with
//! `import self.gamemode.rules`; any other import root names a host schema
//! namespace, which this subsystem accepts syntactically but cannot resolve
//! — schema contracts, `requires` edges, and `since` gating are future work
//! tied to the §9 schema system, as recorded in AGENTS.md.
//!
//! The pipeline has three layers, from disk to a checkable program:
//!
//! 1. [`parse_manifest`] — reads `mod.toml` (§10.2): the three identity
//!    fields plus a `[schemas]` table of namespace → target-version pairs.
//!    The manifest grammar is the TOML subset the format actually uses
//!    (comments, bare/quoted keys, single-quoted and double-quoted strings,
//!    one table header); anything outside the subset is rejected with a
//!    line-anchored diagnostic instead of being guessed at. Versions must
//!    be `X.Y.Z`. `[schemas]` entries are validated for shape only —
//!    enforcing them (hiding capabilities newer than the declared version,
//!    §9.5) needs real schemas.
//! 2. [`load_mod`] — discovers the `src/` tree, maps every `.cm` file to
//!    its module path, validates that every path segment is a Checkmate
//!    identifier (an unmappable file would be silently unimportable, which
//!    is worse than a loud error), and reads each source. Discovery is
//!    deterministic: entries are traversed and kept in sorted module-path
//!    order regardless of the operating system's directory order.
//! 3. [`assemble`] — concatenates the (already megaprogram-expanded)
//!    sources into one virtual program text, remembers each module's byte
//!    range, and runs the ordinary front end once over the whole text.
//!    One parse and one check pass over one text is what makes cross-file
//!    linking exact: duplicate top-level names across files collide in the
//!    single namespace they would actually share, and `impl` blocks for the
//!    same target union across the mod tree with duplicate-member
//!    rejection, exactly as §10.4 specifies. `self`-rooted imports are
//!    resolved against the discovered module table; an import naming no
//!    module is a diagnostic at the import site.
//!
//! Diagnostics stay per-file through [`ModuleRange`]: every diagnostic's
//! span lands inside exactly one module's byte range, so callers can
//! re-anchor it to that module's own text and report
//! `my_mod/src/gamemode/rules.cm:3:5` instead of a meaningless offset into
//! the concatenation. Two recovery shapes swallow "the rest of the file"
//! by design (an unterminated block comment, an unclosed brace at end of
//! input); inside a mod build "the file" is the concatenation, so such a
//! diagnostic may reach past its module's boundary — it is attributed to
//! the module the shape starts in and clamped there.

use std::path::{Path, PathBuf};

use cme_core::Span;
use cme_core::ast::{Stmt, StmtKind};

use crate::diagnostics::Diagnostic;

/// A mod-level problem that is not anchored to a `.cm` source span: a
/// manifest defect (with its 1-based `mod.toml` line when known) or a
/// structure defect (missing/invalid `src/` tree, unreadable file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModIssue {
    pub message: String,
    /// The file the issue belongs to, for display prefixes (`mod.toml` or a
    /// mod-relative source path).
    pub file: Option<String>,
    /// The 1-based line inside `file`, when the issue comes from a line of
    /// text (manifest parsing).
    pub line: Option<usize>,
}

impl ModIssue {
    fn manifest(message: impl Into<String>, line: usize) -> Self {
        Self {
            message: message.into(),
            file: Some("mod.toml".to_string()),
            line: Some(line),
        }
    }

    fn structure(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            file: None,
            line: None,
        }
    }

    fn at_file(message: impl Into<String>, file: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            file: Some(file.into()),
            line: None,
        }
    }
}

/// The parsed `mod.toml` (§10.2). Schema entries keep declaration order so
/// consumers (and tests) see a deterministic table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModManifest {
    pub name: String,
    pub version: String,
    pub checkmate_version: String,
    pub schemas: Vec<(String, String)>,
}

/// One discovered `.cm` file: its module path (`self`-rooted), its on-disk
/// location, and its path relative to the mod root for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredModule {
    pub module_path: Vec<String>,
    pub file_path: PathBuf,
    pub display_path: String,
}

/// A discovered module together with its read source text. `source` is
/// replaced by the megaprogram expansion before assembly, so assembly
/// ranges always describe the text that is actually parsed.
#[derive(Debug, Clone)]
pub struct LoadedModule {
    pub module_path: Vec<String>,
    pub display_path: String,
    pub source: String,
}

/// The outcome of loading a mod directory. `issues` being empty is the
/// gate: with any issue, `manifest`/`modules` may be incomplete and the mod
/// must not be compiled.
#[derive(Debug, Clone)]
pub struct LoadedMod {
    pub manifest: Option<ModManifest>,
    pub modules: Vec<LoadedModule>,
    pub issues: Vec<ModIssue>,
}

/// True when `name` is a legal Checkmate identifier segment
/// (`[A-Za-z_][A-Za-z0-9_]*`) — the shape a module path segment must have
/// to be spellable in an import (§10.3).
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// True for `X.Y.Z` with all-numeric components — the version shape every
/// manifest version field uses (§10.2, §9.5).
fn is_mod_version(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

// ---------------------------------------------------------------------------
// Manifest parsing (§10.2)
// ---------------------------------------------------------------------------

/// The manifest fields allowed at the top level, for the unknown-field
/// diagnostic.
const TOP_LEVEL_KEYS: [&str; 3] = ["name", "version", "checkmate_version"];

/// Parses a `mod.toml` manifest. Returns every defect found (a mod with a
/// broken manifest has no usable identity, so the caller must treat any
/// error as fatal), or the manifest on success.
///
/// The accepted grammar is the TOML subset the §10.2 format uses: comments
/// (`#`), blank lines, `key = "value"` pairs with bare or quoted keys, and
/// one `[schemas]` table. Multiline strings, typed values (integers,
/// booleans, arrays, inline tables), and dotted keys are outside the
/// subset and are rejected with a pointed diagnostic rather than
/// silently reinterpreted.
pub fn parse_manifest(text: &str) -> Result<ModManifest, Vec<ModIssue>> {
    let mut issues: Vec<ModIssue> = Vec::new();
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut checkmate_version: Option<String> = None;
    let mut schemas: Vec<(String, String)> = Vec::new();
    // The current table: `None` at the top level, `Some` after a header.
    let mut section: Option<String> = None;
    let mut seen_sections: Vec<String> = Vec::new();
    // (section, key) pairs for duplicate detection; the section is `None`
    // for top-level keys.
    let mut seen_keys: Vec<(Option<String>, String)> = Vec::new();

    for (index, raw_line) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix('[') {
            let Some((header, consumed)) = scan_table_header(rest) else {
                issues.push(ModIssue::manifest(
                    "expected a table name followed by `]`",
                    line_no,
                ));
                continue;
            };
            if !rest_after_value_is_comment(&rest[consumed..]) {
                issues.push(ModIssue::manifest(
                    "unexpected text after the table header",
                    line_no,
                ));
                continue;
            }
            if seen_sections.contains(&header) {
                issues.push(ModIssue::manifest(
                    format!("duplicate table `[{header}]`"),
                    line_no,
                ));
                continue;
            }
            if header != "schemas" {
                issues.push(ModIssue::manifest(
                    format!("unknown table `[{header}]`; expected `[schemas]`"),
                    line_no,
                ));
                continue;
            }
            seen_sections.push(header.clone());
            section = Some(header);
            continue;
        }
        // key = value
        let Some((key, after_key)) = scan_key(line) else {
            issues.push(ModIssue::manifest("expected a key", line_no));
            continue;
        };
        let rest = line[after_key..].trim_start();
        let Some(value_text) = rest.strip_prefix('=') else {
            issues.push(ModIssue::manifest(
                format!("expected `=` after key `{key}`"),
                line_no,
            ));
            continue;
        };
        let value_text = value_text.trim_start();
        let Some((value, consumed)) = scan_string_value(value_text) else {
            issues.push(ModIssue::manifest(
                format!("key `{key}` needs a string value in double or single quotes"),
                line_no,
            ));
            continue;
        };
        if !rest_after_value_is_comment(&value_text[consumed..]) {
            issues.push(ModIssue::manifest(
                "unexpected text after the value",
                line_no,
            ));
            continue;
        }

        let key_tuple = (section.clone(), key.clone());
        if seen_keys.contains(&key_tuple) {
            let where_text = match &section {
                Some(table) => format!("`[{table}]`"),
                None => "the manifest".to_string(),
            };
            issues.push(ModIssue::manifest(
                format!("duplicate key `{key}` in {where_text}"),
                line_no,
            ));
            continue;
        }
        seen_keys.push(key_tuple);

        match &section {
            None => match key.as_str() {
                "name" => {
                    if value.is_empty() {
                        issues.push(ModIssue::manifest("`name` must not be empty", line_no));
                    }
                    name = Some(value);
                }
                "version" => {
                    validate_version_field("version", &value, line_no, &mut issues);
                    version = Some(value);
                }
                "checkmate_version" => {
                    validate_version_field("checkmate_version", &value, line_no, &mut issues);
                    checkmate_version = Some(value);
                }
                other => {
                    issues.push(ModIssue::manifest(
                        format!(
                            "unknown field `{other}`; expected one of {} \
                             (schema versions live under `[schemas]`)",
                            TOP_LEVEL_KEYS
                                .iter()
                                .map(|key| format!("`{key}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        line_no,
                    ));
                }
            },
            Some(table) if table == "schemas" => {
                if !is_identifier(&key) {
                    issues.push(ModIssue::manifest(
                        format!("schema namespace `{key}` is not a valid identifier"),
                        line_no,
                    ));
                }
                validate_version_field(&format!("schemas.{key}"), &value, line_no, &mut issues);
                schemas.push((key, value));
            }
            _ => {}
        }
    }

    if name.is_none() {
        issues.push(ModIssue::structure("manifest is missing the `name` field"));
    }
    if version.is_none() {
        issues.push(ModIssue::structure(
            "manifest is missing the `version` field",
        ));
    }
    if checkmate_version.is_none() {
        issues.push(ModIssue::structure(
            "manifest is missing the `checkmate_version` field",
        ));
    }

    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(ModManifest {
        name: name.unwrap_or_default(),
        version: version.unwrap_or_default(),
        checkmate_version: checkmate_version.unwrap_or_default(),
        schemas,
    })
}

fn validate_version_field(field: &str, value: &str, line_no: usize, issues: &mut Vec<ModIssue>) {
    if !is_mod_version(value) {
        issues.push(ModIssue::manifest(
            format!("`{field}` must be a version like `1.0.0`, but is `{value}`"),
            line_no,
        ));
    }
}

/// Scans a table header `[name]` (without the opening bracket): the name
/// followed by the closing `]`. Returns the header name and how many bytes
/// of the input it consumed including the bracket, or `None` when the
/// header is empty or the bracket is missing on this line.
fn scan_table_header(rest: &str) -> Option<(String, usize)> {
    let (name, consumed) = scan_key(rest)?;
    if name.is_empty() {
        return None;
    }
    let after = rest[consumed..].trim_start();
    if !after.starts_with(']') {
        return None;
    }
    Some((name, consumed + (rest.len() - consumed - after.len()) + 1))
}

/// Scans a manifest key: a bare `[A-Za-z0-9_-]+` run or a quoted string.
/// Returns the key and the consumed byte count.
fn scan_key(text: &str) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    match bytes[0] {
        b'"' => scan_basic_string(text),
        b'\'' => scan_literal_string(text),
        first if first.is_ascii_alphanumeric() || first == b'_' || first == b'-' => {
            let mut end = 0;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'-')
            {
                end += 1;
            }
            Some((text[..end].to_string(), end))
        }
        _ => None,
    }
}

/// Scans a double-quoted TOML basic string with the common escape set
/// (`\\`, `\"`, `\n`, `\t`, `\r`). Returns the unescaped content and the
/// consumed count including both quotes.
fn scan_basic_string(text: &str) -> Option<(String, usize)> {
    let mut chars = text.char_indices();
    let (_, opening) = chars.next()?; // the leading quote
    let mut value = String::new();
    while let Some((index, current)) = chars.next() {
        match current {
            '"' => {
                return Some((value, index + opening.len_utf8()));
            }
            '\\' => match chars.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, 't')) => value.push('\t'),
                Some((_, 'r')) => value.push('\r'),
                Some((_, '"')) => value.push('"'),
                Some((_, '\\')) => value.push('\\'),
                Some((_, other)) => {
                    // Outside the subset: reject the whole value rather
                    // than guessing at the intent.
                    let _ = other;
                    return None;
                }
                None => return None,
            },
            '\n' => return None, // single-line strings only
            other => value.push(other),
        }
    }
    None // unterminated before end of line
}

/// Scans a single-quoted TOML literal string: no escapes, verbatim.
fn scan_literal_string(text: &str) -> Option<(String, usize)> {
    let quote = text.chars().next()?;
    let rest = &text[quote.len_utf8()..];
    let end = rest.find('\'')?;
    // A literal string cannot span lines in the subset.
    if rest[..end].contains('\n') {
        return None;
    }
    Some((rest[..end].to_string(), quote.len_utf8() + end + 1))
}

/// True when the text after a value is only whitespace and at most one
/// comment.
fn rest_after_value_is_comment(rest: &str) -> bool {
    let rest = rest.trim_start();
    rest.is_empty() || rest.starts_with('#')
}

/// Scans a manifest value: `"""`/`'''` multiline forms are outside the
/// subset (rejected by returning `None`, like any non-string), and the
/// caller distinguishes the error message.
fn scan_string_value(text: &str) -> Option<(String, usize)> {
    if text.starts_with("\"\"\"") || text.starts_with("'''") {
        return None;
    }
    match text.as_bytes().first() {
        Some(b'"') => scan_basic_string(text),
        Some(b'\'') => scan_literal_string(text),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Discovery (§10.1, §10.3)
// ---------------------------------------------------------------------------

/// Walks `mod_root/src/` and maps every `.cm` file to its module path.
/// Dot-prefixed entries are skipped. A file or directory whose name cannot
/// be a module path segment is reported and skipped: silently excluding
/// it would hide code from compilation. The result is sorted by module
/// path so both diagnostics and the assembled program are deterministic.
pub fn discover_modules(mod_root: &Path) -> Result<Vec<DiscoveredModule>, Vec<ModIssue>> {
    let src = mod_root.join("src");
    let mut issues = Vec::new();
    if !src.exists() {
        return Err(vec![ModIssue::structure(format!(
            "mod is missing a `src/` directory (expected {})",
            src.display()
        ))]);
    }
    if !src.is_dir() {
        return Err(vec![ModIssue::structure(format!(
            "`src` is not a directory ({})",
            src.display()
        ))]);
    }

    let mut discovered = Vec::new();
    walk_src(&src, &[], &mut discovered, &mut issues);
    if discovered.is_empty() && issues.is_empty() {
        issues.push(ModIssue::structure(
            "no `.cm` files found under `src/`".to_string(),
        ));
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    discovered.sort_by(|left, right| left.module_path.cmp(&right.module_path));
    Ok(discovered)
}

fn walk_src(
    dir: &Path,
    prefix: &[String],
    discovered: &mut Vec<DiscoveredModule>,
    issues: &mut Vec<ModIssue>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            issues.push(ModIssue::structure(format!(
                "cannot read directory `{}`: {error}",
                dir.display()
            )));
            return;
        }
    };
    // read_dir order is OS-dependent; sort for deterministic traversal.
    let mut entries: Vec<std::fs::DirEntry> = entries.filter_map(|entry| entry.ok()).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir {
            if !is_identifier(&name) {
                issues.push(ModIssue::structure(format!(
                    "directory `{}` cannot map to a module path: \
                     `{name}` is not a valid identifier segment",
                    entry.path().display()
                )));
                continue;
            }
            let mut child = prefix.to_vec();
            child.push(name);
            walk_src(&entry.path(), &child, discovered, issues);
        } else if name.ends_with(".cm") {
            let stem = &name[..name.len() - ".cm".len()];
            if !is_identifier(stem) {
                issues.push(ModIssue::structure(format!(
                    "file `{}` cannot map to a module path: \
                     `{stem}` is not a valid identifier segment",
                    entry.path().display()
                )));
                continue;
            }
            let mut module_path = vec!["self".to_string()];
            module_path.extend(prefix.iter().cloned());
            module_path.push(stem.to_string());
            let mut display = prefix.join("/");
            if !display.is_empty() {
                display.push('/');
            }
            display.push_str(stem);
            display.push_str(".cm");
            discovered.push(DiscoveredModule {
                module_path,
                file_path: entry.path(),
                display_path: format!("src/{display}"),
            });
        }
        // Non-`.cm` files are not modules; a mod tree may carry assets.
    }
}

/// Loads a mod directory: manifest first, then discovery, then reading
/// every source. Structural problems accumulate in `issues`; the caller
/// must refuse to compile while it is non-empty.
pub fn load_mod(mod_root: &Path) -> LoadedMod {
    let mut issues = Vec::new();

    let manifest_path = mod_root.join("mod.toml");
    let manifest = if !manifest_path.exists() {
        issues.push(ModIssue::structure(format!(
            "no mod.toml in {}",
            mod_root.display()
        )));
        None
    } else {
        match std::fs::read_to_string(&manifest_path) {
            Ok(text) => match parse_manifest(&text) {
                Ok(manifest) => Some(manifest),
                Err(mut manifest_issues) => {
                    issues.append(&mut manifest_issues);
                    None
                }
            },
            Err(error) => {
                issues.push(ModIssue::at_file(
                    format!("cannot read mod.toml: {error}"),
                    "mod.toml",
                ));
                None
            }
        }
    };

    let modules = match discover_modules(mod_root) {
        Ok(discovered) => discovered
            .into_iter()
            .filter_map(|module| {
                let DiscoveredModule {
                    module_path,
                    file_path,
                    display_path,
                } = module;
                match std::fs::read_to_string(&file_path) {
                    Ok(source) => Some(LoadedModule {
                        module_path,
                        display_path,
                        source,
                    }),
                    Err(error) => {
                        issues.push(ModIssue::at_file(
                            format!("cannot read module: {error}"),
                            display_path,
                        ));
                        None
                    }
                }
            })
            .collect(),
        Err(mut discovery_issues) => {
            issues.append(&mut discovery_issues);
            Vec::new()
        }
    };

    LoadedMod {
        manifest,
        modules,
        issues,
    }
}

// ---------------------------------------------------------------------------
// Assembly (§10.3 static linking, §10.4 impl union)
// ---------------------------------------------------------------------------

/// One module's byte range inside the assembled virtual program text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRange {
    pub module_path: Vec<String>,
    pub display_path: String,
    /// Inclusive start offset of the module's text in the virtual source.
    pub start: usize,
    /// Exclusive end offset (start + source length).
    pub end: usize,
}

/// The assembled mod program: one parsed statement list over the virtual
/// text, every diagnostic in virtual-text coordinates, and the byte ranges
/// needed to re-anchor those coordinates to individual files.
#[derive(Debug)]
pub struct AssembledProgram {
    pub statements: Vec<Stmt>,
    pub diagnostics: Vec<Diagnostic>,
    pub ranges: Vec<ModuleRange>,
    pub source: String,
}

/// Concatenates the module sources into one virtual program text (modules
/// separated by blank lines), parses it with the ordinary front end, and
/// validates every `self`-rooted import against the module table.
///
/// Modules must already be megaprogram-expanded; assembly works on plain
/// Checkmate text only. The order of `modules` is the order of the
/// concatenation ([`load_mod`] sorts by module path, so it is
/// deterministic).
pub fn assemble(modules: &[LoadedModule]) -> AssembledProgram {
    let mut source = String::new();
    let mut ranges = Vec::with_capacity(modules.len());
    for (index, module) in modules.iter().enumerate() {
        if index > 0 {
            source.push_str("\n\n");
        }
        let start = source.len();
        source.push_str(&module.source);
        ranges.push(ModuleRange {
            module_path: module.module_path.clone(),
            display_path: module.display_path.clone(),
            start,
            end: start + module.source.len(),
        });
    }

    let outcome = crate::parse_source(&source);
    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(validate_imports(&outcome.statements, modules));

    AssembledProgram {
        statements: outcome.statements,
        diagnostics,
        ranges,
        source,
    }
}

/// Validates import statements against the discovered module table.
/// `self`-rooted imports must name a module of the mod (§10.3); imports
/// with any other root target host schema namespaces, which are accepted
/// here and left to the future §9 schema checker.
fn validate_imports(statements: &[Stmt], modules: &[LoadedModule]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for statement in statements {
        let StmtKind::Import { path } = &statement.kind else {
            continue;
        };
        if path.first().map(String::as_str) != Some("self") {
            continue;
        }
        let known = modules.iter().any(|module| {
            module.module_path.len() == path.len() && module.module_path[1..] == path[1..]
        });
        if !known {
            let dotted = path.join(".");
            let relative = path[1..].join("/");
            diagnostics.push(Diagnostic::parse(
                format!(
                    "import of `{dotted}` does not match any module in this mod \
                     (expected a file at `src/{relative}.cm`)"
                ),
                statement.span,
            ));
        }
    }
    diagnostics
}

/// Re-anchors a virtual-text span to the module that owns it, returning
/// the module range plus the span translated into that module's own
/// coordinates. Spans are clamped into the range (a recovery shape may
/// reach past its module's end, see the module docs).
pub fn attribute_span(ranges: &[ModuleRange], span: Span) -> Option<(&ModuleRange, Span)> {
    let owner = ranges
        .iter()
        .filter(|range| range.start <= span.start)
        .max_by_key(|range| range.start)?;
    let start = span.start.saturating_sub(owner.start);
    let end = span
        .end
        .saturating_sub(owner.start)
        .clamp(start, owner.end - owner.start);
    Some((owner, Span::new(start, end)))
}

/// Diagnostics for imports in a standalone (single-file) build. A
/// `self`-rooted import has no mod tree to resolve against, so it is
/// always an error; host-rooted imports remain syntactically legal.
pub fn standalone_import_diagnostics(statements: &[Stmt]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for statement in statements {
        let StmtKind::Import { path } = &statement.kind else {
            continue;
        };
        if path.first().map(String::as_str) == Some("self") {
            diagnostics.push(Diagnostic::parse(
                format!(
                    "cannot import `{}` outside of a mod; `self` names the \
                     internal tree of a mod directory with a mod.toml (§10.1)",
                    path.join(".")
                ),
                statement.span,
            ));
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- manifest -----------------------------------------------------------

    #[test]
    fn parses_a_full_manifest() {
        let text = "\
name = \"advanced_rules\"
version = \"1.0.0\"
checkmate_version = \"0.5.0\"

[schemas]
engine = \"1.4.0\"
physics = '1.0.0'   # literal strings work too
";
        let manifest = parse_manifest(text).unwrap_or_else(|issues| panic!("{issues:#?}"));
        assert_eq!(manifest.name, "advanced_rules");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.checkmate_version, "0.5.0");
        assert_eq!(
            manifest.schemas,
            vec![
                ("engine".to_string(), "1.4.0".to_string()),
                ("physics".to_string(), "1.0.0".to_string()),
            ]
        );
    }

    #[test]
    fn parses_a_minimal_manifest_without_schemas() {
        let manifest =
            parse_manifest("name = \"m\"\nversion = \"0.1.0\"\ncheckmate_version = \"0.5.0\"\n")
                .unwrap_or_else(|issues| panic!("{issues:#?}"));
        assert!(manifest.schemas.is_empty());
    }

    #[test]
    fn manifest_escapes_and_comments_are_honored() {
        let text = "\
# identity
name = \"a \\\"quoted\\\" name\"
version = \"1.0.0\"    # trailing comment
checkmate_version = \"0.5.0\"
";
        let manifest = parse_manifest(text).unwrap_or_else(|issues| panic!("{issues:#?}"));
        assert_eq!(manifest.name, "a \"quoted\" name");
    }

    #[test]
    fn manifest_missing_fields_are_reported_individually() {
        let issues = parse_manifest("[schemas]\nengine = \"1.0.0\"\n").unwrap_err();
        for field in ["`name`", "`version`", "`checkmate_version`"] {
            assert!(
                issues.iter().any(|issue| issue.message.contains(field)),
                "{issues:#?}"
            );
        }
    }

    #[test]
    fn manifest_rejects_unknown_fields_and_tables() {
        let issues = parse_manifest(
            "name = \"m\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.5.0\"\ncheese = \"yes\"\n",
        )
        .unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("unknown field `cheese`")),
            "{issues:#?}"
        );

        let issues = parse_manifest(
            "name = \"m\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.5.0\"\n[secrets]\n",
        )
        .unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("unknown table `[secrets]`")),
            "{issues:#?}"
        );
    }

    #[test]
    fn manifest_rejects_duplicates() {
        let issues = parse_manifest(
            "name = \"m\"\nname = \"n\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.5.0\"\n",
        )
        .unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("duplicate key `name`")),
            "{issues:#?}"
        );

        let issues = parse_manifest(
            "name = \"m\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.5.0\"\n[schemas]\n[schemas]\n",
        )
        .unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("duplicate table")),
            "{issues:#?}"
        );
    }

    #[test]
    fn manifest_versions_must_be_x_y_z() {
        for bad in ["1.0", "one.two.three", "1..0", "1.0.0.0", ""] {
            let text =
                format!("name = \"m\"\nversion = \"{bad}\"\ncheckmate_version = \"0.5.0\"\n");
            let issues = parse_manifest(&text).unwrap_err();
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.message.contains("`version` must be a version")),
                "{bad:?}: {issues:#?}"
            );
        }
    }

    #[test]
    fn manifest_values_must_be_strings() {
        let issues =
            parse_manifest("name = m\nversion = 1.0\ncheckmate_version = \"0.5.0\"\n").unwrap_err();
        // Two non-string values, plus the two missing-field reports that
        // follow because the values never landed.
        assert_eq!(
            issues
                .iter()
                .filter(|issue| issue
                    .message
                    .contains("needs a string value in double or single quotes"))
                .count(),
            2,
            "{issues:#?}"
        );

        // Multiline string forms are outside the subset, not a shorthand.
        let issues = parse_manifest(
            "name = \"\"\"m\"\"\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.5.0\"\n",
        )
        .unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("needs a string value")),
            "{issues:#?}"
        );
    }

    #[test]
    fn manifest_dotted_keys_are_outside_the_subset() {
        let issues = parse_manifest(
            "name = \"m\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.5.0\"\n[schemas]\nengine.graphics = \"1.0.0\"\n",
        )
        .unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("expected `=` after key `engine`")),
            "{issues:#?}"
        );
    }

    #[test]
    fn manifest_schema_namespaces_must_be_identifiers() {
        let issues = parse_manifest(
            "name = \"m\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.5.0\"\n[schemas]\n\"my engine\" = \"1.0.0\"\n",
        )
        .unwrap_err();
        assert!(
            issues.iter().any(|issue| issue
                .message
                .contains("schema namespace `my engine` is not a valid identifier")),
            "{issues:#?}"
        );
    }

    #[test]
    fn manifest_line_numbers_point_at_offenders() {
        let issues =
            parse_manifest("name = \"m\"\nversion = \"bad\"\ncheckmate_version = \"0.5.0\"\n")
                .unwrap_err();
        let version_issue = issues
            .iter()
            .find(|issue| issue.message.contains("`version`"))
            .unwrap();
        assert_eq!(version_issue.line, Some(2));
        assert_eq!(version_issue.file.as_deref(), Some("mod.toml"));
    }

    // -- discovery ----------------------------------------------------------

    /// A unique temporary directory removed on drop.
    struct TempMod {
        root: PathBuf,
    }

    impl TempMod {
        fn new() -> Self {
            static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let root =
                std::env::temp_dir().join(format!("cme_mods_test_{}_{}", std::process::id(), id));
            std::fs::create_dir_all(&root).expect("create temp mod root");
            Self { root }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).expect("create parent");
            std::fs::write(path, contents).expect("write file");
        }
    }

    impl Drop for TempMod {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn module_paths(modules: &[DiscoveredModule]) -> Vec<String> {
        modules
            .iter()
            .map(|module| module.module_path.join("."))
            .collect()
    }

    #[test]
    fn discovery_maps_the_src_tree_to_module_paths() {
        let temp = TempMod::new();
        temp.write("src/main.cm", "int main() {\n    return 0\n}\n");
        temp.write("src/gamemode/rules.cm", "int rules() {\n    return 1\n}\n");
        temp.write(
            "src/gamemode/events.cm",
            "int events() {\n    return 2\n}\n",
        );
        temp.write("src/ui/hud.cm", "int hud() {\n    return 3\n}\n");
        temp.write("src/notes.txt", "not a module");
        temp.write("src/.hidden.cm", "skipped");

        let modules = discover_modules(&temp.root).unwrap_or_else(|issues| panic!("{issues:#?}"));
        assert_eq!(
            module_paths(&modules),
            vec![
                "self.gamemode.events",
                "self.gamemode.rules",
                "self.main",
                "self.ui.hud",
            ],
            "modules are sorted by module path regardless of disk order"
        );
        let rules = modules
            .iter()
            .find(|module| module.display_path == "src/gamemode/rules.cm")
            .unwrap();
        assert_eq!(rules.module_path, vec!["self", "gamemode", "rules"]);
        assert!(rules.file_path.ends_with("src/gamemode/rules.cm"));
    }

    #[test]
    fn discovery_reports_a_missing_or_empty_src_tree() {
        let temp = TempMod::new();
        let issues = discover_modules(&temp.root).unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("missing a `src/` directory")),
            "{issues:#?}"
        );

        temp.write("mod.toml", "name = \"m\"");
        std::fs::create_dir_all(temp.root.join("src")).unwrap();
        let issues = discover_modules(&temp.root).unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("no `.cm` files found under `src/`")),
            "{issues:#?}"
        );
    }

    #[test]
    fn discovery_rejects_unmappable_names() {
        let temp = TempMod::new();
        temp.write("src/ok.cm", "int ok() {\n    return 0\n}\n");
        temp.write("src/my-file.cm", "int broken() {\n    return 1\n}\n");
        let issues = discover_modules(&temp.root).unwrap_err();
        assert!(
            issues.iter().any(|issue| issue
                .message
                .contains("`my-file` is not a valid identifier segment")),
            "{issues:#?}"
        );

        let temp = TempMod::new();
        temp.write("src/my stuff/x.cm", "int x() {\n    return 0\n}\n");
        let issues = discover_modules(&temp.root).unwrap_err();
        assert!(
            issues.iter().any(|issue| issue
                .message
                .contains("`my stuff` is not a valid identifier segment")),
            "{issues:#?}"
        );
    }

    #[test]
    fn load_mod_reads_manifest_and_sources() {
        let temp = TempMod::new();
        temp.write(
            "mod.toml",
            "name = \"sample\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.5.0\"\n\n[schemas]\nengine = \"1.4.0\"\n",
        );
        temp.write(
            "src/main.cm",
            "import self.util\nint main() {\n    return util()\n}\n",
        );
        temp.write("src/util.cm", "int util() {\n    return 7\n}\n");

        let loaded = load_mod(&temp.root);
        assert!(loaded.issues.is_empty(), "{:#?}", loaded.issues);
        let manifest = loaded.manifest.expect("manifest parses");
        assert_eq!(manifest.name, "sample");
        assert_eq!(
            manifest.schemas,
            vec![("engine".to_string(), "1.4.0".to_string())]
        );
        assert_eq!(loaded.modules.len(), 2);
        assert_eq!(loaded.modules[0].module_path, vec!["self", "main"]);
        assert_eq!(loaded.modules[1].module_path, vec!["self", "util"]);
    }

    #[test]
    fn load_mod_without_manifest_reports_and_still_discovers() {
        let temp = TempMod::new();
        temp.write("src/main.cm", "int main() {\n    return 0\n}\n");
        let loaded = load_mod(&temp.root);
        assert!(loaded.manifest.is_none());
        assert!(
            loaded
                .issues
                .iter()
                .any(|issue| issue.message.contains("no mod.toml")),
            "{:#?}",
            loaded.issues
        );
        assert_eq!(loaded.modules.len(), 1);
    }

    // -- assembly -----------------------------------------------------------

    fn module(name: &str, source: &str) -> LoadedModule {
        LoadedModule {
            module_path: vec!["self".to_string(), name.to_string()],
            display_path: format!("src/{name}.cm"),
            source: source.to_string(),
        }
    }

    #[test]
    fn assembles_modules_into_one_program_with_resolved_imports() {
        let modules = [
            module(
                "main",
                "import self.util\nint main() {\n    return util(3)\n}\n",
            ),
            module("util", "int util(int x) {\n    return x + 1\n}\n"),
        ];
        let program = assemble(&modules);
        assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
        // The self import resolves: util(3) links across the two files.
        assert_eq!(crate::check::check(&program.statements), Vec::new());
        assert_eq!(program.ranges.len(), 2);
        assert_eq!(program.ranges[1].start, program.ranges[0].end + 2);
    }

    #[test]
    fn import_of_a_missing_module_is_a_diagnostic_at_the_import_site() {
        let modules = [module(
            "main",
            "import self.gamemode.rules\nint main() {\n    return 0\n}\n",
        )];
        let program = assemble(&modules);
        assert_eq!(program.diagnostics.len(), 1);
        assert!(
            program.diagnostics[0]
                .message()
                .contains("import of `self.gamemode.rules` does not match any module"),
            "{:#?}",
            program.diagnostics
        );
        assert!(
            program.diagnostics[0]
                .message()
                .contains("expected a file at `src/gamemode/rules.cm`"),
            "{:#?}",
            program.diagnostics
        );
    }

    #[test]
    fn host_rooted_imports_are_accepted_without_schema_checks() {
        let modules = [module(
            "main",
            "import engine.graphics\nint main() {\n    return 0\n}\n",
        )];
        let program = assemble(&modules);
        assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
    }

    #[test]
    fn duplicate_names_across_files_are_one_diagnostic_in_the_union() {
        let modules = [
            module(
                "main",
                "int hp() {\n    return 1\n}\nint main() {\n    return 0\n}\n",
            ),
            module("extra", "int hp() {\n    return 2\n}\n"),
        ];
        let program = assemble(&modules);
        assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
        let errors = crate::check::check(&program.statements);
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert_eq!(errors[0].to_string(), "duplicate function `hp`");
        // The report lands in the SECOND file (the later registration).
        let (owner, _) = attribute_span(&program.ranges, errors[0].span()).unwrap();
        assert_eq!(owner.display_path, "src/extra.cm");
    }

    #[test]
    fn impl_blocks_union_across_files_per_10_4() {
        // Two files implement the same local target: §10.4's union. The
        // checker resolves the members and the interpreter calls them.
        let modules = [
            module(
                "main",
                "import self.rules\nstruct counter {\n    int value\n}\nimpl counter {\n    counter reset() {\n        return counter(value: 0)\n    }\n}\nint main() {\n    return counter.reset().value\n}\n",
            ),
            module(
                "rules",
                "impl counter {\n    int peek(counter c) {\n        return c.value\n    }\n}\n",
            ),
        ];
        let program = assemble(&modules);
        assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
        let errors = crate::check::check(&program.statements);
        assert!(errors.is_empty(), "{errors:#?}");

        // A member implemented twice across files is rejected (§10.4).
        let modules = [
            module(
                "main",
                "struct counter {\n    int value\n}\nimpl counter {\n    counter reset() {\n        return counter(value: 0)\n    }\n}\nint main() {\n    return 0\n}\n",
            ),
            module(
                "extra",
                "impl counter {\n    counter reset() {\n        return counter(value: 1)\n    }\n}\n",
            ),
        ];
        let program = assemble(&modules);
        assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
        let errors = crate::check::check(&program.statements);
        assert!(
            errors.iter().any(|error| error
                .to_string()
                .contains("duplicate impl member `counter.reset`")),
            "{errors:#?}"
        );
    }

    #[test]
    fn spans_attribute_back_to_their_own_module() {
        let modules = [
            module("main", "int main() {\n    return 0\n}\n"),
            module("broken", "int f() {\n    return 1 +\n}\n"),
        ];
        let program = assemble(&modules);
        assert_eq!(program.diagnostics.len(), 2);
        // Every diagnostic lands in the broken module, in that module's own
        // coordinates; the first one points at the stray `}`.
        for diagnostic in &program.diagnostics {
            let (owner, local) = attribute_span(&program.ranges, diagnostic.span()).unwrap();
            assert_eq!(owner.display_path, "src/broken.cm");
            assert!(local.end <= owner.end - owner.start);
        }
        let (_, local) = attribute_span(&program.ranges, program.diagnostics[0].span()).unwrap();
        let text = &modules[1].source[local.start..local.end];
        assert_eq!(text, "}");
    }

    #[test]
    fn standalone_self_imports_are_rejected() {
        let outcome = crate::parse_source("import self.util\nint main() {\n    return 0\n}\n");
        let diagnostics = standalone_import_diagnostics(&outcome.statements);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .message()
                .contains("cannot import `self.util` outside of a mod"),
            "{:#?}",
            diagnostics
        );

        let outcome =
            crate::parse_source("import engine.graphics\nint main() {\n    return 0\n}\n");
        assert!(standalone_import_diagnostics(&outcome.statements).is_empty());
    }
}
