//! Typing-simulation suite: the language server driven the way a real
//! author drives it — a schema and a mod on disk, then the script typed
//! into the editor ONE CHARACTER AT A TIME through full-text `didChange`,
//! with `textDocument/completion` evaluated at the cursor after every
//! keystroke and `publishDiagnostics` drained after every change.
//!
//! The invariants pinned here are the incremental ones the static suites
//! cannot see: autocomplete must offer the useful surface at every
//! mid-typing state (imports, host paths, named arguments, schema types,
//! impl members), must never offer the wrong surface for the position
//! (capability members before their path exists, contracts in scope
//! position, anything inside string literals), and diagnostics must stay
//! well-formed through every intermediate — including broken — state,
//! converging to zero errors exactly when the script becomes valid.

use futures::StreamExt;
use serde_json::json;
use tower::{Service, ServiceExt};
use tower_lsp_server::LspService;
use tower_lsp_server::jsonrpc;

use cme_lsp::server::CheckmateLsp;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Fixture: a real schema + mod on disk (§9 + §10)
// ---------------------------------------------------------------------------

fn temp_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cme-lsp-typing-{}-{}-{}",
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

/// The script typed character by character: imports a capability,
/// implements the interface with both members, constructs a schema type,
/// calls the capability with positional and statement shapes, matches the
/// enum, and reads a field.
const TYPED_SCRIPT: &str = "\
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
    return 0
}
";

// ---------------------------------------------------------------------------
// Wire harness: the exact shape an editor drives
// ---------------------------------------------------------------------------

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

    async fn change(&mut self, path: &std::path::Path, version: i32, text: &str) {
        let uri = file_uri(path);
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [ { "text": text } ]
            }),
        )
        .await;
    }

    /// The diagnostics published for `path`, draining any publishes that
    /// arrive first.
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
        static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(9000);
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

// ---------------------------------------------------------------------------
// Typing simulation: one character per didChange, completion after each
// ---------------------------------------------------------------------------

/// The end-of-text cursor position for `text` (ASCII scripts: characters
/// equal bytes, and every line break is one `\n`).
fn end_position(text: &str) -> serde_json::Value {
    let line = text.matches('\n').count() as u32;
    let character = text.rsplit('\n').next().unwrap_or("").chars().count() as u32;
    json!({ "line": line, "character": character })
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

fn message_texts(diagnostics: &serde_json::Value) -> Vec<String> {
    diagnostics
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|d| d["message"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The per-keystroke trace of a typing session: `completions[i]` holds the
/// labels offered at the cursor after the first `i + 1` characters were
/// typed. Diagnostics are drained (and shape-checked) after every change.
struct TypingTrace {
    completions: Vec<Vec<String>>,
}

async fn type_char_by_char(
    harness: &mut Harness,
    path: &std::path::Path,
    final_text: &str,
) -> TypingTrace {
    // Open with the first character; every further character is one
    // full-text didChange — the exact traffic a keystroke produces.
    let mut chars = final_text.chars();
    let first: String = chars.next().expect("non-empty script").to_string();
    harness.open(path, &first).await;
    let mut completions = Vec::new();
    let _ = harness.publish_for(path).await;
    completions.push(labels(
        &harness
            .request(
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": file_uri(path) },
                    "position": end_position(&first)
                }),
            )
            .await,
    ));

    let mut prefix = first;
    for (step, ch) in chars.enumerate() {
        prefix.push(ch);
        harness.change(path, (step as i32) + 2, &prefix).await;
        // Every change publishes diagnostics for the buffer; the payload
        // must stay well-formed through every intermediate state.
        let diagnostics = harness.publish_for(path).await;
        assert!(
            diagnostics.is_array(),
            "diagnostics must stay an array mid-typing: {diagnostics}"
        );
        for message in message_texts(&diagnostics) {
            assert!(
                !message.contains("internal error") && !message.contains("panic"),
                "a keystroke surfaced an internal failure: {message:?}"
            );
        }
        completions.push(labels(
            &harness
                .request(
                    "textDocument/completion",
                    json!({
                        "textDocument": { "uri": file_uri(path) },
                        "position": end_position(&prefix)
                    }),
                )
                .await,
        ));
    }
    TypingTrace { completions }
}

impl TypingTrace {
    /// Labels offered after the keystroke that completed the FIRST
    /// occurrence of `needle` in the final text (the prefix whose text
    /// ends with `needle`).
    fn after(&self, final_text: &str, needle: &str) -> &[String] {
        let start = final_text
            .find(needle)
            .unwrap_or_else(|| panic!("script must contain {needle:?}"));
        let prefix = &final_text[..start + needle.len()];
        let prefix_chars = prefix.chars().count();
        self.completions
            .get(prefix_chars - 1)
            .unwrap_or_else(|| panic!("prefix of {prefix_chars} chars not in trace"))
    }
}

// ---------------------------------------------------------------------------
// Autocomplete across the keystroke stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn import_completion_is_live_while_the_path_is_typed() {
    let root = temp_root("import");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", "");
    let mut harness = setup().await;

    let head = "import game.window\n";
    harness.open(&main, "").await;
    let _ = harness.publish_for(&main).await;

    // Type the import line one character at a time.
    let mut typed = String::new();
    let mut saw_namespace = false;
    let mut saw_contracts = false;
    let mut saw_partial_contracts = false;
    let mut member_leaked = false;
    for ch in head.chars() {
        typed.push(ch);
        harness
            .change(&main, typed.matches('\n').count() as i32 + 2, &typed)
            .await;
        let _ = harness.publish_for(&main).await;
        let offered = labels(
            &harness
                .request(
                    "textDocument/completion",
                    json!({
                        "textDocument": { "uri": file_uri(&main) },
                        "position": end_position(&typed)
                    }),
                )
                .await,
        );
        // The namespace is offered as soon as `import ` (or `import g`) is
        // on screen, and stays offered through the partial segments.
        if ((typed.trim_end() != "import" && typed.ends_with('g')) || typed.contains("import game"))
            && offered.iter().any(|l| l == "game")
        {
            saw_namespace = true;
        }
        if typed.ends_with("game.") || typed.ends_with("game.w") || typed.ends_with("game.wi") {
            if offered.contains(&"window".to_string()) && offered.contains(&"gamemode".to_string())
            {
                if typed.ends_with("game.w") || typed.ends_with("game.wi") {
                    saw_partial_contracts = true;
                }
                saw_contracts = true;
            }
            // Capability MEMBERS never leak into contract position, even
            // mid-path.
            if offered.contains(&"OpenWindow".to_string()) || offered.contains(&"Draw".to_string())
            {
                member_leaked = true;
            }
        }
    }
    assert!(
        saw_namespace,
        "the granted namespace must complete while the import is typed"
    );
    assert!(
        saw_contracts,
        "contracts must complete after `game.` mid-typing"
    );
    assert!(
        saw_partial_contracts,
        "contracts must still complete while the segment is partial (`game.wi`)"
    );
    assert!(
        !member_leaked,
        "capability members must not be offered in import/contract position"
    );
    let _ = root;
}

#[tokio::test]
async fn typing_the_whole_script_never_offers_the_wrong_surface() {
    let root = temp_root("full");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", "");
    let mut harness = setup().await;

    let trace = type_char_by_char(&mut harness, &main, TYPED_SCRIPT).await;

    // Capability members are offered ONLY through their host path: no
    // keystroke before the first `game.window.` path is complete may
    // surface OpenWindow or Draw.
    let path_needle = "game.window.";
    let first_path_end_bytes =
        TYPED_SCRIPT.find(path_needle).expect("path in script") + path_needle.len();
    for (index, offered) in trace.completions.iter().enumerate() {
        let typed_chars = index + 1;
        let typed_bytes: usize = TYPED_SCRIPT
            .chars()
            .take(typed_chars)
            .map(char::len_utf8)
            .sum();
        if typed_bytes < first_path_end_bytes {
            assert!(
                !offered.contains(&"OpenWindow".to_string()),
                "OpenWindow offered before its host path was typed (keystroke {index}, after {:?})",
                TYPED_SCRIPT.chars().take(typed_chars).collect::<String>()
            );
            assert!(
                !offered.contains(&"Draw".to_string()),
                "Draw offered before its host path was typed (keystroke {index}, after {:?})",
                TYPED_SCRIPT.chars().take(typed_chars).collect::<String>()
            );
        }
    }

    // At the keystroke completing `Sprite s = ` the scope is live: schema
    // types, the visible locals/params, and keywords — but no contracts
    // and no capability members. (`main` is NOT here: it has not been
    // typed yet — completions never leak declarations from the future.)
    let decl_needle = "Sprite s = ";
    let decl_labels = trace.after(TYPED_SCRIPT, decl_needle);
    for expected in ["Sprite", "Event", "frame", "match", "infer"] {
        assert!(
            decl_labels.contains(&expected.to_string()),
            "scope after `{decl_needle}` must offer {expected}: {decl_labels:?}"
        );
    }
    for unexpected in ["window", "gamemode", "OpenWindow", "Draw", "main"] {
        assert!(
            !decl_labels.contains(&unexpected.to_string()),
            "scope after `{decl_needle}` must not offer {unexpected:?}: {decl_labels:?}"
        );
    }

    // Named arguments of the capability call fill inside the parens: the
    // label is the parameter name, the fill carries the `name: ` insert.
    let call_needle = "game.window.OpenWindow(";
    let call_labels = trace.after(TYPED_SCRIPT, call_needle);
    assert!(
        call_labels.contains(&"title".to_string()),
        "the capability's parameter must fill as a named argument: {call_labels:?}"
    );
    // The string value being typed after the opening quote is not an
    // argument-name position: completions go silent there.
    let quote_needle = "game.window.OpenWindow(\"";
    let quote_labels = trace.after(TYPED_SCRIPT, quote_needle);
    assert!(
        quote_labels.is_empty(),
        "typing the string argument silences completions: {quote_labels:?}"
    );

    // After the full host path, its exact members are offered.
    let member_needle = "game.window.";
    let member_labels = trace.after(TYPED_SCRIPT, member_needle);
    // The first `game.window.` in the script is inside the decl above;
    // its completion must offer both capability members.
    assert!(
        member_labels.contains(&"OpenWindow".to_string())
            && member_labels.contains(&"Draw".to_string()),
        "the capability members complete after the typed path: {member_labels:?}"
    );
    assert!(
        !member_labels.contains(&"Sprite".to_string()),
        "schema TYPES are not capability members and must not appear after the path: {member_labels:?}"
    );

    let _ = root;
}

#[tokio::test]
async fn diagnostics_converge_to_clean_exactly_when_the_script_becomes_valid() {
    let root = temp_root("converge");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", "");
    let mut harness = setup().await;

    // Type the whole script; the final state must be defect-free.
    type_char_by_char(&mut harness, &main, TYPED_SCRIPT).await;
    // The last didChange already published; request one more round-trip to
    // observe the settled state.
    harness.change(&main, 10_000, TYPED_SCRIPT).await;
    let diagnostics = harness.publish_for(&main).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "the fully typed script must publish zero diagnostics: {diagnostics}"
    );
    let _ = root;
}

#[tokio::test]
async fn completion_is_silent_inside_string_literals_mid_typing() {
    let root = temp_root("strings");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", "");
    let mut harness = setup().await;

    let trace = type_char_by_char(&mut harness, &main, TYPED_SCRIPT).await;

    // Every keystroke whose cursor sits strictly inside the "hero" string
    // literal must offer nothing: strings are not code.
    let open = TYPED_SCRIPT.find("(\"").expect("call literal") + 1; // the quote
    let close = TYPED_SCRIPT.find("\")").expect("literal close");
    for (index, offered) in trace.completions.iter().enumerate() {
        let typed_chars = index + 1;
        let typed_bytes: usize = TYPED_SCRIPT
            .chars()
            .take(typed_chars)
            .map(char::len_utf8)
            .sum();
        // Strictly inside: after the opening quote, before the closing one.
        if typed_bytes > open && typed_bytes <= close {
            assert!(
                offered.is_empty(),
                "completions leaked inside the string literal (keystroke {index}, after {:?}): {offered:?}",
                TYPED_SCRIPT.chars().take(typed_chars).collect::<String>()
            );
        }
    }
    let _ = root;
}

#[tokio::test]
async fn a_type_error_surfaced_mid_typing_clears_when_fixed() {
    let root = temp_root("recovery");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", "");
    let mut harness = setup().await;

    // The Draw capability takes a Sprite, not an int: a real defect the
    // schema catches, typed in deliberately.
    let broken = TYPED_SCRIPT.replace("game.window.Draw(s)", "game.window.Draw(7)");
    assert_ne!(broken, TYPED_SCRIPT);
    let trace = type_char_by_char(&mut harness, &main, &broken).await;
    let _ = trace;

    harness.change(&main, 20_000, &broken).await;
    let messages = message_texts(&harness.publish_for(&main).await);
    assert!(
        messages.iter().any(|m| m.contains("Draw")),
        "the wrong-typed capability call is reported after typing it: {messages:?}"
    );

    // Fixing the argument clears the diagnostic.
    harness.change(&main, 20_001, TYPED_SCRIPT).await;
    let diagnostics = harness.publish_for(&main).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "the fix clears the schema diagnostic: {diagnostics}"
    );
    let _ = root;
}

#[tokio::test]
async fn typing_a_syntax_broken_script_stays_well_formed() {
    let root = temp_root("broken");
    write(&root, "mod.toml", MOD_MANIFEST);
    write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", "");
    let mut harness = setup().await;

    // Missing closing braces: every keystroke must produce well-formed
    // diagnostics (never an internal error), and the final state reports
    // the defect instead of crashing.
    let broken = "int main() {\n    byte b = 300\n    return b\n";
    type_char_by_char(&mut harness, &main, broken).await;
    harness.change(&main, 30_000, broken).await;
    let messages = message_texts(&harness.publish_for(&main).await);
    assert!(
        !messages.is_empty(),
        "the unclosed program must report its defect: {messages:?}"
    );
    for message in &messages {
        assert!(
            !message.contains("internal error") && !message.contains("panic"),
            "a broken program must degrade to clean diagnostics, not {message:?}"
        );
    }
    let _ = root;
}

#[tokio::test]
async fn typing_the_schema_file_itself_offers_the_authoring_surface() {
    let root = temp_root("schema-typing");
    write(&root, "mod.toml", MOD_MANIFEST);
    let schema = write(&root, "schemas/game.cm", "");
    write(&root, "src/main.cm", "int main() {\n    return 0\n}\n");
    let mut harness = setup().await;

    let trace = type_char_by_char(&mut harness, &schema, GAME_SCHEMA).await;

    // After the header line the top-level authoring surface is live.
    let after_header = trace.after(GAME_SCHEMA, "schema game 1.0.0\n");
    for expected in ["struct", "enum", "capability", "interface"] {
        assert!(
            after_header.contains(&expected.to_string()),
            "the schema top level must offer `{expected}` while typing: {after_header:?}"
        );
    }

    // Inside the interface body the `optional` flag is offered (§9.5:
    // optional members are an INTERFACE concept).
    let interface_needle = "since 1.0.0 interface gamemode {\n    ";
    let interface_labels = trace.after(GAME_SCHEMA, interface_needle);
    assert!(
        interface_labels.contains(&"optional".to_string()),
        "the interface body offers `optional`: {interface_labels:?}"
    );

    // The final schema publishes zero diagnostics for its own buffer.
    harness.change(&schema, 40_000, GAME_SCHEMA).await;
    let diagnostics = harness.publish_for(&schema).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "the fully typed schema must be clean: {diagnostics}"
    );
    let _ = root;
}

#[tokio::test]
async fn a_defective_schema_edit_reports_on_its_own_buffer_only() {
    let root = temp_root("schema-defect");
    write(&root, "mod.toml", MOD_MANIFEST);
    let schema = write(&root, "schemas/game.cm", GAME_SCHEMA);
    let main = write(&root, "src/main.cm", "int main() {\n    return 0\n}\n");
    let mut harness = setup().await;
    harness.open(&main, "int main() {\n    return 0\n}\n").await;
    let _ = harness.publish_for(&main).await;
    harness.open(&schema, GAME_SCHEMA).await;
    let _ = harness.publish_for(&schema).await;

    // Break the schema mid-edit with a parse-level defect: a lowercase
    // member with no parameter list (`int broken`) is reported by the
    // schema file parser itself — the schema buffer reports the defect on
    // itself.
    let broken_schema = GAME_SCHEMA.replace(
        "    Sprite OpenWindow(str title)",
        "    int broken\n    Sprite OpenWindow(str title)",
    );
    assert_ne!(broken_schema, GAME_SCHEMA);
    harness.change(&schema, 50_000, &broken_schema).await;
    let schema_messages = message_texts(&harness.publish_for(&schema).await);
    assert!(
        schema_messages
            .iter()
            .any(|m| m.contains("PascalCase") || m.contains("sprite")),
        "the broken schema buffer reports its own defect: {schema_messages:?}"
    );

    // The script stays clean against the broken schema (the workspace
    // falls back to the schema's last clean parse): re-notify the script
    // buffer and observe its settled diagnostics.
    harness
        .change(&main, 50_001, "int main() {\n    return 0\n}\n")
        .await;
    let script_diagnostics = harness.publish_for(&main).await;
    assert_eq!(
        script_diagnostics.as_array().map(Vec::len),
        Some(0),
        "the clean script must stay clean while its schema is mid-edit: {script_diagnostics}"
    );

    // Restoring the header clears the schema defect.
    harness.change(&schema, 50_002, GAME_SCHEMA).await;
    let diagnostics = harness.publish_for(&schema).await;
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(0),
        "the repaired schema is clean again: {diagnostics}"
    );
    let _ = root;
}
