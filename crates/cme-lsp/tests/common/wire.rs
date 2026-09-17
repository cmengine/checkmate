//! Shared in-process wire harness for the lifecycle suites: a real
//! `LspService` driving the server exactly like the stdio transport, plus
//! throwaway on-disk mod trees for project-level scenarios.
//!
//! The pipeline layers this harness serves:
//!
//! 1. analysis features (unit-level, see the other suites' `common`);
//! 2. the wire lifecycle itself (open → change → feature → close);
//! 3. typing simulation — author a real program edit by edit;
//! 4. project lifecycle — a mod tree + schema on disk, many buffers;
//! 5. robustness — protocol misuse and hostile text.
//!
//! Layers 1–2 are covered by the existing suites; the lifecycle suites
//! build layers 3–5 on top of the helpers here.

#![allow(dead_code)]

use futures::StreamExt;
use serde_json::json;
use tower::{Service, ServiceExt};
use tower_lsp_server::LspService;
use tower_lsp_server::jsonrpc;

use cme_lsp::server::CheckmateLsp;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub struct Harness {
    service: LspService<CheckmateLsp>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<jsonrpc::Request>,
    next_id: i64,
}

/// Builds the service with `cme/expand` registered (as `run_stdio` does),
/// performs the initialize/initialized handshake, and returns the harness
/// pumping server-to-client messages into a channel.
pub async fn setup() -> Harness {
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
    let mut harness = Harness {
        service,
        receiver,
        next_id: 1,
    };
    let response = request_raw(&mut harness, "initialize", json!({ "capabilities": {} }))
        .await
        .expect("initialize gets a response");
    assert!(response.is_ok(), "initialize must succeed: {response:?}");
    notify(&mut harness, "initialized", json!({})).await;
    harness
}

/// An uninitialized harness for robustness tests: no handshake ran.
pub async fn setup_raw() -> Harness {
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
    Harness {
        service,
        receiver,
        next_id: 1,
    }
}

impl Harness {
    pub fn receiver(&mut self) -> &mut tokio::sync::mpsc::UnboundedReceiver<jsonrpc::Request> {
        &mut self.receiver
    }

    /// Direct service access for tests that need to shape raw requests
    /// (string ids, custom methods) themselves.
    pub fn service(&mut self) -> &mut LspService<CheckmateLsp> {
        &mut self.service
    }
}

pub async fn notify(harness: &mut Harness, method: &str, params: serde_json::Value) {
    let request = jsonrpc::Request::build(method.to_string())
        .params(params)
        .finish();
    let response = harness
        .service
        .ready()
        .await
        .unwrap()
        .call(request)
        .await
        .unwrap();
    assert!(response.is_none(), "{method} is a notification");
}

async fn request_raw(
    harness: &mut Harness,
    method: &str,
    params: serde_json::Value,
) -> Option<jsonrpc::Response> {
    let id = harness.next_id;
    harness.next_id += 1;
    let request = jsonrpc::Request::build(method.to_string())
        .params(params)
        .id(id)
        .finish();
    tokio::time::timeout(TIMEOUT, async {
        harness
            .service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
    })
    .await
    .expect("the request completes within the timeout")
}

/// A request that must succeed: returns the `result` payload.
pub async fn request(
    harness: &mut Harness,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    request_raw(harness, method, params)
        .await
        .expect("requests get a response")
        .result()
        .cloned()
        .expect("result payload")
}

/// A request whose error/success split the test wants to inspect.
pub async fn try_request(
    harness: &mut Harness,
    method: &str,
    params: serde_json::Value,
) -> jsonrpc::Response {
    request_raw(harness, method, params)
        .await
        .expect("requests get a response")
}

pub async fn open(harness: &mut Harness, uri: &str, text: &str) {
    open_versioned(harness, uri, text, 1).await;
}

pub async fn open_versioned(harness: &mut Harness, uri: &str, text: &str, version: i32) {
    notify(
        harness,
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "checkmate",
                "version": version,
                "text": text,
            }
        }),
    )
    .await;
}

pub async fn change(harness: &mut Harness, uri: &str, version: i32, text: &str) {
    notify(
        harness,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": version },
            "contentChanges": [ { "text": text } ]
        }),
    )
    .await;
}

pub async fn close(harness: &mut Harness, uri: &str) {
    notify(
        harness,
        "textDocument/didClose",
        json!({ "textDocument": { "uri": uri } }),
    )
    .await;
}

/// The next `textDocument/publishDiagnostics`, skipping progress and log
/// noise.
pub async fn next_publish(harness: &mut Harness) -> serde_json::Value {
    loop {
        let message = tokio::time::timeout(TIMEOUT, harness.receiver.recv())
            .await
            .expect("message within the timeout")
            .expect("the socket stays open");
        if message.method() == "textDocument/publishDiagnostics" {
            return message.params().expect("params").clone();
        }
    }
}

/// Opens a buffer and returns its first publish, asserting it targets the
/// opened URI.
pub async fn open_and_drain(harness: &mut Harness, uri: &str, text: &str) -> serde_json::Value {
    open(harness, uri, text).await;
    let publish = next_publish(harness).await;
    assert_eq!(
        publish["uri"].as_str(),
        Some(uri),
        "the right buffer publishes"
    );
    publish
}

/// Applies a full-text change and returns the publish for that URI.
pub async fn change_and_drain(
    harness: &mut Harness,
    uri: &str,
    version: i32,
    text: &str,
) -> serde_json::Value {
    change(harness, uri, version, text).await;
    let publish = next_publish(harness).await;
    assert_eq!(
        publish["uri"].as_str(),
        Some(uri),
        "the right buffer publishes"
    );
    publish
}

/// The diagnostic messages of a publish payload, as plain strings.
pub fn messages(publish: &serde_json::Value) -> Vec<String> {
    publish["diagnostics"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["message"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A unique throwaway directory for a mod tree.
pub fn temp_root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cme-lsp-lifecycle-{}-{}-{}",
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

/// Writes `body` to `root/relative`, creating directories.
pub fn write(root: &std::path::Path, relative: &str, body: &str) -> std::path::PathBuf {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdirs");
    std::fs::write(&path, body).expect("write fixture");
    path
}

pub fn file_uri(path: &std::path::Path) -> String {
    tower_lsp_server::ls_types::Uri::from_file_path(path)
        .expect("file uri")
        .to_string()
}

/// The (line, character) of a byte offset — character in UTF-16 code
/// units, as the LSP positions are encoded.
pub fn line_column(text: &str, offset: usize) -> (u32, u32) {
    let mut line = 0u32;
    let mut line_start = 0usize;
    for (index, byte) in text.as_bytes().iter().enumerate() {
        if index >= offset {
            break;
        }
        if *byte == b'\n' {
            line += 1;
            line_start = index + 1;
        }
    }
    let column = text[line_start..offset]
        .chars()
        .take_while(|c| *c != '\n')
        .map(|c| c.len_utf16() as u32)
        .sum();
    (line, column)
}

/// Position params for `uri` aimed at the end of `needle`'s first
/// occurrence (optionally `back` bytes earlier).
pub fn position_of(text: &str, uri: &str, needle: &str, back: usize) -> serde_json::Value {
    let start = text.find(needle).unwrap_or_else(|| {
        panic!("fixture must contain {needle:?}:\n{text}");
    });
    let offset = start + needle.len() - back;
    let (line, character) = line_column(text, offset);
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character }
    })
}
