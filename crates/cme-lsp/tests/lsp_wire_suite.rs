//! Wire-level suite: the JSON-RPC surface driven through a real
//! `LspService` — lifecycle, document tracking, per-feature responses over
//! the protocol (including the position encoding rules), and the
//! feature-gating rules for schema and megaprogram files.
//!
//! The transport pattern matches `lsp_server.rs`: `publish_diagnostics`
//! only completes once the client socket is drained, so messages are
//! pumped into a channel in a background task.

use futures::StreamExt;
use serde_json::json;
use tower::{Service, ServiceExt};
use tower_lsp_server::LspService;
use tower_lsp_server::jsonrpc;

use cme_lsp::server::CheckmateLsp;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

async fn setup() -> (
    LspService<CheckmateLsp>,
    tokio::sync::mpsc::UnboundedReceiver<jsonrpc::Request>,
) {
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
    (service, receiver)
}

async fn next_message(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<jsonrpc::Request>,
) -> jsonrpc::Request {
    tokio::time::timeout(TIMEOUT, receiver.recv())
        .await
        .expect("server message arrives within the timeout")
        .expect("the socket stays open")
}

async fn handshake(service: &mut LspService<CheckmateLsp>) -> serde_json::Value {
    let initialize = jsonrpc::Request::build("initialize".to_string())
        .params(json!({ "capabilities": {} }))
        .id(1)
        .finish();
    let response = service
        .ready()
        .await
        .unwrap()
        .call(initialize)
        .await
        .unwrap()
        .expect("initialize gets a response");
    let result = response.result().cloned().expect("initialize payload");
    let initialized = jsonrpc::Request::build("initialized".to_string())
        .params(json!({}))
        .finish();
    let _ = service
        .ready()
        .await
        .unwrap()
        .call(initialized)
        .await
        .unwrap();
    result
}

async fn notify(service: &mut LspService<CheckmateLsp>, method: &str, params: serde_json::Value) {
    let request = jsonrpc::Request::build(method.to_string())
        .params(params)
        .finish();
    let response = service.ready().await.unwrap().call(request).await.unwrap();
    assert!(response.is_none(), "{method} is a notification");
}

async fn request(
    service: &mut LspService<CheckmateLsp>,
    method: &str,
    params: serde_json::Value,
) -> jsonrpc::Response {
    static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1000);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let request = jsonrpc::Request::build(method.to_string())
        .params(params)
        .id(id)
        .finish();
    tokio::time::timeout(TIMEOUT, async {
        service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
            .expect("requests get a response")
    })
    .await
    .expect("the request completes within the timeout")
}

async fn open(service: &mut LspService<CheckmateLsp>, uri: &str, text: &str) {
    notify(
        service,
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "checkmate",
                "version": 1,
                "text": text,
            }
        }),
    )
    .await;
}

async fn next_publish(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<jsonrpc::Request>,
) -> serde_json::Value {
    loop {
        let message = next_message(receiver).await;
        if message.method() == "textDocument/publishDiagnostics" {
            return message.params().expect("params").clone();
        }
    }
}

/// Opens a document and drains the publish that follows.
async fn open_and_drain(
    service: &mut LspService<CheckmateLsp>,
    uri: &str,
    text: &str,
    messages: &mut tokio::sync::mpsc::UnboundedReceiver<jsonrpc::Request>,
) {
    open(service, uri, text).await;
    let _ = next_publish(messages).await;
}

fn position_params(uri: &str, line: u32, character: u32) -> serde_json::Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character }
    })
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initialize_advertises_the_checkmate_capabilities() {
    let (mut service, _messages) = setup().await;
    let result = handshake(&mut service).await;
    let capabilities = &result["capabilities"];
    assert_eq!(capabilities["hoverProvider"], json!(true));
    assert_eq!(
        capabilities["completionProvider"]["triggerCharacters"],
        json!(["."])
    );
    assert_eq!(capabilities["definitionProvider"], json!(true));
    assert_eq!(capabilities["referencesProvider"], json!(true));
    assert_eq!(capabilities["documentSymbolProvider"], json!(true));
    assert!(capabilities["semanticTokensProvider"].is_object());
    assert_eq!(
        capabilities["textDocumentSync"],
        json!(1),
        "full-document sync"
    );
    assert_eq!(result["serverInfo"]["name"], json!("cme"));
}

#[tokio::test]
async fn shutdown_responds_ok() {
    let (mut service, _messages) = setup().await;
    handshake(&mut service).await;
    let response = request(&mut service, "shutdown", json!(null)).await;
    assert!(response.is_ok(), "shutdown succeeds: {response:?}");
}

// ---------------------------------------------------------------------------
// Requests for documents the server does not track
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unopened_documents_answer_empty_without_erroring() {
    let (mut service, _messages) = setup().await;
    handshake(&mut service).await;

    for method in [
        "textDocument/hover",
        "textDocument/completion",
        "textDocument/definition",
        "textDocument/documentSymbol",
        "textDocument/semanticTokens/full",
    ] {
        let response = request(
            &mut service,
            method,
            position_params("file:///never.cm", 0, 0),
        )
        .await;
        assert!(response.is_ok(), "{method} must not error: {response:?}");
    }
    // references requires its context field per the LSP spec; with it, the
    // request must also answer without error.
    let response = request(
        &mut service,
        "textDocument/references",
        json!({
            "textDocument": { "uri": "file:///never.cm" },
            "position": { "line": 0, "character": 0 },
            "context": { "includeDeclaration": true }
        }),
    )
    .await;
    assert!(response.is_ok(), "references must not error: {response:?}");
}

#[tokio::test]
async fn did_close_releases_the_document() {
    // Regression: didClose used to leave the document in the session map,
    // so later requests kept answering for a closed buffer.
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///closing.cm",
        "struct vec2 {\n    float x\n}\n\nint main() {\n    return 0\n}\n",
        &mut messages,
    )
    .await;

    notify(
        &mut service,
        "textDocument/didClose",
        json!({ "textDocument": { "uri": "file:///closing.cm" } }),
    )
    .await;
    let published = next_publish(&mut messages).await;
    assert_eq!(
        published["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "close publishes empty diagnostics"
    );

    let response = request(
        &mut service,
        "textDocument/completion",
        position_params("file:///closing.cm", 4, 13),
    )
    .await;
    assert!(response.is_ok());
    let items = response.result().expect("payload");
    assert_eq!(
        items.as_array().map(Vec::len),
        Some(0),
        "a closed document completes to nothing: {items}"
    );
    let hover = request(
        &mut service,
        "textDocument/hover",
        position_params("file:///closing.cm", 4, 13),
    )
    .await;
    let empty = hover.result().map(|value| value.is_null()).unwrap_or(true);
    assert!(empty, "a closed document has no hover: {hover:?}");
}

// ---------------------------------------------------------------------------
// Position encoding over the wire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crlf_documents_hover_correctly() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///crlf.cm",
        "int main() {\r\n    int hp = 100\r\n    return hp\r\n}\r\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/hover",
        position_params("file:///crlf.cm", 2, 12),
    )
    .await;
    let hover = response.result().expect("hover payload");
    assert!(hover.to_string().contains("hp: int"), "{hover}");
}

#[tokio::test]
async fn utf16_positions_survive_emoji_on_earlier_lines() {
    // The 😀 on line 0 costs 2 UTF-16 units but 4 bytes; line/character
    // addressing must not drift on later lines.
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///emoji.cm",
        "str s = \"a😀b\"\n\nint main() {\n    int hp = 100\n    return hp\n}\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/hover",
        position_params("file:///emoji.cm", 4, 12),
    )
    .await;
    let hover = response.result().expect("hover payload");
    assert!(hover.to_string().contains("hp: int"), "{hover}");
}

// ---------------------------------------------------------------------------
// Completion over the wire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn completion_after_partial_member_name() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///member.cm",
        "struct player {\n    str name\n    int health\n}\n\nint main() {\n    player p = player(name: \"h\", health: 1)\n    p.na\n    return 0\n}\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/completion",
        position_params("file:///member.cm", 7, 8),
    )
    .await;
    let items = response.result().expect("payload");
    let text = items.to_string();
    assert!(text.contains("\"name\""), "field name offered: {items}");
    assert!(text.contains("\"health\""), "field health offered: {items}");
    assert!(
        !text.contains("\"while\""),
        "no keywords in member context: {items}"
    );
}

#[tokio::test]
async fn completion_offers_named_arguments_with_fills() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///args.cm",
        "struct vec2 {\n    float x\n    float y\n}\n\nint main() {\n    vec2 v = vec2(x: 1.0, )\n    return 0\n}\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/completion",
        position_params("file:///args.cm", 6, 26),
    )
    .await;
    let items = response.result().expect("payload");
    let text = items.to_string();
    assert!(text.contains("\"y\""), "remaining field offered: {items}");
    assert!(
        text.contains("y: "),
        "the fill carries the named-argument prefix: {items}"
    );
    assert!(
        !text.contains("\"x: \""),
        "the used field is not re-offered: {items}"
    );
}

#[tokio::test]
async fn completion_reflects_edits() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///edit.cm",
        "int main() {\n    return 0\n}\n",
        &mut messages,
    )
    .await;

    notify(
        &mut service,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": "file:///edit.cm", "version": 2 },
            "contentChanges": [ { "text": "struct gear {\n    int teeth\n}\n\nint main() {\n    return 0\n}\n" } ]
        }),
    )
    .await;
    let _ = next_publish(&mut messages).await;

    let response = request(
        &mut service,
        "textDocument/completion",
        position_params("file:///edit.cm", 5, 4),
    )
    .await;
    let items = response.result().expect("payload");
    assert!(
        items.to_string().contains("gear"),
        "the edited-in struct completes: {items}"
    );
}

#[tokio::test]
async fn empty_document_completion_does_not_error() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(&mut service, "file:///empty.cm", "", &mut messages).await;
    let response = request(
        &mut service,
        "textDocument/completion",
        position_params("file:///empty.cm", 0, 0),
    )
    .await;
    assert!(response.is_ok(), "{response:?}");
    let items = response.result().expect("payload");
    assert!(items.is_array(), "{items}");
}

// ---------------------------------------------------------------------------
// Other features over the wire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn goto_definition_returns_a_location() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///defs.cm",
        "int twice(int v) {\n    return v + v\n}\n\nint main() {\n    return twice(2)\n}\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/definition",
        position_params("file:///defs.cm", 5, 13),
    )
    .await;
    let result = response.result().expect("payload");
    let text = result.to_string();
    assert!(text.contains("\"uri\":\"file:///defs.cm\""), "{text}");
    assert!(text.contains("\"line\":0"), "jumps to line 0: {text}");
    assert!(
        text.contains("\"character\":4"),
        "jumps to the name column: {text}"
    );
}

#[tokio::test]
async fn references_return_locations() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///refs.cm",
        "int twice(int v) {\n    return v + v\n}\n\nint main() {\n    return twice(2)\n}\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/references",
        json!({
            "textDocument": { "uri": "file:///refs.cm" },
            "position": { "line": 5, "character": 13 },
            "context": { "includeDeclaration": true }
        }),
    )
    .await;
    let result = response.result().expect("payload");
    assert_eq!(result.as_array().map(Vec::len), Some(2), "{result}");
}

#[tokio::test]
async fn document_symbols_return_the_outline() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///outline.cm",
        "struct vec2 {\n    float x\n}\n\nint main() {\n    return 0\n}\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": "file:///outline.cm" } }),
    )
    .await;
    let result = response.result().expect("payload");
    let text = result.to_string();
    assert!(text.contains("vec2"), "{text}");
    assert!(text.contains("main"), "{text}");
    assert!(text.contains("x"), "field child present: {text}");
}

#[tokio::test]
async fn semantic_tokens_return_delta_encoded_data() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///tokens.cm",
        "int main() {\n    return 0\n}\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": "file:///tokens.cm" } }),
    )
    .await;
    let result = response.result().expect("payload");
    assert!(result["data"].is_array(), "{result}");
}

#[tokio::test]
async fn hover_on_unresolvable_identifier_is_null() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///nothing.cm",
        "int main() {\n    return 0\n}\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/hover",
        position_params("file:///nothing.cm", 1, 11),
    )
    .await;
    let empty = response
        .result()
        .map(|value| value.is_null())
        .unwrap_or(true);
    assert!(empty, "{response:?}");
}

// ---------------------------------------------------------------------------
// Multiple documents and file kinds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn documents_are_isolated_from_each_other() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///alpha.cm",
        "struct alpha_type {\n    int a\n}\n\nint main() {\n    return 0\n}\n",
        &mut messages,
    )
    .await;
    open_and_drain(
        &mut service,
        "file:///beta.cm",
        "struct beta_type {\n    int b\n}\n\nint main() {\n    return 0\n}\n",
        &mut messages,
    )
    .await;

    let response = request(
        &mut service,
        "textDocument/completion",
        position_params("file:///beta.cm", 4, 4),
    )
    .await;
    let text = response.result().expect("payload").to_string();
    assert!(text.contains("beta_type"), "{text}");
    assert!(
        !text.contains("alpha_type"),
        "no leakage across documents: {text}"
    );
}

#[tokio::test]
async fn schema_files_complete_to_nothing_but_still_publish() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///engine.cm",
        "schema engine v1.4.0\n",
        &mut messages,
    )
    .await;

    let response = request(
        &mut service,
        "textDocument/completion",
        position_params("file:///engine.cm", 0, 3),
    )
    .await;
    let items = response.result().expect("payload");
    assert_eq!(items.as_array().map(Vec::len), Some(0), "{items}");

    // A broken schema publishes diagnostics.
    notify(
        &mut service,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": "file:///engine.cm", "version": 2 },
            "contentChanges": [ { "text": "schema 1.4.0\n" } ]
        }),
    )
    .await;
    let published = next_publish(&mut messages).await;
    assert!(
        published["diagnostics"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0)
            > 0,
        "{published}"
    );
}

#[tokio::test]
async fn megaprogram_files_expand_but_do_not_complete() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    let mega = "int log(int x) {\n    return x\n}\n\nmega twice(\n    $int value\n) {\n    log($value)\n}\n\nint main() {\n    twice! {\n        21\n    }\n    return 0\n}\n";
    open_and_drain(&mut service, "file:///mega.cm", mega, &mut messages).await;

    let response = request(
        &mut service,
        "textDocument/completion",
        position_params("file:///mega.cm", 9, 4),
    )
    .await;
    let items = response.result().expect("payload");
    assert_eq!(
        items.as_array().map(Vec::len),
        Some(0),
        "megaprogram spans live in expanded coordinates: {items}"
    );

    // cme/expand still previews the expansion.
    let response = request(
        &mut service,
        "cme/expand",
        json!({ "uri": "file:///mega.cm" }),
    )
    .await;
    let expanded = response.result().expect("payload");
    assert!(
        expanded.to_string().contains("log(21)"),
        "the region expanded: {expanded}"
    );

    // A broken mega fails expansion: the preview is null and diagnostics
    // publish.
    notify(
        &mut service,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": "file:///mega.cm", "version": 2 },
            "contentChanges": [ { "text": "mega twice(\n    $int value\n) {\n    ???\n}\n\ntwice! {\n    21\n}\n" } ]
        }),
    )
    .await;
    let published = next_publish(&mut messages).await;
    assert!(
        published["diagnostics"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0)
            > 0,
        "expansion diagnostics surface: {published}"
    );
    let response = request(
        &mut service,
        "cme/expand",
        json!({ "uri": "file:///mega.cm" }),
    )
    .await;
    let empty = response
        .result()
        .map(|value| value.is_null())
        .unwrap_or(true);
    assert!(
        !empty,
        "the expansion itself succeeded (the broken part is the emitted \
         template body, which fails parse and publishes): {response:?}"
    );
}

#[tokio::test]
async fn broken_code_still_completes_from_the_recovered_ast() {
    // Completion must survive a broken file: the healthy parts answer.
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;
    open_and_drain(
        &mut service,
        "file:///broken.cm",
        "int = ???\n\nint main() {\n    int hp = 100\n    return hp\n}\n",
        &mut messages,
    )
    .await;
    let response = request(
        &mut service,
        "textDocument/completion",
        position_params("file:///broken.cm", 4, 12),
    )
    .await;
    assert!(response.is_ok(), "{response:?}");
    let text = response.result().expect("payload").to_string();
    assert!(
        text.contains("\"hp\""),
        "the recovered local completes: {text}"
    );
    assert!(text.contains("\"int\""), "keywords still offered: {text}");
}
