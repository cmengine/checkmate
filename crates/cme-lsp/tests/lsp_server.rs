//! In-process tests for the language server: the JSON-RPC surface is
//! driven through a real `LspService`, so initialize → didOpen → feature
//! request → publishDiagnostics all run exactly as they would over stdio,
//! without spawning a process.
//!
//! One detail mirrors the production transport: `Client.publish_diagnostics`
//! only completes once the client socket is being drained, so each test
//! pumps socket messages into a channel in a background task, exactly like
//! the stdio transport does.

use futures::StreamExt;
use serde_json::json;
use tower::{Service, ServiceExt};
use tower_lsp_server::LspService;
use tower_lsp_server::jsonrpc;

use cme_lsp::server::CheckmateLsp;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

const BROKEN: &str = "int main() {\n    int hp = tr\n    return hp\n}\n";
const FIXED: &str = "int main() {\n    int hp = 100\n    return hp\n}\n";

/// Builds the service (with `cme/expand` registered, as `run_stdio` does)
/// plus a receiver of the server-to-client messages.
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

/// The next server-to-client message, failing the test instead of hanging.
async fn next_message(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<jsonrpc::Request>,
) -> jsonrpc::Request {
    tokio::time::timeout(TIMEOUT, receiver.recv())
        .await
        .expect("server message arrives within the timeout")
        .expect("the socket stays open")
}

async fn handshake(service: &mut LspService<CheckmateLsp>) {
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
    assert!(response.is_ok(), "initialize must succeed: {response:?}");

    let initialized = jsonrpc::Request::build("initialized".to_string())
        .params(json!({}))
        .finish();
    let response = service
        .ready()
        .await
        .unwrap()
        .call(initialized)
        .await
        .unwrap();
    assert!(response.is_none(), "initialized is a notification");
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
    static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(100);
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

/// The next `textDocument/publishDiagnostics` message, skipping progress
/// and log noise (`initialized` races editor notifications).
async fn next_publish(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<jsonrpc::Request>,
) -> jsonrpc::Request {
    loop {
        let message = next_message(receiver).await;
        if message.method() == "textDocument/publishDiagnostics" {
            return message;
        }
    }
}

#[tokio::test]
async fn open_publishes_diagnostics_and_edits_republish() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;

    open(&mut service, "file:///broken.cm", BROKEN).await;

    // First server-to-client publish: diagnostics for the opened file,
    // carrying one error anchored on the type mismatch.
    let published = next_publish(&mut messages).await;
    assert_eq!(published.method(), "textDocument/publishDiagnostics");
    let params = published.params().expect("params");
    let diagnostics = &params["diagnostics"];
    assert_eq!(
        diagnostics.as_array().map(Vec::len),
        Some(1),
        "one type error: {diagnostics}"
    );
    assert_eq!(diagnostics[0]["range"]["start"]["line"], 1);

    // An edit that fixes the code publishes an empty list.
    notify(
        &mut service,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": "file:///broken.cm", "version": 2 },
            "contentChanges": [ { "text": FIXED } ]
        }),
    )
    .await;
    let published = next_publish(&mut messages).await;
    let params = published.params().expect("params");
    assert_eq!(
        params["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "the fix clears diagnostics: {params}"
    );
}

#[tokio::test]
async fn hover_and_completion_over_the_wire() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;

    let source = "struct vec2 {\n    float x\n}\n\nint main() {\n    infer v = vec2(x: 1.0)\n    return 0\n}\n";
    open(&mut service, "file:///hover.cm", source).await;
    let _ = next_publish(&mut messages).await; // drain the open publish

    // Hover over the `v` of `infer v = ...` (line 5, the name at char 10).
    let response = request(
        &mut service,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": "file:///hover.cm" },
            "position": { "line": 5, "character": 10 }
        }),
    )
    .await;
    assert!(response.is_ok(), "hover succeeds: {response:?}");
    let hover = response.result().expect("hover payload");
    assert!(
        hover.to_string().contains("vec2"),
        "hover shows the crystallized type: {hover}"
    );

    // Completion right after `v.` — send the dot, then ask.
    let source_with_dot = "struct vec2 {\n    float x\n}\n\nint main() {\n    infer v = vec2(x: 1.0)\n    return v.\n}\n";
    notify(
        &mut service,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": "file:///hover.cm", "version": 2 },
            "contentChanges": [ { "text": source_with_dot } ]
        }),
    )
    .await;
    let _ = next_publish(&mut messages).await; // drain the change publish

    let response = request(
        &mut service,
        "textDocument/completion",
        json!({
            "textDocument": { "uri": "file:///hover.cm" },
            "position": { "line": 6, "character": 14 }
        }),
    )
    .await;
    assert!(response.is_ok(), "completion succeeds: {response:?}");
    let items = response.result().expect("completion payload");
    assert!(
        items.to_string().contains("\"x\""),
        "field x is offered after `v.`: {items}"
    );
}

#[tokio::test]
async fn expand_preview_returns_expanded_text() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;

    let mega_source = "int log(int x) {\n    return x\n}\n\nmega twice(\n    $int value\n) {\n    log($value)\n}\n\nint main() {\n    twice! {\n        21\n    }\n    return 0\n}\n";
    open(&mut service, "file:///mega.cm", mega_source).await;
    let _ = next_publish(&mut messages).await; // drain the open publish

    let response = request(
        &mut service,
        "cme/expand",
        json!({ "uri": "file:///mega.cm" }),
    )
    .await;
    assert!(response.is_ok(), "cme/expand succeeds: {response:?}");
    let expanded = response.result().expect("expanded payload");
    assert!(
        expanded.to_string().contains("log(21)"),
        "the region was expanded into generated code: {expanded}"
    );

    // Plain files have nothing to expand (JSON null — the serialized None).
    open(&mut service, "file:///plain.cm", BROKEN).await;
    let _ = next_publish(&mut messages).await; // drain the open publish
    let response = request(
        &mut service,
        "cme/expand",
        json!({ "uri": "file:///plain.cm" }),
    )
    .await;
    let empty = response
        .result()
        .map(|value| value.is_null())
        .unwrap_or(true);
    assert!(empty, "no megaprogram, no preview: {response:?}");
}

#[tokio::test]
async fn schema_files_get_diagnostics_and_their_own_authoring_features() {
    let (mut service, mut messages) = setup().await;
    handshake(&mut service).await;

    open(&mut service, "file:///engine.cm", "schema engine 1.4.0\n").await;
    let published = next_publish(&mut messages).await;
    let params = published.params().expect("params");
    assert_eq!(
        params["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "a clean schema publishes nothing: {params}"
    );

    // Hover over the namespace identifier: the §9 document feature set
    // answers with the header.
    let response = request(
        &mut service,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": "file:///engine.cm" },
            "position": { "line": 0, "character": 10 }
        }),
    )
    .await;
    assert!(response.is_ok(), "hover succeeds: {response:?}");
    let hover = response.result().expect("hover payload");
    assert!(
        hover.to_string().contains("engine") && hover.to_string().contains("1.4.0"),
        "schema files carry their own hover: {hover}"
    );

    // Completion at the top level offers the declaration keywords.
    let response = request(
        &mut service,
        "textDocument/completion",
        json!({
            "textDocument": { "uri": "file:///engine.cm" },
            "position": { "line": 1, "character": 0 }
        }),
    )
    .await;
    assert!(response.is_ok(), "completion succeeds: {response:?}");
    let items = response.result().expect("completion payload");
    let offered = items.to_string();
    assert!(
        offered.contains("capability") && offered.contains("interface"),
        "the §9 declaration keywords complete at the top level: {offered}"
    );
}
