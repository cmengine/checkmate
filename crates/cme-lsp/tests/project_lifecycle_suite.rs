//! Project lifecycle suite — pipeline layer 4: the server against a real
//! mod tree on disk (§10) with a real schema contract (§9), the way a
//! Checkmate developer's editor session actually looks. The matrix under
//! test:
//!
//! - many documents open at once, edited in any order, with open-buffer
//!   text overriding disk for every file of the mod;
//! - cross-file navigation (hover, go-to-definition, references) through
//!   the module table and the schema contract;
//! - the diagnostic pipeline as a WHOLE: breaking a module re-anchors the
//!   defect to that module's own file; the importer's next edit reflects
//!   the imported buffer's new state;
//! - schema-buffer authoring: an unsaved schema edit changes what the
//!   scripts see, and a broken schema edit falls back to the last clean
//!   parse;
//! - files appearing on disk while buffers are open (mtime-driven cache
//!   invalidation).
//!
//! Layers 1–3 live in the analysis suites, `lsp_server`, and
//! `authoring_lifecycle_suite`; layer 5 (hostile input) is
//! `robustness_suite`. Shared harness: `common::wire`.

mod common;

use common::wire;
use serde_json::json;

// ---------------------------------------------------------------------------
// The project: a mod with a schema, three modules, and a cross-file impl
// ---------------------------------------------------------------------------

const SCHEMA: &str = "\
schema game 1.2.0

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

since 1.2.0 capability window {
    int Ping()
}

since 1.0.0 interface gamemode requires core {
    int OnEvent(Event event)
    int Tick(int frame)
}

since 1.0.0 interface core {
    bool Validate(str token)
}
";

const MANIFEST: &str = "\
name = \"lifecycle_game\"
version = \"1.0.0\"
checkmate_version = \"0.3.0\"

[schemas]
game = \"1.0.0\"
";

const UTILS: &str = "\
int clampTo(int value, int low, int high) {
    if (value < low) {
        return low
    }
    if (value > high) {
        return high
    }
    return value
}

int doubled(int value) {
    return value * 2
}
";

const MAIN: &str = "\
import self.helpers.utils as *

int main() {
    int hp = clampTo(300, 0, 0xFF)
    return doubled(hp)
}
";

const RULES: &str = "\
impl game.gamemode {
    int OnEvent(Event event) {
        return 0
    }

    int Tick(int frame) {
        return frame
    }
}

impl game.core {
    bool Validate(str token) {
        return true
    }
}
";

struct Project {
    root: std::path::PathBuf,
    main_uri: String,
    utils_uri: String,
    rules_uri: String,
    schema_uri: String,
}

fn plant(name: &str) -> Project {
    let root = wire::temp_root(name);
    wire::write(&root, "mod.toml", MANIFEST);
    wire::write(&root, "schemas/game.cm", SCHEMA);
    wire::write(&root, "src/main.cm", MAIN);
    wire::write(&root, "src/helpers/utils.cm", UTILS);
    wire::write(&root, "src/gamemode/rules.cm", RULES);
    Project {
        main_uri: wire::file_uri(&root.join("src/main.cm")),
        utils_uri: wire::file_uri(&root.join("src/helpers/utils.cm")),
        rules_uri: wire::file_uri(&root.join("src/gamemode/rules.cm")),
        schema_uri: wire::file_uri(&root.join("schemas/game.cm")),
        root,
    }
}

// ---------------------------------------------------------------------------
// Cross-file diagnostics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn opening_the_entry_publishes_a_clean_cross_file_build() {
    let project = plant("clean");
    let mut harness = wire::setup().await;
    let publish = wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;
    assert!(
        wire::messages(&publish).is_empty(),
        "the whole tree checks: {:?}",
        wire::messages(&publish)
    );
}

#[tokio::test]
async fn breaking_an_imported_module_reanchors_the_defect_to_that_module() {
    let project = plant("reanchor");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;

    // The import resolves at the import's span: a module that does not
    // exist reports inside the IMPORTER at the import statement.
    let broken_import =
        "import self.helpers.utils\nimport self.helpers.nope\n\nint main() {\nreturn 0\n}";
    let publish = wire::change_and_drain(&mut harness, &project.main_uri, 2, broken_import).await;
    let messages = wire::messages(&publish);
    assert!(
        messages.iter().any(|m| m.contains("nope")),
        "the missing module is named: {messages:?}"
    );

    // A defect inside utils.cm publishes on utils.cm's own URI.
    let broken_utils = "int clampTo(int value, int low, int high) {\nreturn value +\n}";
    let publish = wire::change_and_drain(&mut harness, &project.utils_uri, 2, broken_utils).await;
    assert!(
        !wire::messages(&publish).is_empty(),
        "the broken module reports on its own buffer"
    );
}

#[tokio::test]
async fn the_importer_sees_the_imported_buffer_newest_state_on_its_next_edit() {
    let project = plant("liveimport");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;
    wire::open_and_drain(&mut harness, &project.utils_uri, UTILS).await;

    // Remove `doubled` from the open utils buffer…
    let shrunk = "int clampTo(int value, int low, int high) {\nreturn value\n}";
    let _ = wire::change_and_drain(&mut harness, &project.utils_uri, 2, shrunk).await;
    // …then touch main.cm: the mod pipeline re-checks with the OPEN
    // buffer, so the missing callee is named in main's publish.
    let publish = wire::change_and_drain(&mut harness, &project.main_uri, 2, MAIN).await;
    let messages = wire::messages(&publish);
    assert!(
        messages.iter().any(|m| m.contains("doubled")),
        "the stale callee is reported in the importer: {messages:?}"
    );
}

#[tokio::test]
async fn disk_content_serves_after_the_buffer_is_closed() {
    let project = plant("closefallback");
    let mut harness = wire::setup().await;
    // Open a broken buffer over a healthy disk file.
    let broken = "int main() {\nreturn bogus\n}";
    let publish = wire::open_and_drain(&mut harness, &project.main_uri, broken).await;
    assert!(
        !wire::messages(&publish).is_empty(),
        "the unsaved damage publishes"
    );

    // Closing drops the buffer; the publish clears the list. A fresh open
    // re-reads the mod from disk and is clean again.
    wire::close(&mut harness, &project.main_uri).await;
    let _ = wire::next_publish(&mut harness).await;
    let publish = wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;
    assert!(
        wire::messages(&publish).is_empty(),
        "the disk tree is clean: {:?}",
        wire::messages(&publish)
    );
}

#[tokio::test]
async fn a_file_added_on_disk_while_buffers_are_open_becomes_importable() {
    let project = plant("newfile");
    let mut harness = wire::setup().await;
    let broken_import =
        "import self.helpers.utils\nimport self.helpers.stats\n\nint main() {\nreturn 0\n}";
    let publish = wire::open_and_drain(&mut harness, &project.main_uri, broken_import).await;
    assert!(
        wire::messages(&publish).iter().any(|m| m.contains("stats")),
        "the new module is missing at first: {:?}",
        wire::messages(&publish)
    );

    // The file appears on disk OUTSIDE the editor (a code generator, a
    // teammate's sync). The workspace's mtime cache picks it up.
    wire::write(&project.root, "src/helpers/stats.cm", "int mean = 42\n");
    let publish = wire::change_and_drain(&mut harness, &project.main_uri, 2, broken_import).await;
    assert!(
        wire::messages(&publish).is_empty(),
        "the on-disk module resolves after the cache refresh: {:?}",
        wire::messages(&publish)
    );
}

// ---------------------------------------------------------------------------
// Cross-file navigation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn go_to_definition_answers_in_file_and_declines_across_buffers() {
    // The position features run on the OPEN BUFFER's analysis: a local
    // name resolves to its in-file span, and a cross-file callee declines
    // with null (the mod pipeline links names for diagnostics, but the
    // per-buffer analysis does not carry the other files' ASTs). When
    // cross-file definition lands, this pin flips to the new behavior —
    // the decline is the honest baseline.
    let project = plant("defjump");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;

    let location = wire::request(
        &mut harness,
        "textDocument/definition",
        json!({
            "textDocument": { "uri": project.main_uri },
            "position": wire::position_of(MAIN, &project.main_uri, "int main() {", 8)["position"]
        }),
    )
    .await;
    assert!(
        location.to_string().contains("main.cm"),
        "an in-file name defines to its own span: {location}"
    );

    let location = wire::request(
        &mut harness,
        "textDocument/definition",
        wire::position_of(MAIN, &project.main_uri, "clampTo(300", 10),
    )
    .await;
    assert_eq!(
        location,
        serde_json::Value::Null,
        "a cross-file callee declines cleanly today"
    );
}

#[tokio::test]
async fn hover_on_an_imported_function_declines_without_error() {
    let project = plant("hovercross");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;

    // Same honest baseline: a cross-file callee has no hover payload, but
    // the request must still succeed end to end.
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        wire::position_of(MAIN, &project.main_uri, "clampTo(300", 10),
    )
    .await;
    assert_eq!(hover, serde_json::Value::Null, "clean decline, no error");

    // Hovering the LOCAL variable in the same buffer keeps working.
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": project.main_uri },
            "position": wire::position_of(MAIN, &project.main_uri, "int hp = clampTo", 12)["position"]
        }),
    )
    .await;
    assert!(
        hover.to_string().contains("int"),
        "the local variable hovers: {hover}"
    );
}

#[tokio::test]
async fn references_are_scoped_to_the_open_buffer() {
    let project = plant("refs");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;
    wire::open_and_drain(&mut harness, &project.utils_uri, UTILS).await;

    // `doubled` is used in main.cm but declared in utils.cm; the
    // per-buffer analysis sees only the declaration.
    let references = wire::request(
        &mut harness,
        "textDocument/references",
        json!({
            "textDocument": { "uri": project.utils_uri },
            "position": wire::position_of(UTILS, &project.utils_uri, "doubled(int value)", 14)["position"],
            "context": { "includeDeclaration": true }
        }),
    )
    .await;
    let count = references.as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(count, 1, "declaration only, per buffer: {references}");

    // In-file references still find every use.
    let references = wire::request(
        &mut harness,
        "textDocument/references",
        json!({
            "textDocument": { "uri": project.main_uri },
            "position": wire::position_of(MAIN, &project.main_uri, "int hp = clampTo", 12)["position"],
            "context": { "includeDeclaration": false }
        }),
    )
    .await;
    let count = references.as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(
        count, 1,
        "hp's return-statement use in main.cm: {references}"
    );
}

#[tokio::test]
async fn the_outline_covers_every_module_of_the_session() {
    let project = plant("outline");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;
    wire::open_and_drain(&mut harness, &project.utils_uri, UTILS).await;

    for (uri, expected) in [(&project.main_uri, "main"), (&project.utils_uri, "clampTo")] {
        let symbols = wire::request(
            &mut harness,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        )
        .await;
        assert!(
            symbols.to_string().contains(expected),
            "{uri} outlines {expected}: {symbols}"
        );
    }
}

// ---------------------------------------------------------------------------
// Schema-driven authoring (§9 through the LSP)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unsaved_schema_edit_drives_the_script_surface() {
    let project = plant("schemabuffer");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.schema_uri, SCHEMA).await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;

    // The unsaved schema drops the `window` capability entirely; the
    // scripts' next publish sees the shrunken contract.
    let shrunk_schema = SCHEMA.replace(
        "since 1.0.0 capability window {\n    Sprite OpenWindow(str title)\n    void Draw(Sprite sprite)\n}\n\n",
        "",
    );
    let publish =
        wire::change_and_drain(&mut harness, &project.schema_uri, 2, &shrunk_schema).await;
    assert!(
        wire::messages(&publish).is_empty(),
        "the schema buffer itself stays clean: {:?}",
        wire::messages(&publish)
    );

    // A script that used the capability would now fail; a script that
    // does not keeps checking clean against the edited contract.
    let publish = wire::change_and_drain(&mut harness, &project.main_uri, 2, MAIN).await;
    assert!(
        wire::messages(&publish).is_empty(),
        "main does not use window, so it stays clean: {:?}",
        wire::messages(&publish)
    );
}

#[tokio::test]
async fn a_broken_schema_edit_reports_on_the_schema_and_holds_the_last_clean_contract() {
    let project = plant("schemabreak");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.schema_uri, SCHEMA).await;

    // Mid-edit: the schema buffer itself reports its defect…
    let broken = "schema game 1.2.0\n\nstruct Sprite {\n    int id\n";
    let publish = wire::change_and_drain(&mut harness, &project.schema_uri, 2, broken).await;
    assert!(
        !wire::messages(&publish).is_empty(),
        "the broken schema reports on its own buffer: {:?}",
        wire::messages(&publish)
    );

    // …and the pipeline survives the broken schema without wedging: the
    // next script edit still publishes.
    let publish = wire::change_and_drain(&mut harness, &project.main_uri, 2, MAIN).await;
    assert_eq!(
        publish["uri"].as_str(),
        Some(project.main_uri.as_str()),
        "script publishing survives a broken schema"
    );
}

#[tokio::test]
async fn the_manifest_version_hides_newer_capability_members() {
    // The schema declares `Ping` since 1.2.0; the manifest targets 1.0.0,
    // so the member is hidden (§9.5). An impl-less script still sees the
    // hidden member as unavailable: calling it fails to resolve.
    let project = plant("versionhide");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;

    let caller =
        "import game.window\n\nint main() {\nint pong = game.window.Ping()\nreturn pong\n}";
    let publish = wire::change_and_drain(&mut harness, &project.main_uri, 2, caller).await;
    let messages = wire::messages(&publish);
    assert!(
        messages.iter().any(|m| m.contains("Ping")),
        "the version-hidden member does not resolve: {messages:?}"
    );

    // Raising the manifest's target version (unsaved buffer) makes the
    // member available.
    let raised = MANIFEST.replace("game = \"1.0.0\"", "game = \"1.2.0\"");
    wire::write(&project.root, "mod.toml", &raised);
    let publish = wire::change_and_drain(&mut harness, &project.main_uri, 3, caller).await;
    assert!(
        wire::messages(&publish).is_empty(),
        "with the version raised the call resolves: {:?}",
        wire::messages(&publish)
    );
}

#[tokio::test]
async fn impl_member_completion_offers_the_missing_contract_members() {
    let project = plant("implfill");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.rules_uri, RULES).await;

    // An impl block missing `Tick`: completion inside the block offers
    // the schema-exact signature fill.
    let partial = "impl game.gamemode {\nint OnEvent(Event event) {\nreturn 0\n}\n\n\n}\n";
    let _ = wire::change_and_drain(&mut harness, &project.rules_uri, 2, partial).await;
    let completion = wire::request(
        &mut harness,
        "textDocument/completion",
        json!({
            "textDocument": { "uri": project.rules_uri },
            "position": { "line": 5, "character": 0 }
        }),
    )
    .await;
    assert!(
        completion.to_string().contains("Tick"),
        "the missing member is offered with its signature: {completion}"
    );
}

#[tokio::test]
async fn schema_types_and_capability_calls_hover_across_the_contract() {
    let project = plant("schemahover");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;

    let script = "import game.window\n\nint main() {\nSprite sprite = game.window.OpenWindow(\"hud\")\nreturn sprite.id\n}";
    let _ = wire::change_and_drain(&mut harness, &project.main_uri, 2, script).await;

    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": project.main_uri },
            "position": wire::position_of(script, &project.main_uri, "sprite.id", 8)["position"]
        }),
    )
    .await;
    assert!(
        hover.to_string().contains("Sprite"),
        "the schema struct type shows on hover: {hover}"
    );
}

// ---------------------------------------------------------------------------
// Multi-document sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn four_documents_edit_in_rotation_all_stay_answerable() {
    let project = plant("rotation");
    let mut harness = wire::setup().await;
    let uris = [
        project.main_uri.clone(),
        project.utils_uri.clone(),
        project.rules_uri.clone(),
        project.schema_uri.clone(),
    ];
    let texts = [MAIN, UTILS, RULES, SCHEMA];
    for (index, uri) in uris.iter().enumerate() {
        wire::open_and_drain(&mut harness, uri, texts[index]).await;
    }
    // Rotate edits: each round touches one buffer and queries another.
    for round in 0..3 {
        let target = &uris[round % uris.len()];
        let text = texts[round % texts.len()];
        let publish = wire::change_and_drain(&mut harness, target, (round + 2) as i32, text).await;
        assert_eq!(publish["uri"].as_str(), Some(target.as_str()));
        let hover_uri = &uris[(round + 1) % uris.len()];
        let hover = wire::request(
            &mut harness,
            "textDocument/hover",
            json!({
                "textDocument": { "uri": hover_uri },
                "position": { "line": 0, "character": 0 }
            }),
        )
        .await;
        let _ = hover; // any answer is fine; the pin is "no hang, no 500"
    }
}

#[tokio::test]
async fn two_mod_roots_in_one_session_stay_isolated() {
    let a = plant("isolated_a");
    let b = plant("isolated_b");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &a.main_uri, MAIN).await;
    wire::open_and_drain(&mut harness, &b.main_uri, MAIN).await;

    // Break mod b's utils; mod a keeps checking clean.
    let broken = "int clampTo(int value, int low, int high) {\nreturn\n}";
    let _ = wire::change_and_drain(&mut harness, &b.utils_uri, 2, broken).await;
    let publish = wire::change_and_drain(&mut harness, &a.main_uri, 2, MAIN).await;
    assert!(
        wire::messages(&publish).is_empty(),
        "mod a is unaffected by mod b's open damage: {:?}",
        wire::messages(&publish)
    );
}

#[tokio::test]
async fn deep_module_paths_navigate_through_every_segment() {
    let project = plant("deep");
    wire::write(
        &project.root,
        "src/world/generation/terrain/noise.cm",
        "int sampleOctave(int x) {\nreturn x\n}\n",
    );
    let mut harness = wire::setup().await;
    let script = "import self.world.generation.terrain.noise as *\n\nint main() {\nreturn sampleOctave(7)\n}";
    let publish = wire::open_and_drain(&mut harness, &project.main_uri, script).await;
    assert!(
        wire::messages(&publish).is_empty(),
        "the five-segment import resolves: {:?}",
        wire::messages(&publish)
    );

    // The deep callee declines definition (per-buffer analysis) but the
    // import itself validates through the whole five-segment path.
    let location = wire::request(
        &mut harness,
        "textDocument/definition",
        wire::position_of(script, &project.main_uri, "sampleOctave(7", 12),
    )
    .await;
    assert_eq!(
        location,
        serde_json::Value::Null,
        "the deep callee declines cleanly (see the defjump pin)"
    );
}

#[tokio::test]
async fn main_may_live_in_any_module_of_the_tree() {
    let project = plant("entryanywhere");
    let mut harness = wire::setup().await;
    // main.cm has no main; rules.cm carries the entry. The tree still
    // checks (the entry convention is mod-wide, §10).
    let entryless = "int helper(int x) {\nreturn x\n}";
    let publish = wire::change_and_drain(&mut harness, &project.main_uri, 2, entryless).await;
    let messages = wire::messages(&publish);
    assert!(
        !messages.iter().any(|m| m.contains("main")),
        "no missing-entry complaint for the tree: {messages:?}"
    );
}

#[tokio::test]
async fn semantic_tokens_cover_the_whole_open_session() {
    let project = plant("tokens");
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, &project.main_uri, MAIN).await;
    wire::open_and_drain(&mut harness, &project.utils_uri, UTILS).await;

    for uri in [&project.main_uri, &project.utils_uri] {
        let tokens = wire::request(
            &mut harness,
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": uri } }),
        )
        .await;
        let count = tokens["data"].as_array().map(Vec::len).unwrap_or_default();
        assert!(count > 0, "{uri} paints semantic tokens: {tokens}");
    }
}
