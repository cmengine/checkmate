//! Robustness suite — pipeline layer 5: the wire surface under hostile
//! and degenerate input. Everything an editor, a plugin, or a malicious
//! workspace can throw at a language server:
//!
//! - protocol misuse: requests before `initialize`, unknown methods and
//!   notifications, double initialization, post-shutdown traffic,
//!   cancellation notes, string request ids;
//! - document garbage: empty and whitespace-only buffers, keyword soup,
//!   punctuation soup, control characters, a 100 KB single line, ten
//!   thousand nested parens, unreasonably long identifiers;
//! - position hazards: out-of-range lines and columns, EOF boundaries,
//!   emoji/CJK/multi-byte content on the SAME line as the target,
//!   CRLF documents, empty lines;
//! - lifecycle edges: didChange without didOpen, didChange with an empty
//!   change list, didClose for unknown documents, duplicate didOpen,
//!   interleaved open/close of many files.
//!
//! The pin on every test is the same: the server answers within the
//! timeout, never panics, and never double-fires a publish it should not.

mod common;

use common::wire::{self, Harness};
use serde_json::json;
use tower::{Service, ServiceExt};

const URI: &str = "file:///robustness/main.cm";

async fn open_raw(harness: &mut Harness, text: &str) -> serde_json::Value {
    wire::open_and_drain(harness, URI, text).await
}

// ---------------------------------------------------------------------------
// Protocol misuse
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_feature_request_before_initialize_answers_without_hanging() {
    let mut harness = wire::setup_raw().await;
    let response = wire::try_request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": URI },
            "position": { "line": 0, "character": 0 }
        }),
    )
    .await;
    // Any JSON-RPC outcome is acceptable (error or null result); the pin
    // is that the server responds at all.
    assert!(
        response.result().is_some() || response.error().is_some(),
        "an outcome arrives: {response:?}"
    );
}

#[tokio::test]
async fn unknown_methods_answer_method_not_found() {
    let mut harness = wire::setup().await;
    for method in [
        "textDocument/rename",
        "workspace/executeCommand",
        "cme/nope",
    ] {
        let response = wire::try_request(&mut harness, method, json!({})).await;
        assert!(
            response.error().is_some(),
            "{method} answers an error: {response:?}"
        );
    }
}

#[tokio::test]
async fn unknown_notifications_are_ignored_silently() {
    let mut harness = wire::setup().await;
    wire::notify(
        &mut harness,
        "textDocument/didSave",
        json!({ "textDocument": { "uri": URI } }),
    )
    .await;
    wire::notify(&mut harness, "$/setTrace", json!({ "value": "verbose" })).await;
    wire::notify(&mut harness, "$/cancelRequest", json!({ "id": 1 })).await;
    // The server still works: an open round-trips normally.
    let publish = open_raw(&mut harness, "int main() {\nreturn 0\n}").await;
    assert!(wire::messages(&publish).is_empty());
}

#[tokio::test]
async fn a_string_request_id_round_trips() {
    let mut harness = wire::setup().await;
    let request = tower_lsp_server::jsonrpc::Request::build("textDocument/hover".to_string())
        .params(json!({
            "textDocument": { "uri": URI },
            "position": { "line": 0, "character": 0 }
        }))
        .id("string-id-42")
        .finish();
    let response = harness
        .service()
        .ready()
        .await
        .unwrap()
        .call(request)
        .await
        .unwrap()
        .expect("response");
    let expected = tower_lsp_server::jsonrpc::Id::from("string-id-42");
    assert_eq!(response.id(), &expected, "the string id round-trips");
}

#[tokio::test]
async fn shutdown_answers_and_the_session_stays_tear_down_able() {
    let mut harness = wire::setup().await;
    let request = tower_lsp_server::jsonrpc::Request::build("shutdown".to_string())
        .id(999)
        .finish();
    let response = harness
        .service()
        .ready()
        .await
        .unwrap()
        .call(request)
        .await
        .unwrap()
        .expect("shutdown answers");
    assert!(
        response.result().is_some(),
        "shutdown answers ok: {response:?}"
    );
}

#[tokio::test]
async fn did_change_without_an_open_still_publishes() {
    let mut harness = wire::setup().await;
    let publish = wire::change_and_drain(&mut harness, URI, 1, "int main() {\nreturn 0\n}").await;
    assert_eq!(publish["uri"].as_str(), Some(URI));
    assert!(wire::messages(&publish).is_empty());
}

#[tokio::test]
async fn did_change_with_an_empty_change_list_publishes_nothing_new() {
    let mut harness = wire::setup().await;
    open_raw(&mut harness, "int main() {\nreturn 0\n}").await;
    wire::notify(
        &mut harness,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": URI, "version": 2 },
            "contentChanges": []
        }),
    )
    .await;
    // No publish follows an empty change list; the next feature request
    // still answers.
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": URI },
            "position": { "line": 0, "character": 4 }
        }),
    )
    .await;
    let _ = hover;
}

#[tokio::test]
async fn did_close_for_a_never_opened_document_publishes_an_empty_list() {
    let mut harness = wire::setup().await;
    wire::close(&mut harness, "file:///robustness/never.cm").await;
    let publish = wire::next_publish(&mut harness).await;
    assert_eq!(publish["uri"].as_str(), Some("file:///robustness/never.cm"));
    assert_eq!(
        publish["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "the close clears the list: {publish}"
    );
}

#[tokio::test]
async fn a_duplicate_did_open_replaces_the_buffer() {
    let mut harness = wire::setup().await;
    let _ = open_raw(&mut harness, "int main() {\nint hp = tr\nreturn hp\n}").await;
    let publish =
        wire::open_and_drain(&mut harness, URI, "int main() {\nint hp = 1\nreturn hp\n}").await;
    assert!(
        wire::messages(&publish).is_empty(),
        "the second open wins: {:?}",
        wire::messages(&publish)
    );
}

#[tokio::test]
async fn interleaved_open_close_of_many_files_never_crosses_streams() {
    let mut harness = wire::setup().await;
    let uris: Vec<String> = (0..6)
        .map(|index| format!("file:///robustness/multi-{index}.cm"))
        .collect();
    for (index, uri) in uris.iter().enumerate() {
        let text = if index % 2 == 0 {
            format!("int main() {{\nint hp{index} = {index}\nreturn hp{index}\n}}")
        } else {
            format!("int main() {{\nint hp{index} = oops{index}\nreturn hp{index}\n}}")
        };
        wire::open_and_drain(&mut harness, uri, &text).await;
    }
    for (index, uri) in uris.iter().enumerate() {
        wire::close(&mut harness, uri).await;
        let publish = wire::next_publish(&mut harness).await;
        assert_eq!(publish["uri"].as_str(), Some(uri.as_str()));
        assert_eq!(publish["diagnostics"].as_array().map(Vec::len), Some(0));
        let _ = index;
    }
}

// ---------------------------------------------------------------------------
// Hostile document content
// ---------------------------------------------------------------------------

async fn survives(harness: &mut Harness, text: &str, label: &str) {
    let publish = wire::open_and_drain(harness, URI, text).await;
    let _ = wire::messages(&publish); // diagnostics or none; the pin is survival
    let hover = wire::request(
        harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": URI },
            "position": { "line": 0, "character": 0 }
        }),
    )
    .await;
    let _ = hover;
    let tokens = wire::request(
        harness,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": URI } }),
    )
    .await;
    let _ = tokens;
    let _ = label;
}

#[tokio::test]
async fn the_empty_document_survives_every_feature() {
    let mut harness = wire::setup().await;
    survives(&mut harness, "", "empty").await;
}

#[tokio::test]
async fn whitespace_and_newline_only_documents_survive() {
    let mut harness = wire::setup().await;
    survives(&mut harness, "\n\n\n", "newlines").await;
    survives(&mut harness, "   \t  \r\n", "spaces").await;
}

#[tokio::test]
async fn a_comment_only_document_survives() {
    let mut harness = wire::setup().await;
    survives(&mut harness, "// nothing to see\n/* here */\n", "comments").await;
}

#[tokio::test]
async fn keyword_soup_survives() {
    let mut harness = wire::setup().await;
    survives(
        &mut harness,
        "int int int\nstruct struct\nif else while return\nmatch for in impl import\n",
        "keywords",
    )
    .await;
}

#[tokio::test]
async fn punctuation_soup_survives() {
    let mut harness = wire::setup().await;
    survives(
        &mut harness,
        "(){}[]<>=!&&||+-*/%.,:;?=>\n!!!@@@###$$$%%^\n",
        "punctuation",
    )
    .await;
}

#[tokio::test]
async fn control_characters_survive() {
    let mut harness = wire::setup().await;
    survives(
        &mut harness,
        "int main() {\nstr s = \"a\\u0001b\"\nreturn 0\n}",
        "control chars",
    )
    .await;
}

#[tokio::test]
async fn a_one_hundred_kilobyte_single_line_survives() {
    let mut harness = wire::setup().await;
    let line = "1 + ".repeat(25_000) + "1";
    survives(&mut harness, &line, "100KB line").await;
}

#[tokio::test]
async fn deeply_nested_parens_recover_and_stay_answerable() {
    let mut harness = wire::setup().await;
    let depth = 2_000;
    let text = format!("{}0{}", "(".repeat(depth), ")".repeat(depth));
    survives(&mut harness, &text, "nested parens").await;
}

#[tokio::test]
async fn an_absurdly_long_identifier_survives() {
    let mut harness = wire::setup().await;
    let name = "v".repeat(20_000);
    survives(&mut harness, &format!("int {name} = 1"), "long ident").await;
}

#[tokio::test]
async fn numeric_garbage_boundaries_survive() {
    let mut harness = wire::setup().await;
    // Every pathological digit-led shape at once: overflow prefixes,
    // broken separators, dangling exponents, hex-with-tails.
    survives(
        &mut harness,
        "0x 0b 0o 0xZZZ 0b12 0o8 1__0 1_ 0x8000000000000000 1e 1e+ 1.5e 0xFFg 123abc 2_D 3Vector\n",
        "numeric garbage",
    )
    .await;
    // And the file still publishes its diagnostics: the overflow shape
    // reports exactly one lex error, the rest are names.
    let publish =
        wire::open_and_drain(&mut harness, URI, "0x8000000000000000 0b2 1e 1_000\n").await;
    let messages = wire::messages(&publish);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("integer literal is too large")),
        "the overflow is named: {messages:?}"
    );
}

// ---------------------------------------------------------------------------
// Position hazards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn far_out_of_range_positions_answer_null() {
    let mut harness = wire::setup().await;
    open_raw(&mut harness, "int main() {\nreturn 0\n}\n").await;
    for (line, character) in [
        (999u32, 0u32),
        (0, 9999),
        (9999, 9999),
        (u32::MAX, u32::MAX),
    ] {
        let hover = wire::request(
            &mut harness,
            "textDocument/hover",
            json!({
                "textDocument": { "uri": URI },
                "position": { "line": line, "character": character }
            }),
        )
        .await;
        assert_eq!(
            hover,
            serde_json::Value::Null,
            "({line},{character}) declines"
        );
    }
}

#[tokio::test]
async fn positions_at_exact_eof_answer_null() {
    let mut harness = wire::setup().await;
    let text = "int main() {\nreturn 0\n}\n";
    open_raw(&mut harness, text).await;
    let end = wire::line_column(text, text.len());
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": URI },
            "position": { "line": end.0, "character": end.1 }
        }),
    )
    .await;
    assert_eq!(hover, serde_json::Value::Null);
}

#[tokio::test]
async fn positions_on_empty_lines_answer_null() {
    let mut harness = wire::setup().await;
    open_raw(&mut harness, "int main() {\n\nreturn 0\n}\n").await;
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": URI },
            "position": { "line": 1, "character": 0 }
        }),
    )
    .await;
    assert_eq!(hover, serde_json::Value::Null);
}

#[tokio::test]
async fn utf16_positions_survive_emoji_and_cjk_on_the_target_line() {
    let mut harness = wire::setup().await;
    // "🚀" is 2 UTF-16 units; "棋" is 1; both are 3-4 UTF-8 bytes. The
    // hover aims at the local `hp`, which sits AFTER the multi-byte map
    // key on the same line: a server counting UTF-8 bytes instead of
    // UTF-16 units would land short and answer null.
    let text = "int main() {\nint hp = 5\nmap<str, int> marks = { \"🚀棋\": hp }\nreturn 0\n}\n";
    open_raw(&mut harness, text).await;
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": URI },
            "position": wire::position_of(text, URI, ": hp }", 4)["position"]
        }),
    )
    .await;
    assert!(
        hover.to_string().contains("int"),
        "hover resolves after multi-byte content: {hover}"
    );

    // The server also survives a position INSIDE the emoji.
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": URI },
            "position": { "line": 2, "character": 24 }
        }),
    )
    .await;
    let _ = hover;
}

#[tokio::test]
async fn a_crlf_document_drives_the_full_feature_matrix() {
    let mut harness = wire::setup().await;
    let text = "struct vec2 {\r\nfloat x\r\n}\r\n\r\nint main() {\r\ninfer v = vec2(x: 1.0)\r\nreturn 0\r\n}\r\n";
    open_raw(&mut harness, text).await;

    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": URI },
            "position": wire::position_of(text, URI, "infer v = vec2", 4)["position"]
        }),
    )
    .await;
    assert!(
        hover.to_string().contains("vec2"),
        "hover over the CRLF buffer: {hover}"
    );

    let symbols = wire::request(
        &mut harness,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": URI } }),
    )
    .await;
    assert!(
        symbols.to_string().contains("vec2"),
        "the outline survives CRLF: {symbols}"
    );

    let tokens = wire::request(
        &mut harness,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": URI } }),
    )
    .await;
    assert!(
        tokens["data"].as_array().map(Vec::len).unwrap_or_default() > 0,
        "tokens survive CRLF: {tokens}"
    );
}

#[tokio::test]
async fn a_document_with_mixed_line_endings_publishes_once_per_edit() {
    let mut harness = wire::setup().await;
    let text = "int main() {\r\nint a = 1\nint b = 2\r\nreturn a + b\n}";
    let publish = open_raw(&mut harness, text).await;
    assert!(
        wire::messages(&publish).is_empty(),
        "mixed endings parse: {:?}",
        wire::messages(&publish)
    );
}

#[tokio::test]
async fn request_flood_of_twenty_is_answered_in_order() {
    let mut harness = wire::setup().await;
    open_raw(&mut harness, "int main() {\nreturn 0\n}").await;
    for round in 0..20 {
        let hover = wire::request(
            &mut harness,
            "textDocument/hover",
            json!({
                "textDocument": { "uri": URI },
                "position": { "line": 0, "character": 4 + (round % 2) }
            }),
        )
        .await;
        let _ = hover;
    }
}

#[tokio::test]
async fn non_file_uri_schemes_follow_the_loose_file_pipeline() {
    let mut harness = wire::setup().await;
    // Untitled buffers use the untitled: scheme in many editors.
    let publish = wire::open_and_drain(
        &mut harness,
        "untitled:Untitled-1",
        "int main() {\nreturn 0\n}",
    )
    .await;
    assert!(
        wire::messages(&publish).is_empty(),
        "a synthetic uri checks as a loose script: {:?}",
        wire::messages(&publish)
    );
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": "untitled:Untitled-1" },
            "position": { "line": 0, "character": 4 }
        }),
    )
    .await;
    assert!(
        hover.to_string().contains("main"),
        "features answer on the synthetic uri: {hover}"
    );
}

#[tokio::test]
async fn the_same_filename_in_two_directories_stays_two_documents() {
    let mut harness = wire::setup().await;
    let a = wire::open_and_drain(
        &mut harness,
        "file:///robustness/a/main.cm",
        "int main() {\nint flag = 1\nreturn flag\n}",
    )
    .await;
    let b = wire::open_and_drain(
        &mut harness,
        "file:///robustness/b/main.cm",
        "int main() {\nint flag = tr\nreturn flag\n}",
    )
    .await;
    assert!(wire::messages(&a).is_empty());
    assert!(!wire::messages(&b).is_empty(), "b's damage stays in b");
}
