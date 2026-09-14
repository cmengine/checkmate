//! Mod and schema auto-detection: the layer between the editor and the
//! §10/§9 file layout.
//!
//! The CLI takes schemas explicitly (`--schema`) and mods by path; the
//! language server has neither, so it derives them from the file being
//! edited:
//!
//! 1. **Mod root** — walking up from the document's directory to the
//!    nearest `mod.toml` (§10.1). Files under that root belong to the mod.
//! 2. **Schemas** — `.cm` schema files under the mod root (the `src/`
//!    module tree excluded: those are scripts) and under the parent
//!    directory's `schemas/` tree — the layout this repository's host
//!    fixtures use (`host_api_test/Rust/schemas/shop.cm` next to
//!    `host_api_test/Rust/shop_mod/`). Every candidate parses through the
//!    real §9 front end; files with diagnostics are skipped (their own
//!    buffer reports the defects; a broken schema must not cascade
//!    misleading errors into scripts).
//! 3. **Grant** — the manifest's `[schemas]` table narrows the set exactly
//!    like a CLI mod run (§9.5, §10.2); a manifest without `[schemas]`
//!    grants every discovered namespace at its own version, which keeps
//!    the editor helpful for loose host-style projects.
//!
//! Results are cached per mod root and revalidated through mtimes plus the
//! open-buffer texts, so editing a schema buffer re-derives the context
//! the editor hands to scripts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cme_compiler::mods;
use cme_compiler::schema::{SchemaContext, SchemaFile, SchemaSet, parse_schema_file};

use crate::db;

/// How deep the schema search descends below a root (editors should never
/// need more, and runaway symlink trees must not hang the server).
const MAX_WALK_DEPTH: usize = 6;

/// One mod's resolved schema surface plus the freshness inputs it was
/// built from.
struct CachedMod {
    context: Option<Arc<SchemaContext>>,
    /// File mtimes at build time (manifest, schema candidates, modules).
    inputs: BTreeMap<PathBuf, SystemTime>,
    /// Open-buffer texts at build time (they override disk for the files
    /// they cover, so they are part of the freshness key).
    buffers: BTreeMap<PathBuf, String>,
}

/// The per-session discovery cache.
#[derive(Default)]
pub struct Workspace {
    mods: BTreeMap<PathBuf, CachedMod>,
    /// The last version of each schema file that PARSED CLEANLY. While a
    /// schema buffer is mid-edit (or a save is broken), the mod's scripts
    /// keep checking against the last valid contract instead of drowning
    /// in "unknown function" cascades; the schema buffer itself reports
    /// its own defects.
    last_good: BTreeMap<PathBuf, SchemaFile>,
}

impl Workspace {
    pub fn new() -> Workspace {
        Workspace::default()
    }

    /// The §9 schema contract active for a document at `path`, or `None`
    /// when the file has no mod root or the mod ships no usable schemas.
    /// `open_text` reports the editor buffer for open documents (their
    /// unsaved state counts); disk is the fallback.
    pub fn schema_context(
        &mut self,
        path: &Path,
        open_text: &dyn Fn(&Path) -> Option<String>,
    ) -> Option<Arc<SchemaContext>> {
        let mod_root = find_mod_root(path)?;
        let inputs = self.freshness_inputs(&mod_root, open_text);
        if let Some(cached) = self.mods.get(&mod_root)
            && cached.inputs == inputs.paths
            && cached.buffers == inputs.buffers
        {
            return cached.context.clone();
        }
        let context = self.build_schema_context(&mod_root, &inputs, open_text);
        self.mods.insert(
            mod_root,
            CachedMod {
                context: context.clone(),
                inputs: inputs.paths,
                buffers: inputs.buffers,
            },
        );
        context
    }

    /// Builds the schema context for a mod: parse every candidate (open
    /// buffers first, disk as fallback), keep every clean parse, and grant
    /// through the manifest's `[schemas]` table (§9.5). A file with parse
    /// diagnostics falls back to its last clean parse, so a mod's view of
    /// the contract stays stable while the schema buffer is mid-edit. A
    /// set that fails its cross-file invariants retries without the
    /// implicated namespaces, so one defective schema file does not darken
    /// every script in the mod.
    fn build_schema_context(
        &mut self,
        mod_root: &Path,
        inputs: &Inputs,
        open_text: &dyn Fn(&Path) -> Option<String>,
    ) -> Option<Arc<SchemaContext>> {
        let mut files: Vec<SchemaFile> = Vec::new();
        for path in schema_candidates(mod_root) {
            // A candidate that stopped being a schema file (or stopped
            // existing) between discovery and now is skipped quietly.
            let Some(text) = text_for(&path, inputs, open_text) else {
                continue;
            };
            if db::sniff_kind(&text) != db::FileKind::Schema {
                continue;
            }
            let outcome = parse_schema_file(&text);
            if outcome.is_clean()
                && let Some(file) = outcome.file
            {
                self.last_good.insert(path.clone(), file.clone());
                files.push(file);
                continue;
            }
            // Defective: the last clean parse of this file keeps the
            // mod's view of the contract stable while it is being fixed.
            if let Some(file) = self.last_good.get(&path) {
                files.push(file.clone());
            }
        }
        if files.is_empty() {
            return None;
        }

        let targets = manifest_targets(mod_root);
        let mut remaining = files;
        loop {
            match SchemaSet::build(remaining.clone()) {
                Ok(set) => {
                    let context = if targets.is_empty() {
                        SchemaContext::grant_all(set)
                    } else {
                        // Manifest entries the discovery never found a file
                        // for are skipped: the editor cannot check against
                        // a schema it does not have, and a missing file is
                        // better reported where it is imported than as a
                        // grant error.
                        let usable: Vec<(String, String)> = targets
                            .iter()
                            .filter(|(namespace, _)| set.namespace(namespace).is_some())
                            .cloned()
                            .collect();
                        SchemaContext::grant_targets(set, usable).ok()?
                    };
                    return if context.set.is_empty() {
                        None
                    } else {
                        Some(Arc::new(context))
                    };
                }
                Err(issues) => {
                    // Drop the first implicated namespace and retry; the
                    // set strictly shrinks, so this terminates.
                    let drop = issues.iter().find_map(|issue| issue.namespace.clone())?;
                    remaining.retain(|file| file.namespace != drop);
                    if remaining.is_empty() {
                        return None;
                    }
                }
            }
        }
    }

    /// The mod root a document belongs to, if any.
    pub fn mod_root(&self, path: &Path) -> Option<PathBuf> {
        find_mod_root(path)
    }

    /// The dotted module path (§10.3) of a document inside its mod, when
    /// the document is one of the mod's `src/**.cm` modules.
    pub fn module_path(&self, mod_root: &Path, path: &Path) -> Option<Vec<String>> {
        let modules = discover_quiet(mod_root)?;
        let canonical = std::fs::canonicalize(path).ok()?;
        modules
            .into_iter()
            .find(|module| std::fs::canonicalize(&module.file_path).is_ok_and(|p| p == canonical))
            .map(|module| module.module_path)
    }

    /// The dotted module paths (§10.3) of a mod's `src/**.cm` tree, for
    /// `import self.*` completion. Empty when there is no mod root.
    pub fn module_table(&self, mod_root: &Path) -> Vec<Vec<String>> {
        discover_quiet(mod_root)
            .map(|modules| {
                modules
                    .into_iter()
                    .map(|module| module.module_path)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The freshness fingerprint of one mod's schema surface: the manifest
    /// plus the schema candidates, each to its mtime, plus the open-buffer
    /// texts that override them. DISCOVERY runs on every check (cheap
    /// directory reads); file CONTENTS are only read when the fingerprint
    /// changed. Module sources are deliberately absent — scripts do not
    /// affect the schema contract.
    fn freshness_inputs(
        &self,
        mod_root: &Path,
        open_text: &dyn Fn(&Path) -> Option<String>,
    ) -> Inputs {
        let mut paths = BTreeMap::new();
        let mut buffers = BTreeMap::new();
        let manifest = mod_root.join("mod.toml");
        for path in std::iter::once(manifest).chain(schema_candidates(mod_root)) {
            let Some(mtime) = mtime_of(&path) else {
                continue;
            };
            if let Some(text) = open_text(&path) {
                buffers.insert(path.clone(), text);
            }
            paths.insert(path, mtime);
        }
        Inputs { paths, buffers }
    }
}

struct Inputs {
    paths: BTreeMap<PathBuf, SystemTime>,
    buffers: BTreeMap<PathBuf, String>,
}

/// Walks up from `file`'s directory to the nearest `mod.toml` (§10.1).
pub fn find_mod_root(file: &Path) -> Option<PathBuf> {
    let mut dir = file.parent()?.to_path_buf();
    loop {
        if dir.join("mod.toml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Candidate schema files for a mod: `.cm` files under the mod root
/// (excluding the `src/` module tree) plus the parent directory's
/// `schemas/` tree. Sorted, so builds are deterministic.
fn schema_candidates(mod_root: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    collect_cm_files(mod_root, Some(&mod_root.join("src")), &mut candidates, 0);
    if let Some(parent) = mod_root.parent() {
        collect_cm_files(&parent.join("schemas"), None, &mut candidates, 0);
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn discover_quiet(mod_root: &Path) -> Option<Vec<mods::DiscoveredModule>> {
    mods::discover_modules(mod_root).ok()
}

/// Recursively collects `.cm` files below `dir`, skipping dot entries and
/// `skip` when reached. Symlink loops are bounded by the depth cap.
fn collect_cm_files(dir: &Path, skip: Option<&Path>, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<std::fs::DirEntry> = entries.filter_map(|entry| entry.ok()).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if skip.is_some_and(|skip| path == skip) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_cm_files(&path, skip, out, depth + 1);
        } else if path.extension().is_some_and(|ext| ext == "cm") {
            out.push(path);
        }
    }
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// The manifest's `[schemas]` table (§10.2), or empty when there is no
/// readable manifest: a manifest without the table grants every discovered
/// namespace (the loose-source default), and an unreadable manifest falls
/// back to the same.
fn manifest_targets(mod_root: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(mod_root.join("mod.toml"))
        .ok()
        .and_then(|text| mods::parse_manifest(&text).ok())
        .map(|manifest| manifest.schemas)
        .unwrap_or_default()
}

/// The text for a candidate: the editor buffer when the file is open, disk
/// otherwise.
fn text_for(
    path: &Path,
    inputs: &Inputs,
    open_text: &dyn Fn(&Path) -> Option<String>,
) -> Option<String> {
    if let Some(text) = inputs.buffers.get(path) {
        return Some(text.clone());
    }
    if let Some(text) = open_text(path) {
        return Some(text);
    }
    std::fs::read_to_string(path).ok()
}

/// Loads a mod's modules with open-buffer texts overriding disk (§10).
/// Structural defects (broken manifest, unreadable files) return `None`:
/// the CLI reports them with paths; the editor stays quiet rather than
/// mis-anchoring them into an unrelated buffer. Expansion stays the
/// caller's job.
pub fn load_mod_with_buffers(
    mod_root: &Path,
    open_text: &dyn Fn(&Path) -> Option<String>,
) -> Option<mods::LoadedMod> {
    let mut loaded = mods::load_mod(mod_root);
    if !loaded.issues.is_empty() {
        return None;
    }
    for module in &mut loaded.modules {
        if let Some(text) = open_text(&module_file_path(mod_root, &module.module_path)) {
            module.source = text;
        }
    }
    Some(loaded)
}

/// The on-disk path of a loaded module: `src/` plus the module path's
/// segments after the `self` root (§10.3).
fn module_file_path(mod_root: &Path, module_path: &[String]) -> PathBuf {
    let mut path = mod_root.join("src");
    for segment in module_path.iter().skip(1) {
        path.push(segment);
    }
    path.set_extension("cm");
    path
}

// ---------------------------------------------------------------------------
// Mod programs — the §10 assembly the editor checks documents against
// ---------------------------------------------------------------------------

/// The compiled state of one mod: the assembled virtual program (§10.3
/// static linking) plus the checker's verdict, from which per-module
/// diagnostics are re-anchored (§10, `mods::attribute_span`).
pub struct ModPlan {
    pub program: mods::AssembledProgram,
    /// Checker diagnostics in virtual-text coordinates.
    pub check_diagnostics: Vec<cme_compiler::diagnostics::Diagnostic>,
    /// Display paths of modules whose ORIGINAL text mentions megaprograms.
    /// Their parse/check spans live in expanded-text coordinates (the
    /// anchoring rule in [`crate::db`]), so only their expansion
    /// diagnostics publish — anchored in the original text.
    pub mega_modules: BTreeMap<String, Vec<cme_compiler::diagnostics::Diagnostic>>,
}

impl ModPlan {
    /// Runs the CLI's mod pipeline over the current buffer state: load
    /// (open buffers override disk), expand megaprograms per module,
    /// assemble, check with the given schema context.
    pub fn build(
        mod_root: &Path,
        schema: Option<&SchemaContext>,
        open_text: &dyn Fn(&Path) -> Option<String>,
    ) -> Option<ModPlan> {
        let loaded = load_mod_with_buffers(mod_root, open_text)?;
        let mut modules = loaded.modules;
        let mut mega_modules = BTreeMap::new();
        for module in &mut modules {
            if !cme_compiler::mega::expand::mentions_megaprogram(&module.source) {
                continue;
            }
            let source = module.source.clone();
            match cme_compiler::mega::expand::expand_source(&source) {
                Ok(outcome) => module.source = outcome.expanded,
                Err(diagnostics) => {
                    // The expansion failed: its diagnostics anchor in the
                    // module's own text, which is what the editor shows.
                    mega_modules.insert(module.display_path.clone(), diagnostics);
                }
            }
        }
        let program = mods::assemble(&modules);
        let check_diagnostics = match schema {
            Some(context) => {
                cme_compiler::check::check_with_schema(&program.statements, Some(context))
            }
            None => cme_compiler::check::check(&program.statements),
        };
        Some(ModPlan {
            program,
            check_diagnostics,
            mega_modules,
        })
    }

    /// The diagnostics to publish for the module at `display_path`, in
    /// that module's own text coordinates (what the editor buffer shows).
    /// Empty for display paths that are not modules of this mod.
    pub fn diagnostics_for(
        &self,
        display_path: &str,
    ) -> Vec<cme_compiler::diagnostics::Diagnostic> {
        if let Some(expansion) = self.mega_modules.get(display_path) {
            return expansion.clone();
        }
        if self
            .program
            .ranges
            .iter()
            .find(|candidate| candidate.display_path == display_path)
            .is_none()
        {
            return Vec::new();
        }
        let mut anchored = Vec::new();
        let all = self
            .program
            .diagnostics
            .iter()
            .chain(self.check_diagnostics.iter());
        for diagnostic in all {
            let Some((owner, span)) = mods::attribute_span(&self.program.ranges, diagnostic.span())
            else {
                continue;
            };
            if owner.display_path == display_path {
                anchored.push(diagnostic.anchored_at(span));
            }
        }
        anchored
    }

    /// The diagnostics to publish for the module with the dotted module
    /// path `module_path` (§10.3), re-anchored into its own text
    /// coordinates. Empty when the mod does not contain it.
    pub fn diagnostics_for_module(
        &self,
        module_path: &[String],
    ) -> Vec<cme_compiler::diagnostics::Diagnostic> {
        let Some(range) = self
            .program
            .ranges
            .iter()
            .find(|range| range.module_path == module_path)
        else {
            return Vec::new();
        };
        self.diagnostics_for(&range.display_path)
    }
}
