#![cfg(feature = "cli")]
//! End-to-end CLI tests for the §9 schema system: the real `cme` binary
//! over real schema files and programs — `schema`, `codegen-c`, and the
//! `--schema` flag on check/run.

use std::path::PathBuf;
use std::process::Command;

/// A unique temporary file, removed on drop.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn new(tag: &str, contents: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "cme_schema_cli_test_{}_{}_{}.cm",
            std::process::id(),
            tag,
            id
        ));
        std::fs::write(&path, contents).expect("write temp file");
        Self { path }
    }

    fn path(&self) -> &str {
        self.path.to_str().expect("temp path is utf-8")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A schema fixture exercising the shapes codegen-c must render.
const SCHEMA_FIXTURE: &str = "schema det 1.0.0\n\
struct Handle {\n\
int id\n\
}\n\
struct Item {\n\
str name\n\
Handle inner\n\
}\n\
enum Kind {\n\
Off\n\
Loaded(int amount)\n\
}\n\
capability cap {\n\
since 1.0.0 Item Make(str label)\n\
}\n\
interface iface {\n\
since 1.0.0 bool Ready()\n\
}\n";

fn cme(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_cme"))
        .args(args)
        .output()
        .expect("the cme binary runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

const GOOD_SCHEMA: &str = "
schema engine 1.4.0

struct TextureHandle {
    int id
}

struct GameState {
    int score
    bool active
}

capability graphics {
    since 1.0.0 TextureHandle LoadTexture(str path)
    since 1.2.0 int DrawSprite(TextureHandle tex, int frame)
}

interface gamemode {
    since 1.0.0 GameState InitGame()
}
";

const PROGRAM_USING_GRAPHICS: &str = "
import engine.graphics

int main() {
    TextureHandle tex = engine.graphics.LoadTexture(\"hero.png\")
    return tex.id
}
";

#[test]
fn schema_validates_a_clean_file_silently() {
    let schema = TempFile::new("good_schema", GOOD_SCHEMA);
    let (code, stdout, stderr) = cme(&["schema", schema.path()]);
    assert_eq!(code, 0, "stdout: {stdout}, stderr: {stderr}");
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn schema_reports_defects_with_positions() {
    let schema = TempFile::new(
        "bad_schema",
        "schema engine 1.0.0\ncapability graphics {\nint noParens\n}\n",
    );
    let (code, _stdout, stderr) = cme(&["schema", schema.path()]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("PascalCase") && stderr.contains(schema.path()),
        "expected a positioned PascalCase diagnostic, got: {stderr}"
    );

    // An unresolved `requires` fails the set invariants.
    let schema = TempFile::new(
        "requires_schema",
        "schema app 1.0.0\ncapability net requires missing { int Send() }\n",
    );
    let (code, _stdout, stderr) = cme(&["schema", schema.path()]);
    assert_ne!(code, 0);
    assert!(stderr.contains("requires"), "got: {stderr}");
}

#[test]
fn codegen_c_prints_a_header() {
    let schema = TempFile::new("gen_schema", GOOD_SCHEMA);
    let (code, stdout, stderr) = cme(&["codegen-c", schema.path()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("#ifndef CME_SCHEMA_GEN_ENGINE_H"));
    assert!(stdout.contains("cme_engine_graphics_LoadTexture_fn"));
    assert!(stdout.contains("CME_ENGINE_GRAPHICS_REGISTER"));
}

#[test]
fn codegen_c_output_is_byte_deterministic() {
    // The schema file is the contract: two runs over the same file must
    // produce byte-identical headers, so a host can commit one and diff it.
    let schema = TempFile::new("det.cm", SCHEMA_FIXTURE);
    let (code1, out1, _) = cme(&["codegen-c", schema.path()]);
    let (code2, out2, _) = cme(&["codegen-c", schema.path()]);
    assert_eq!((code1, code2), (0, 0), "codegen-c succeeds");
    assert_eq!(out1, out2, "codegen-c must be deterministic");
    assert!(
        out1.contains("CME_SCHEMA_DET_VERSION"),
        "the header metadata renders"
    );
    assert!(out1.contains("char* name;"), "str fields render");
    assert!(
        out1.contains("strcmp(variant_name, \"Loaded\")"),
        "enum unpack renders"
    );
}

#[test]
fn check_with_schema_gates_capability_calls() {
    let schema = TempFile::new("gate_schema", GOOD_SCHEMA);
    let program = TempFile::new("gate_program", PROGRAM_USING_GRAPHICS);

    // Without --schema the contract is not active: `TextureHandle` is an
    // unknown type (schema types exist only through the §9.3 registry).
    let (code, _stdout, stderr) = cme(&["check", program.path()]);
    assert_ne!(code, 0, "the contract-less check must fail: {stderr}");
    assert!(stderr.contains("unknown type `TextureHandle`"));

    // With --schema: the program satisfies the contract.
    let (code, _stdout, stderr) = cme(&["check", "--schema", schema.path(), program.path()]);
    assert_eq!(code, 0, "stderr: {stderr}");

    // A program violating the contract fails with a schema diagnostic.
    let bad = TempFile::new(
        "bad_program",
        "import engine.graphics\nint main() {\nengine.graphics.LoadTexture(42)\nreturn 0\n}\n",
    );
    let (code, _stdout, stderr) = cme(&["check", "--schema", schema.path(), bad.path()]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("LoadTexture") && stderr.contains("`int`"),
        "expected a parameter-type diagnostic, got: {stderr}"
    );

    // Version gating: target 1.0.0 hides the since-1.2.0 member.
    let old_schema = TempFile::new(
        "old_schema",
        &GOOD_SCHEMA.replace("schema engine 1.4.0", "schema engine 1.0.0"),
    );
    let newer = TempFile::new(
        "newer_program",
        "import engine.graphics\nint main() {\nTextureHandle tex = engine.graphics.LoadTexture(\"x\")\nint n = engine.graphics.DrawSprite(tex, 1)\nreturn n\n}\n",
    );
    let (code, _stdout, stderr) = cme(&["check", "--schema", old_schema.path(), newer.path()]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("1.2.0") && stderr.contains("1.0.0"),
        "expected a version-gating diagnostic, got: {stderr}"
    );
}

#[test]
fn run_with_schema_constructs_schema_types() {
    let schema = TempFile::new("run_schema", GOOD_SCHEMA);
    let program = TempFile::new(
        "run_program",
        "import engine.graphics\nint main() {\nTextureHandle tex = engine.graphics.LoadTexture(\"hero.png\")\nreturn tex.id\n}\n",
    );
    // Without the contract, `TextureHandle` is an unknown type: this
    // program only compiles against the registered schema.
    let (code, _stdout, stderr) = cme(&["check", program.path()]);
    assert_ne!(
        code, 0,
        "the un-schema'd program must not compile: {stderr}"
    );

    let (code, _stdout, stderr) = cme(&["run", "--schema", schema.path(), program.path()]);
    assert_ne!(code, 0, "capability calls need a provider: {stderr}");
    assert!(
        stderr.contains("engine.graphics"),
        "expected a capability diagnostic, got: {stderr}"
    );
    let _ = code;

    // A program that only implements the interface runs clean.
    let impl_program = TempFile::new(
        "impl_program",
        "impl engine.gamemode {\nGameState InitGame() {\nreturn GameState(score: 3, active: true)\n}\n}\nint main() {\nreturn 7\n}\n",
    );
    let (code, stdout, stderr) = cme(&["run", "--schema", schema.path(), impl_program.path()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout.trim(), "7");
}

#[test]
fn diagnostics_name_the_script_even_when_schema_precedes_it() {
    // The display path is the file argument, resolved after `--schema`
    // pairs are consumed — not raw args().nth(2), which is the literal
    // `--schema` whenever the flag precedes the path.
    let schema = TempFile::new("prefix_schema", GOOD_SCHEMA);
    let program = TempFile::new(
        "prefix_program",
        "import engine.graphics\nint main() {\nint broken = engine.graphics.LoadTexture(42)\nreturn 0\n}\n",
    );

    let (code, _stdout, stderr) = cme(&["check", "--schema", schema.path(), program.path()]);
    assert_ne!(code, 0);
    let program_name = program
        .path()
        .rsplit('/')
        .next()
        .expect("a temp path with a file name");
    assert!(
        stderr.contains(program_name),
        "the diagnostic names the script file: {stderr}"
    );
    assert!(
        !stderr.contains("--schema:"),
        "no diagnostic may render the flag as the file name: {stderr}"
    );
}
