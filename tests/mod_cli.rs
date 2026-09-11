#![cfg(feature = "cli")]
//! End-to-end CLI tests for multi-file mods (WHITEPAPER §10): the real
//! `cme` binary against real mod directories. Each test builds a throwaway
//! mod tree in the system temp directory, runs one command, and pins both
//! the exit code and the exact file-anchored reporting shape — a mod
//! diagnostic must name the MODULE it came from
//! (`my_mod/src/gamemode/rules.cm:3:1`), never a raw offset into the
//! assembled program text.

use std::path::PathBuf;
use std::process::Command;

/// A unique temporary mod directory, removed on drop.
struct TempMod {
    root: PathBuf,
}

impl TempMod {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "cme_mod_cli_test_{}_{}_{}",
            std::process::id(),
            tag,
            id
        ));
        std::fs::create_dir_all(&root).expect("create temp mod root");
        Self { root }
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create parent");
        std::fs::write(path, contents).expect("write file");
    }

    fn path(&self) -> &str {
        self.root.to_str().expect("temp path is utf-8")
    }
}

impl Drop for TempMod {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn cme(args: &[&str]) -> (String, String, Option<i32>) {
    let output = Command::new(env!("CARGO_BIN_EXE_cme"))
        .args(args)
        .output()
        .expect("spawn the cme binary");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

const MANIFEST: &str = "name = \"cli_mod\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.5.0\"\n\n[schemas]\nengine = \"1.4.0\"\n";

/// A two-module mod: main imports and calls `double` from util.cm.
fn two_module_mod(tag: &str) -> TempMod {
    let temp = TempMod::new(tag);
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        "import self.util\nint main() {\n    return double(21) + 10\n}\n",
    );
    temp.write("src/util.cm", "int double(int x) {\n    return x + x\n}\n");
    temp
}

#[test]
fn mod_runs_end_to_end_across_files() {
    let temp = two_module_mod("run");
    let (stdout, stderr, code) = cme(&["run", temp.path()]);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "52\n");
}

#[test]
fn mod_runs_via_the_manifest_path_too() {
    let temp = two_module_mod("manifest_path");
    let mut argument = temp.root.clone();
    argument.push("mod.toml");
    let (stdout, stderr, code) = cme(&["run", argument.to_str().unwrap()]);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "52\n");
}

#[test]
fn clean_mod_checks_silently() {
    let temp = two_module_mod("check");
    let (stdout, stderr, code) = cme(&["check", temp.path()]);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert!(stdout.is_empty());
}

#[test]
fn host_rooted_imports_run_without_a_schema() {
    // §9 schema gating is future work; a host import is accepted.
    let temp = TempMod::new("host_import");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        "import engine.graphics\nint main() {\n    return 5\n}\n",
    );
    let (stdout, stderr, code) = cme(&["run", temp.path()]);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "5\n");
}

#[test]
fn import_of_a_missing_module_names_the_file_and_line() {
    let temp = two_module_mod("bad_import");
    temp.write(
        "src/extra.cm",
        "import self.nope\nint extra() {\n    return 1\n}\n",
    );
    let (_, stderr, code) = cme(&["check", temp.path()]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains(&format!(
            "{}/src/extra.cm:1:1: import of `self.nope` does not match any module",
            temp.path()
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains("expected a file at `src/nope.cm`"),
        "{stderr}"
    );
    // The caret sits under the import statement of its own file.
    assert!(
        stderr.contains("import self.nope\n^^^^^^^^^^^^^^^^"),
        "{stderr}"
    );
}

#[test]
fn manifest_defects_report_the_mod_toml_line() {
    let temp = TempMod::new("bad_manifest");
    temp.write(
        "mod.toml",
        "name = \"cli_mod\"\nversion = \"bad\"\ncheckmate_version = \"0.5.0\"\n",
    );
    temp.write("src/main.cm", "int main() {\n    return 0\n}\n");
    let (_, stderr, code) = cme(&["check", temp.path()]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains(&format!(
            "{}/mod.toml:2: `version` must be a version",
            temp.path()
        )),
        "{stderr}"
    );
}

#[test]
fn duplicate_names_across_files_report_in_the_second_file() {
    let temp = two_module_mod("duplicate");
    // Same name declared in ANOTHER module: the union collides.
    temp.write("src/other.cm", "int double(int x) {\n    return 0\n}\n");
    let (_, stderr, code) = cme(&["check", temp.path()]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("duplicate function `double`"), "{stderr}");
    // The later registration is the one reported, and it is attributed to
    // its own file. Modules link in sorted module-path order, so
    // `src/util.cm` registers after `src/other.cm` and is the collision.
    assert!(
        stderr.contains(&format!("{}/src/util.cm:1:1:", temp.path())),
        "{stderr}"
    );
}

#[test]
fn impl_members_link_and_fail_across_files() {
    // §10.4: the impl block for one target is split across two files;
    // main calls the member through the qualified path.
    let temp = TempMod::new("impl_union");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/main.cm",
        "import self.rules\nint main() {\n    return engine.rules.first()\n}\n",
    );
    temp.write(
        "src/rules.cm",
        "impl engine.rules {\n    int first() {\n        return 11\n    }\n}\n",
    );
    let (stdout, stderr, code) = cme(&["run", temp.path()]);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "11\n");

    // A member implemented twice across files fails the build (§10.4).
    temp.write(
        "src/other.cm",
        "impl engine.rules {\n    int first() {\n        return 12\n    }\n}\n",
    );
    let (_, stderr, code) = cme(&["check", temp.path()]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("duplicate impl member `engine.rules.first`"),
        "{stderr}"
    );
}

#[test]
fn runtime_errors_point_at_the_offending_module() {
    let temp = two_module_mod("runtime");
    temp.write(
        "src/boom.cm",
        "void boom() {\n    int x = 1\n    int y = x / (x - x)\n    return\n}\n",
    );
    temp.write(
        "src/main.cm",
        "import self.boom\nint main() {\n    boom()\n    return 0\n}\n",
    );
    let (_, stderr, code) = cme(&["run", temp.path()]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains(&format!(
            "{}/src/boom.cm:3:13: integer division by zero",
            temp.path()
        )),
        "{stderr}"
    );
}

#[test]
fn megaprogram_modules_expand_before_linking() {
    // §8 expansion stays per file; the generated function links into the
    // union and is callable from another module.
    let temp = TempMod::new("megamod");
    temp.write("mod.toml", MANIFEST);
    temp.write(
        "src/gen.cm",
        "magic stepper($word name \"(\" $int from \"..\" $int to \")\") {\n    int $name(int current) {\n        if (current >= $to) {\n            return $from\n        }\n        return current + 1\n    }\n}\n\nmagic(stepper) { page ( 0 .. 10 ) }\n",
    );
    temp.write(
        "src/main.cm",
        "import self.gen\nint main() {\n    return page(9)\n}\n",
    );
    let (stdout, stderr, code) = cme(&["run", temp.path()]);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "10\n");
}

#[test]
fn mod_without_main_reports_the_convention() {
    let temp = TempMod::new("no_main");
    temp.write("mod.toml", MANIFEST);
    temp.write("src/helper.cm", "int five() {\n    return 5\n}\n");
    let (_, stderr, code) = cme(&["run", temp.path()]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains(&format!("no `main` function to run in mod {}", temp.path())),
        "{stderr}"
    );
}

#[test]
fn single_file_self_imports_are_rejected() {
    // Outside a mod there is no tree to resolve against (§10.1).
    let temp = TempMod::new("standalone");
    let mut file = temp.root.clone();
    file.push("solo.cm");
    std::fs::write(&file, "import self.util\nint main() {\n    return 0\n}\n")
        .expect("write standalone file");
    let (_, stderr, code) = cme(&["check", file.to_str().unwrap()]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("cannot import `self.util` outside of a mod"),
        "{stderr}"
    );
}

#[test]
fn lex_and_expand_reject_mod_directories() {
    let temp = two_module_mod("lex");
    for command in ["lex", "expand"] {
        let (_, stderr, code) = cme(&[command, temp.path()]);
        assert_eq!(code, Some(1), "{command}");
        assert!(
            stderr.contains("works on a single .cm file"),
            "{command}: {stderr}"
        );
    }
}

#[test]
fn a_directory_without_mod_toml_is_not_a_mod() {
    let temp = TempMod::new("no_manifest");
    temp.write("src/main.cm", "int main() {\n    return 0\n}\n");
    let (_, stderr, code) = cme(&["run", temp.path()]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("no mod.toml"), "{stderr}");
}
