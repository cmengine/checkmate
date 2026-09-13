//! The Checkmate extension for Zed.
//!
//! Grammar, queries, and language metadata are declarative (see
//! `languages/checkmate/`). The only Rust code here launches the `cme`
//! language server: the same single binary that carries the CLI toolchain,
//! serving LSP over stdio via `cme lsp`.
//!
//! Binary resolution, in order:
//!
//! 1. a user override — `lsp.cme.binary.path` (plus `arguments`/`env`) in
//!    Zed settings;
//! 2. a `cme` binary on `$PATH` — the normal case after
//!    `cargo install --path . --features cli` or a distro package;
//! 3. an in-repository build — when the opened worktree IS the Checkmate
//!    repository and `cargo` has produced `target/`, the freshly built
//!    `target/debug/cme` is used so LSP changes can be tested against the
//!    source tree.

use zed_extension_api::{
    self as zed, settings::LspSettings, Command, LanguageServerId, Result, Worktree,
};

/// The language server id declared in `extension.toml`
/// (`[language_servers.cme]`).
const LANGUAGE_SERVER_ID: &str = "cme";

struct CheckmateExtension;

impl zed::Extension for CheckmateExtension {
    fn new() -> Self {
        CheckmateExtension
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        // 1. Explicit user override.
        if let Ok(settings) = LspSettings::for_worktree(language_server_id.as_ref(), worktree) {
            if let Some(binary) = settings.binary {
                let path = binary
                    .path
                    .ok_or_else(|| "`binary.path` is empty in the lsp.cme settings".to_string())?;
                return Ok(Command {
                    command: path,
                    args: binary.arguments.unwrap_or_default(),
                    env: binary.env.unwrap_or_default().into_iter().collect(),
                });
            }
        }

        // 2. A `cme` on the user's $PATH.
        if let Some(path) = worktree.which("cme") {
            return Ok(Command {
                command: path,
                args: vec!["lsp".to_string()],
                env: vec![],
            });
        }

        // 3. The dev workflow: the worktree is the Checkmate repository and
        //    a cargo build has already produced a target directory. The
        //    debug build is the default artifact of `cargo build --features
        //    cli`; release users can point at their binary through
        //    `lsp.cme.binary.path`.
        if is_checkmate_repository(worktree)
            && worktree.read_text_file("target/CACHEDIR.TAG").is_ok()
        {
            let root = worktree.root_path();
            return Ok(Command {
                command: format!("{root}/target/debug/cme"),
                args: vec!["lsp".to_string()],
                env: vec![],
            });
        }

        Err(format!(
            "no `cme` binary found for the Checkmate language server. \
             Install or build it with `cargo install --path . --features cli` \
             (from the checkmate repository), put `cme` on your $PATH, or set \
             `\"lsp\": {{ \"{LANGUAGE_SERVER_ID}\": {{ \"binary\": {{ \"path\": \"...\" }} }} }}` \
             in your Zed settings."
        ))
    }
}

/// Whether the worktree is the Checkmate compiler repository itself (as
/// opposed to a project that merely uses the language): its root `Cargo.toml`
/// declares the workspace package `cme`.
fn is_checkmate_repository(worktree: &Worktree) -> bool {
    worktree
        .read_text_file("Cargo.toml")
        .map(|manifest| {
            manifest
                .lines()
                .any(|line| line.trim_start().starts_with("name = \"cme\""))
        })
        .unwrap_or(false)
}

zed::register_extension!(CheckmateExtension);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_detection_matches_the_workspace_manifest() {
        let manifest = "[workspace]\nmembers = [\"crates/*\"]\n\n[package]\nname = \"cme\"\n";
        let found = manifest
            .lines()
            .any(|line| line.trim_start().starts_with("name = \"cme\""));
        assert!(found);
        let other = "[package]\nname = \"other-crate\"\n";
        assert!(!other
            .lines()
            .any(|line| line.trim_start().starts_with("name = \"cme\"")));
    }
}
