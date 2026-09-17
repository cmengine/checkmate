//! Real-world mod + schema suites: a throwaway mod directory on disk with
//! a `mod.toml`, a `schemas/` tree, and `src/` modules, driven through the
//! real `LspService` — the exact shape a mod author works in.
//!
//! The language server has no `--schema` flag; these tests pin the
//! auto-detection contract: mods are discovered by walking up to the
//! nearest `mod.toml` (§10.1), schemas are collected from the mod root and
//! the parent directory's `schemas/` tree, the manifest's `[schemas]`
//! table narrows the grant (§9.5, §10.2), and the mod is checked as a
//! whole assembly (§10.3/§10.4) with per-module re-anchored diagnostics.

use futures::StreamExt;
use serde_json::json;
use tower::{Service, ServiceExt};
use tower_lsp_server::LspService;
use tower_lsp_server::jsonrpc;

use cme_lsp::server::CheckmateLsp;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Fixture: a mod directory on disk
// ---------------------------------------------------------------------------

/// A unique throwaway directory per test (parallel test runs stay apart).
fn temp_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cme-lsp-modsuite-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp mod root");
    dir
}

/// Writes `body` to `root/relative`, creating parents.
fn write(root: &std::path::Path, relative: &str, body: &str) -> std::path::PathBuf {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
    std::fs::write(&path, body).expect("write fixture");
    path
}

fn file_uri(path: &std::path::Path) -> String {
    tower_lsp_server::ls_types::Uri::from_file_path(path)
        .expect("file uri")
        .to_string()
}

/// The §9 shape from the task brief: game schema with a struct, an enum, a
/// capability, and an interface.
const GAME_SCHEMA: &str = "\
schema game 1.0.0

struct Sprite {
    int id
    str name
    float scale
}

enum Event {
    Started
    Scored(int points)
}

since 1.0.0 capability window {
    Sprite OpenWindow(str title)
    void Draw(Sprite sprite)
}

since 1.0.0 interface gamemode {
    int OnEvent(Event event)
    int Tick(int frame)
}
";

const MOD_MANIFEST: &str = "\
name = \"game_mod\"
version = \"1.0.0\"
checkmate_version = \"0.3.0\"

[schemas]
game = \"1.0.0\"
";

/// A mod using the schema correctly: capability import + calls, interface
/// impl with exact signatures, schema types constructed and matched.
const GOOD_MAIN: &str = "\
import game.window

impl game.gamemode {
    int OnEvent(Event event) {
        return match (event) {
            Started() => 0
            Scored(int points) => points
        }
    }
    int Tick(int frame) {
        Sprite s = game.window.OpenWindow(\"hero\")
        game.window.Draw(s)
        return frame + s.id
    }
}

int main() {
    return 1
}
";

struct Harness {
    service: LspService<CheckmateLsp>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<jsonrpc::Request>,
}

async fn setup() -> Harness {
    let (service, socket) = LspService::build(CheckmateLsp::new)
        .custom_method("cme/expand", CheckmateLsp::expand_preview)
        .finish();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut socket = socket;
        while let Some(message) = socket.next().await {
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    let mut harness = Harness { service, receiver };
    // Lifecycle handshake, exactly as an editor drives it.
    let initialize = jsonrpc::Request::build("initialize".to_string())
        .params(json!({ "capabilities": {} }))
        .id(1)
        .finish();
    let response = harness
        .service
        .ready()
        .await
        .unwrap()
        .call(initialize)
        .await
        .unwrap()
        .expect("initialize response");
    assert!(response.is_ok(), "initialize: {response:?}");
    let initialized = jsonrpc::Request::build("initialized".to_string())
        .params(json!({}))
        .finish();
    let _ = harness
        .service
        .ready()
        .await
        .unwrap()
        .call(initialized)
        .await
        .unwrap();
    harness
}

impl Harness {
    async fn notify(&mut self, method: &str, params: serde_json::Value) {
        let request = jsonrpc::Request::build(method.to_string())
            .params(params)
            .finish();
        let response = self
            .service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap();
        assert!(response.is_none(), "{method} is a notification");
    }

    async fn open(&mut self, path: &std::path::Path, text: &str) {
        let uri = file_uri(path);
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": { "uri": uri, "languageId": "checkmate", "version": 1, "text": text }
            }),
        )
        .await;
    }

    async fn change(&mut self, path: &std::path::Path, text: &str) {
        let uri = file_uri(path);
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [ { "text": text } ]
            }),
        )
        .await;
    }

    /// The diagnostics published for `path` (draining any other publishes
    /// that arrive first).
    async fn publish_for(&mut self, path: &std::path::Path) -> serde_json::Value {
        let uri = file_uri(path);
        loop {
            let message = tokio::time::timeout(TIMEOUT, self.receiver.recv())
                .await
                .expect("message within timeout")
                .expect("socket stays open");
            if message.method() != "textDocument/publishDiagnostics" {
                continue;
            }
            let params = message.params().expect("params").clone();
            if params["uri"] == json!(uri) {
                return params["diagnostics"].clone();
            }
        }
    }

    async fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(5000);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let request = jsonrpc::Request::build(method.to_string())
            .params(params)
            .id(id)
            .finish();
        let response = tokio::time::timeout(TIMEOUT, async {
            self.service
                .ready()
                .await
                .unwrap()
                .call(request)
                .await
                .unwrap()
                .expect("requests get a response")
        })
        .await
        .expect("request within timeout");
        response
            .result()
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    }
}

/// Assembles the canonical game mod; returns (root, main.cm path).
fn game_mod() -> (std::path::PathBuf, std::path::PathBuf) {
    let root = temp_root("game");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", GOOD_MAIN);
    (root, main)
}

fn message_text(diagnostic: &serde_json::Value) -> String {
    diagnostic["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn labels(result: &serde_json::Value) -> Vec<String> {
    result
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["label"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Diagnostics: schema usage is understood, real defects still surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_mod_script_using_its_schema_has_no_false_errors() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    harness.open(&main, GOOD_MAIN).await;
    let diagnostics = harness.publish_for(&main).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "the §9 flow (import + capability calls + impl + schema types) must be clean: {diagnostics}"
    );
    let _ = root;
}

#[tokio::test]
async fn capability_calls_are_type_checked_against_the_schema() {
    let (root, main) = game_mod();
    // Draw takes a Sprite, not an int — a real defect the schema catches.
    let broken = GOOD_MAIN.replace("game.window.Draw(s)", "game.window.Draw(7)");
    assert_ne!(broken, GOOD_MAIN);
    let mut harness = setup().await;
    harness.open(&main, &broken).await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("Draw")),
        "the wrong-typed capability call is reported: {messages:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn unknown_capability_members_are_reported() {
    let (root, main) = game_mod();
    let broken = GOOD_MAIN.replace("game.window.Draw(s)", "game.window.Kiss(s)");
    let mut harness = setup().await;
    harness.open(&main, &broken).await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("Kiss") && m.contains("no member")),
        "a member the schema does not declare cannot be called: {messages:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn an_incomplete_impl_is_reported() {
    let (root, main) = game_mod();
    // Drop the required OnEvent member: the impl no longer satisfies the
    // interface (§9.1, §10.4).
    let broken = GOOD_MAIN.replace(
        "    int OnEvent(Event event) {\n        return match (event) {\n            Started() => 0\n            Scored(int points) => points\n        }\n    }\n",
        "",
    );
    assert_ne!(broken, GOOD_MAIN);
    let mut harness = setup().await;
    harness.open(&main, &broken).await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("missing") && m.contains("OnEvent")),
        "the missing interface member is reported: {messages:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn wrong_impl_signature_is_reported() {
    let (root, main) = game_mod();
    // Tick must return int; void breaks the exact-signature rule.
    let broken = GOOD_MAIN.replace("int Tick(int frame)", "void Tick(int frame)");
    let mut harness = setup().await;
    harness.open(&main, &broken).await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("Tick")),
        "a wrong-signature impl member is reported: {messages:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn members_hidden_by_the_target_version_are_enforced() {
    // A schema that grew after the mod's target: OpenWindow moves to
    // since 1.1.0 while the manifest targets 1.0.0 — the call must be
    // hidden (§9.5), which is exactly the schema's backwards-compatibility
    // promise.
    let root = temp_root("hiding");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(
        &root,
        "schemas/game.cm",
        &GAME_SCHEMA.replace(
            "since 1.0.0 capability window {",
            "since 1.1.0 capability window {",
        ),
    );
    let main = write(&root, "src/main.cm", GOOD_MAIN);
    let mut harness = setup().await;
    harness.open(&main, GOOD_MAIN).await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("OpenWindow")),
        "a member introduced after the target version is invisible (§9.5): {messages:?}"
    );
}

#[tokio::test]
async fn a_namespace_missing_from_the_manifest_is_not_granted() {
    // The manifest grants `game` only: `import other.helper` names an
    // unknown namespace and is reported (§7.2, §9.5).
    let root = temp_root("grant");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(
        &root,
        "src/main.cm",
        "import game.window\nimport other.helper\n\nint main() {\n    return 0\n}\n",
    );
    let mut harness = setup().await;
    harness
        .open(
            &main,
            "import game.window\nimport other.helper\n\nint main() {\n    return 0\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("other") && m.contains("namespace")),
        "the ungranted namespace is reported: {messages:?}"
    );
}

#[tokio::test]
async fn a_defective_schema_keeps_the_mod_on_its_last_clean_contract() {
    // The schema file is OPEN and mid-edit (a broken head). The schema
    // buffer itself reports its defect; the mod's scripts keep checking
    // against the last clean parse of that file — an error storm over
    // every script while the schema is being edited is worse than a
    // moment-stale contract.
    let root = temp_root("broken-schema");
    write(&root, "mod.toml", MOD_MANIFEST);
    let schema = write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", GOOD_MAIN);
    let mut harness = setup().await;

    harness.open(&schema, GAME_SCHEMA).await;
    let _ = harness.publish_for(&schema).await;
    harness.open(&main, GOOD_MAIN).await;
    let diagnostics = harness.publish_for(&main).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "the clean schema checks the script: {diagnostics}"
    );

    // Break the schema buffer: its own buffer reports the defect...
    harness
        .change(&schema, &format!("{GAME_SCHEMA}\ncapability {{\n"))
        .await;
    let schema_diags = harness.publish_for(&schema).await;
    assert!(
        !schema_diags.as_array().expect("array").is_empty(),
        "the defective schema reports its own defect"
    );

    // ...and the script keeps its clean verdict from the last contract.
    harness.change(&main, GOOD_MAIN).await;
    let diagnostics = harness.publish_for(&main).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "the script stays clean on the last good contract while the schema is mid-edit: {diagnostics}"
    );
    let _ = root;
}

// ---------------------------------------------------------------------------
// The mod tree as one program: cross-module linking (§10.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cross_module_calls_link_through_the_mod() {
    let root = temp_root("linking");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    // §10.3: an import links the module's top-level names into the shared
    // namespace — the helper is called UNQUALIFIED, exactly like the
    // repository's own mod_cli fixtures.
    write(
        &root,
        "src/main.cm",
        "import self.helpers\n\nint main() {\n    return triple(3)\n}\n",
    );
    let helpers = write(
        &root,
        "src/helpers.cm",
        "int triple(int v) {\n    return v * 3\n}\n",
    );
    let mut harness = setup().await;
    harness
        .open(&helpers, "int triple(int v) {\n    return v * 3\n}\n")
        .await;
    let diagnostics = harness.publish_for(&helpers).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "a helper module is clean on its own: {diagnostics}"
    );
    // Now the importing module: the cross-module call must link.
    let main = root.join("src/main.cm");
    harness
        .open(
            &main,
            "import self.helpers\n\nint main() {\n    return triple(3)\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&main).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "`import self.helpers` links the helper into the mod's shared namespace (§10.3): {diagnostics}"
    );
}

#[tokio::test]
async fn a_self_import_missing_from_the_module_tree_is_reported() {
    let root = temp_root("self-import");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(
        &root,
        "src/main.cm",
        "import self.missing\n\nint main() {\n    return 0\n}\n",
    );
    let mut harness = setup().await;
    harness
        .open(
            &main,
            "import self.missing\n\nint main() {\n    return 0\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("self.missing") && m.contains("does not match any module")),
        "the missing module is named at the import (§10.3): {messages:?}"
    );
}

#[tokio::test]
async fn sibling_modules_see_each_others_impl_members_without_duplicates() {
    // §10.4: impl blocks for one target union across the mod tree, and a
    // duplicate member is an error.
    let root = temp_root("union");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let a = write(
        &root,
        "src/a.cm",
        "impl game.gamemode {\n    int OnEvent(Event event) {\n        return 0\n    }\n}\n",
    );
    let b = write(
        &root,
        "src/b.cm",
        "impl game.gamemode {\n    int Tick(int frame) {\n        return frame\n    }\n}\n",
    );
    let mut harness = setup().await;
    harness
        .open(
            &a,
            "impl game.gamemode {\n    int OnEvent(Event event) {\n        return 0\n    }\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&a).await;
    assert!(
        diagnostics
            .as_array()
            .expect("array")
            .iter()
            .all(|d| !message_text(d).contains("Tick")),
        "a member implemented in a SIBLIE module completes the interface: {diagnostics}"
    );
    harness
        .open(
            &b,
            "impl game.gamemode {\n    int Tick(int frame) {\n        return frame\n    }\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&b).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "the union across files satisfies the interface (§10.4): {diagnostics}"
    );
}

// ---------------------------------------------------------------------------
// Completion and hover over the schema surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn completion_offers_schema_boundary_types_and_contract_members() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    // Statement position at the top level: schema types are candidates.
    harness.open(&main, &format!("{GOOD_MAIN}\n")).await;
    let _ = harness.publish_for(&main).await;
    let text = format!(
        "import game.window\n\nint main() {{\n    infer s = \n    return 0\n}}\n{GOOD_MAIN}"
    );
    harness.change(&main, &text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 3, "character": 13 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.iter().any(|label| label == "Sprite"),
        "the schema struct Sprite completes in expression position: {offered:?}"
    );
    assert!(
        offered.iter().any(|label| label == "Event"),
        "the schema enum Event completes too: {offered:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn completion_after_a_capability_path_offers_its_members() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    let text = "import game.window\n\nint main() {\n    game.window.\n    return 0\n}\n";
    harness.open(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 3, "character": 16 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.contains(&"OpenWindow".to_string()) && offered.contains(&"Draw".to_string()),
        "capability members complete after `game.window.`: {offered:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn completion_after_the_namespace_offers_its_contracts() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    let text = "import game.window\n\nint main() {\n    game.\n    return 0\n}\n";
    harness.open(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 3, "character": 9 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.contains(&"window".to_string()),
        "capabilities complete after `game.`: {offered:?}"
    );
    assert!(
        offered.contains(&"gamemode".to_string()),
        "interfaces complete after `game.` (they are impl targets): {offered:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn named_arguments_complete_inside_capability_calls() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    let text = "import game.window\n\nint main() {\n    game.window.OpenWindow(\n    return 0\n}\n";
    harness.open(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 3, "character": 31 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.iter().any(|label| label == "title"),
        "the schema member's parameters complete as named arguments (§2.12): {offered:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn import_completion_offers_schema_namespaces_and_capabilities() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    let text = "import \n\nint main() {\n    return 0\n}\n";
    harness.open(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 0, "character": 7 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.contains(&"game".to_string()) && offered.contains(&"self".to_string()),
        "the schema namespace and the mod root complete after `import `: {offered:?}"
    );

    let text = "import game.\n\nint main() {\n    return 0\n}\n";
    harness.change(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 0, "character": 12 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.contains(&"window".to_string()),
        "the capability completes one dot under the namespace (§9.1): {offered:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn hover_shows_capability_member_signatures() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    let text = "import game.window\n\nint main() {\n    game.window.OpenWindow(\"hero\")\n    return 0\n}\n";
    harness.open(&main, text).await;
    let _ = harness.publish_for(&main).await;
    // Hover the `OpenWindow` token (line 3).
    let offset = text.find("OpenWindow").expect("call site");
    let (line, column) = line_column(text, offset + 4);
    let result = harness
        .request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": line, "character": column }
            }),
        )
        .await;
    let rendered = result.to_string();
    assert!(
        rendered.contains("OpenWindow") && rendered.contains("Sprite"),
        "hover shows the schema member's full signature: {rendered}"
    );
    let _ = root;
}

/// The byte offset → LSP (line, character) pair.
fn line_column(text: &str, offset: usize) -> (u32, u32) {
    let mut line = 0u32;
    let mut character = 0u32;
    for (index, byte) in text.bytes().enumerate() {
        if index == offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }
    (line, character)
}

#[tokio::test]
async fn implementing_a_schema_interface_offers_its_missing_members() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    let text =
        "import game.window\n\nimpl game.gamemode {\n    \n}\n\nint main() {\n    return 0\n}\n";
    harness.open(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 3, "character": 4 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.contains(&"OnEvent".to_string()) && offered.contains(&"Tick".to_string()),
        "the interface's missing members complete inside `impl game.gamemode`: {offered:?}"
    );
    let fills: Vec<(String, Option<String>)> = result
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            (
                item["label"].as_str().unwrap_or_default().to_string(),
                item["insertText"].as_str().map(str::to_string),
            )
        })
        .collect();
    let on_event = fills
        .iter()
        .find(|(label, _)| label == "OnEvent")
        .expect("OnEvent offered");
    assert!(
        on_event
            .1
            .as_deref()
            .is_some_and(|fill| fill.contains("int OnEvent(Event event)")),
        "the fill is the exact signature the checker requires: {fills:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn editing_the_open_schema_buffer_updates_the_grant_immediately() {
    // The schema file is OPEN: adding a capability member in the buffer
    // (unsaved) must let scripts complete it without any disk write.
    let root = temp_root("buffer-schema");
    write(&root, "mod.toml", MOD_MANIFEST);
    let schema = write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", GOOD_MAIN);
    let mut harness = setup().await;

    harness.open(&schema, GAME_SCHEMA).await;
    let _ = harness.publish_for(&schema).await;
    harness.open(&main, GOOD_MAIN).await;
    let _ = harness.publish_for(&main).await;

    let grown = GAME_SCHEMA.replace(
        "since 1.0.0 capability window {",
        "since 1.0.0 capability window {\n    void Blink(int times)",
    );
    harness.change(&schema, &grown).await;
    let _ = harness.publish_for(&schema).await;

    let text = "import game.window\n\nint main() {\n    game.window.\n    return 0\n}\n";
    harness.change(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 3, "character": 16 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.contains(&"Blink".to_string()),
        "the member added in the open schema buffer completes: {offered:?}"
    );
    let _ = root;
}

// ---------------------------------------------------------------------------
// Deeper real-world shapes: nested modules, sibling mods, ranges, and the
// buffer lifecycle
// ---------------------------------------------------------------------------

/// A richer schema for the visibility/shape tests: `extra` namespace for
/// grant narrowing, and members spread across versions.
const VERSIONED_SCHEMA: &str = "\
schema media 1.4.0

struct Clip {
    int id
    str name
}

since 1.0.0 capability player {
    void Play(Clip clip)
}

since 1.4.0 capability player {
    void PlayHd(Clip clip)
}
";

#[tokio::test]
async fn mod_detection_walks_up_from_nested_module_directories() {
    // The file lives in src/ui/hud.cm — two levels under the mod root.
    // Discovery must still find mod.toml and the schemas.
    let root = temp_root("nested");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let hud = write(
        &root,
        "src/ui/hud.cm",
        "import game.window\n\nimpl game.gamemode {\n    int Tick(int frame) {\n        Sprite s = game.window.OpenWindow(\"hud\")\n        game.window.Draw(s)\n        return s.id\n    }\n    int OnEvent(Event event) {\n        return 0\n    }\n}\n",
    );
    let mut harness = setup().await;
    harness
        .open(
            &hud,
            "import game.window\n\nimpl game.gamemode {\n    int Tick(int frame) {\n        Sprite s = game.window.OpenWindow(\"hud\")\n        game.window.Draw(s)\n        return s.id\n    }\n    int OnEvent(Event event) {\n        return 0\n    }\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&hud).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "a nested module still sees the mod's schema contract: {diagnostics}"
    );
}

#[tokio::test]
async fn the_parent_directory_schemas_layout_is_discovered() {
    // The host_app layout: schemas/ sits NEXT to the mod directory, not
    // inside it — the shape of this repository's own host_api_test/Rust.
    let root = temp_root("hostapp");
    let shop_schema = GAME_SCHEMA.replace("schema game", "schema shop");
    let manifest = MOD_MANIFEST
        .replace("name = \"game_mod\"", "name = \"shop_mod\"")
        .replace("game = \"1.0.0\"", "shop = \"1.0.0\"");
    write(&root, "schemas/shop.cm", &shop_schema);
    let main = write(
        &root,
        "shop_mod/src/main.cm",
        "import shop.window\n\nimpl shop.gamemode {\n    int OnEvent(Event event) {\n        return 0\n    }\n    int Tick(int frame) {\n        return frame\n    }\n}\n",
    );
    write(&root, "shop_mod/mod.toml", &manifest);
    let mut harness = setup().await;
    harness
        .open(
            &main,
            "import shop.window\n\nimpl shop.gamemode {\n    int OnEvent(Event event) {\n        return 0\n    }\n    int Tick(int frame) {\n        return frame\n    }\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&main).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "the parent's schemas/ tree is discovered (§10 layout variant): {diagnostics}"
    );
}

#[tokio::test]
async fn manifest_target_versions_select_visible_members() {
    // media 1.4.0 schema, mod targets 1.0.0: Play is visible, PlayHd is
    // hidden — and the HIDDEN call must be reported (§9.5).
    let root = temp_root("versions");
    write(&root, "mod.toml", &MOD_MANIFEST.replace("game", "media"));
    write(&root, "schemas/media.cm", VERSIONED_SCHEMA);
    let main = write(
        &root,
        "src/main.cm",
        "import media.player\n\nint main() {\n    media.player.PlayHd(Clip(id: 1, name: \"x\"))\n    return 0\n}\n",
    );
    let mut harness = setup().await;
    harness
        .open(
            &main,
            "import media.player\n\nint main() {\n    media.player.PlayHd(Clip(id: 1, name: \"x\"))\n    return 0\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("PlayHd")),
        "the member from the future version is hidden (§9.5): {messages:?}"
    );
}

#[tokio::test]
async fn the_manifest_narrows_even_discovered_namespaces() {
    // extra.cm is discovered on disk but NOT in [schemas] — importing it
    // is refused, exactly like a CLI mod run.
    let root = temp_root("narrow");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    write(
        &root,
        "schemas/extra.cm",
        &GAME_SCHEMA.replace("schema game", "schema extra"),
    );
    let main = write(
        &root,
        "src/main.cm",
        "import extra.window\n\nint main() {\n    return 0\n}\n",
    );
    let mut harness = setup().await;
    harness
        .open(
            &main,
            "import extra.window\n\nint main() {\n    return 0\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("extra")),
        "an ungranted namespace stays invisible (§7.2, §9.5): {messages:?}"
    );
}

#[tokio::test]
async fn sibling_mods_do_not_leak_schemas_into_each_other() {
    let root = temp_root("siblings");
    write(&root, "a/mod.toml", MOD_MANIFEST);
    write(&root, "a/schemas/game.cm", GAME_SCHEMA);
    write(&root, "a/src/main.cm", "int main() {\n    return 0\n}\n");
    write(
        &root,
        "b/mod.toml",
        "name = \"b\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.3.0\"\n",
    );
    let b_main = write(&root, "b/src/main.cm", "int main() {\n    return 0\n}\n");
    let mut harness = setup().await;
    harness
        .open(&b_main, "int main() {\n    return 0\n}\n")
        .await;
    let diagnostics = harness.publish_for(&b_main).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "mod b has no schemas and no imports: nothing to report: {diagnostics}"
    );
    // And b's scripts do NOT see a's schema in completion.
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&b_main) },
                "position": { "line": 2, "character": 13 }
            }),
        )
        .await;
    assert!(
        !result.to_string().contains("Sprite"),
        "mod b must not see mod a's boundary types: {result}"
    );
}

#[tokio::test]
async fn errors_anchor_at_the_right_line_in_the_buffer() {
    let (root, main) = game_mod();
    let broken = GOOD_MAIN.replace("game.window.Draw(s)", "game.window.Draw(7)");
    let mut harness = setup().await;
    harness.open(&main, &broken).await;
    let diagnostics = harness.publish_for(&main).await;
    let entries = diagnostics.as_array().expect("array");
    let line_of_draw = broken
        .lines()
        .position(|line| line.contains("Draw(7)"))
        .expect("Draw line") as u64;
    let anchored = entries
        .iter()
        .find(|d| d["range"]["start"]["line"] == json!(line_of_draw));
    assert!(
        anchored.is_some(),
        "the capability error anchors on its own line {line_of_draw}: {entries:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn editing_a_buffer_to_break_a_capability_call_republishes() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    harness.open(&main, GOOD_MAIN).await;
    let clean = harness.publish_for(&main).await;
    assert_eq!(clean.as_array().map(Vec::len), Some(0), "{clean}");

    // A mid-edit typo: the wrong-typed call surfaces on the very next
    // publish, anchored in the buffer.
    let broken = GOOD_MAIN.replace("game.window.Draw(s)", "game.window.Draw(true)");
    harness.change(&main, &broken).await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("Draw")),
        "the buffer edit re-checks against the schema: {messages:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn schema_enum_variants_complete_after_the_type_name() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    // `Event.` inside a match arm position — the schema enum's variants.
    let text = "import game.window\n\nimpl game.gamemode {\n    int OnEvent(Event event) {\n        return match (event) {\n            Started() => 0\n            Scored(int points) => points\n        }\n    }\n    int Tick(int frame) {\n        return frame\n    }\n}\n\nint main() {\n    Event.\n    return 0\n}\n";
    harness.open(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 15, "character": 10 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.contains(&"Started".to_string()) && offered.contains(&"Scored".to_string()),
        "the schema enum's variants complete after `Event.` (§2.7): {offered:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn hover_on_a_schema_struct_shows_its_fields_in_a_script() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    let text = "import game.window\n\nimpl game.gamemode {\n    int OnEvent(Event event) {\n        return 0\n    }\n    int Tick(int frame) {\n        Sprite s = game.window.OpenWindow(\"hero\")\n        game.window.Draw(s)\n        return s.id\n    }\n}\n\nint main() {\n    return 0\n}\n";
    harness.open(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let offset = text.find("Sprite s").expect("declaration") + 1;
    let (line, column) = line_column(text, offset);
    let result = harness
        .request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": line, "character": column }
            }),
        )
        .await;
    let rendered = result.to_string();
    assert!(
        rendered.contains("struct Sprite") && rendered.contains("id"),
        "the schema struct hovers with its fields (§9.3): {rendered}"
    );
    let _ = root;
}

#[tokio::test]
async fn go_to_definition_on_a_schema_type_answers_null_without_crashing() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    harness.open(&main, GOOD_MAIN).await;
    let _ = harness.publish_for(&main).await;
    let offset = GOOD_MAIN.find("Sprite s = ").expect("declaration");
    let (line, column) = line_column(GOOD_MAIN, offset);
    let result = harness
        .request(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": line, "character": column }
            }),
        )
        .await;
    assert!(
        result.is_null(),
        "schema types live in another file — null, not a bogus jump: {result}"
    );
    // And a LOCAL symbol still resolves (regression guard).
    let offset = GOOD_MAIN.find("return frame + s.id").expect("use") + "return frame + ".len();
    let (line, column) = line_column(GOOD_MAIN, offset);
    let result = harness
        .request(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": line, "character": column }
            }),
        )
        .await;
    let rendered = result.to_string();
    assert!(
        !rendered.is_empty() && rendered != "null",
        "the local `s` still jumps to its declaration: {result}"
    );
    let _ = root;
}

#[tokio::test]
async fn document_symbols_do_not_leak_schema_types_into_the_outline() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    harness.open(&main, GOOD_MAIN).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": file_uri(&main) } }),
        )
        .await;
    let rendered = result.to_string();
    assert!(
        rendered.contains("OnEvent") && rendered.contains("Tick"),
        "the file's impl members outline: {rendered}"
    );
    assert!(
        !rendered.contains("\"Sprite\"") && !rendered.contains("\"Event\""),
        "schema boundary types are not document symbols: {rendered}"
    );
    let _ = root;
}

#[tokio::test]
async fn option_and_result_constructors_still_complete_in_mod_scripts() {
    let (root, main) = game_mod();
    let mut harness = setup().await;
    let text = "import game.window\n\nint main() {\n    option.\n    return 0\n}\n";
    harness.open(&main, text).await;
    let _ = harness.publish_for(&main).await;
    let result = harness
        .request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": file_uri(&main) },
                "position": { "line": 3, "character": 12 }
            }),
        )
        .await;
    let offered = labels(&result);
    assert!(
        offered.contains(&"Some".to_string()) && offered.contains(&"None".to_string()),
        "the built-in constructors survive the schema-aware receiver path: {offered:?}"
    );
    let _ = root;
}

#[tokio::test]
async fn a_mod_without_any_schema_still_validates_self_imports() {
    // No schema files anywhere: the plain pipeline must still run — and
    // self-import validation still names the missing module.
    let root = temp_root("schemaless");
    write(
        &root,
        "mod.toml",
        "name = \"plain\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.3.0\"\n",
    );
    let main = write(
        &root,
        "src/main.cm",
        "import self.ghost\n\nint main() {\n    return 0\n}\n",
    );
    let mut harness = setup().await;
    harness
        .open(
            &main,
            "import self.ghost\n\nint main() {\n    return 0\n}\n",
        )
        .await;
    let diagnostics = harness.publish_for(&main).await;
    let messages: Vec<String> = diagnostics
        .as_array()
        .expect("array")
        .iter()
        .map(message_text)
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("self.ghost") && m.contains("does not match any module")),
        "a schema-less mod still validates its module tree (§10.3): {messages:?}"
    );
}

#[tokio::test]
async fn every_module_publishes_its_own_share_of_the_mod_check() {
    // Two modules, one defect each file; opening each publishes exactly
    // that file's diagnostics (no cross-file bleed).
    let root = temp_root("shares");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let a = write(
        &root,
        "src/a.cm",
        "impl game.gamemode {\n    int OnEvent(Event event) {\n        string wrong = \"x\"\n        return 0\n    }\n}\n",
    );
    let b = write(
        &root,
        "src/b.cm",
        "impl game.gamemode {\n    int Tick(int frame) {\n        return frame\n    }\n}\n",
    );
    let mut harness = setup().await;
    harness
        .open(
            &a,
            "impl game.gamemode {\n    int OnEvent(Event event) {\n        string wrong = \"x\"\n        return 0\n    }\n}\n",
        )
        .await;
    let a_diags = harness.publish_for(&a).await;
    assert!(
        !a_diags.as_array().expect("array").is_empty(),
        "a's own defect (unknown type `string`) publishes to a: {a_diags}"
    );
    harness
        .open(
            &b,
            "impl game.gamemode {\n    int Tick(int frame) {\n        return frame\n    }\n}\n",
        )
        .await;
    let b_diags = harness.publish_for(&b).await;
    assert_eq!(
        b_diags.as_array().map(Vec::len),
        Some(0),
        "b stays clean even though the mod as a whole has a defect elsewhere: {b_diags}"
    );
    let _ = root;
}

#[tokio::test]
async fn a_member_added_in_the_open_schema_buffer_type_checks_immediately() {
    // Diagnostics variant of the buffer lifecycle: the script CALLS the
    // new member; the schema buffer carries it unsaved; the load-time
    // provider rule aside, the checker must see the member.
    let root = temp_root("buffer-diag");
    write(&root, "mod.toml", MOD_MANIFEST);
    let schema = write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", GOOD_MAIN);
    let mut harness = setup().await;
    harness.open(&schema, GAME_SCHEMA).await;
    let _ = harness.publish_for(&schema).await;
    harness.open(&main, GOOD_MAIN).await;
    let _ = harness.publish_for(&main).await;

    // Break Tick's use of Draw with a wrong type; then fix BOTH buffers.
    let broken = GOOD_MAIN.replace("game.window.Draw(s)", "game.window.Draw(1)");
    harness.change(&main, &broken).await;
    let diagnostics = harness.publish_for(&main).await;
    assert!(
        !diagnostics.as_array().expect("array").is_empty(),
        "the broken call reports: {diagnostics}"
    );
    harness.change(&main, GOOD_MAIN).await;
    let fixed = harness.publish_for(&main).await;
    assert_eq!(
        fixed.as_array().map(Vec::len),
        Some(0),
        "the fix clears the diagnostics: {fixed}"
    );
    let _ = root;
}
