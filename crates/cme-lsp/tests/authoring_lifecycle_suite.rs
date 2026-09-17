//! Authoring lifecycle suite — pipeline layer 3: the server as a human
//! actually experiences it while WRITING code. Each test drives a real
//! editor session over the wire (initialize → didOpen → a sequence of
//! full-text didChange edits that mimic keystrokes and completions) and
//! pins what the author sees at every step:
//!
//! - every intermediate, half-typed buffer must publish diagnostics and
//!   still answer feature requests (no panic, no hang, no lost publish);
//! - the diagnostic set must CONVERGE: it is empty exactly when the
//!   program under construction becomes valid;
//! - hover/completion must degrade gracefully on partial words and
//!   half-written declarations instead of firing.
//!
//! The companion layers live in `project_lifecycle_suite` (layer 4, a
//! real mod tree on disk) and `robustness_suite` (layer 5, hostile
//! input); the shared harness is `common::wire`.

mod common;

use common::wire::{self, Harness};
use serde_json::json;

const URI: &str = "file:///authoring/main.cm";

/// Types `steps` as successive full-text buffer states: each becomes one
/// didChange (versioned), and every publish is drained and returned.
async fn type_steps(harness: &mut Harness, steps: &[&str]) -> Vec<Vec<String>> {
    let mut seen = Vec::new();
    for (index, text) in steps.iter().enumerate() {
        let publish = wire::change_and_drain(harness, URI, (index + 2) as i32, text).await;
        seen.push(wire::messages(&publish));
    }
    seen
}

fn assert_any(messages: &[String], contains: &str) {
    assert!(
        messages.iter().any(|m| m.contains(contains)),
        "no diagnostic mentions {contains:?}; got {messages:?}"
    );
}

fn assert_clean(messages: &[String], step: usize) {
    assert!(
        messages.is_empty(),
        "step {step} must be clean; got {messages:?}"
    );
}

// ---------------------------------------------------------------------------
// Authoring a function from an empty buffer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn typing_a_function_from_scratch_converges_to_clean() {
    let mut harness = wire::setup().await;
    let steps = [
        "i",
        "in",
        "int",
        "int main",
        "int main(",
        "int main()",
        "int main() {",
        "int main() {\nret",
        "int main() {\nreturn 0",
        "int main() {\nreturn 0\n}",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    // Mid-word states report the unfinished statement, never a crash.
    assert!(
        !seen[1].is_empty(),
        "the half-typed `in` reports: {:?}",
        seen[1]
    );
    // The moment the declaration is complete the file is clean.
    assert_clean(&seen[9], 9);
}

#[tokio::test]
async fn a_half_typed_buffer_still_answers_hover_and_completion() {
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, URI, "int main() {\nret\n}\n").await;

    // Completion at the end of the partial word answers with a payload.
    let completion = wire::request(
        &mut harness,
        "textDocument/completion",
        wire::position_of("int main() {\nret\n}\n", URI, "ret", 0),
    )
    .await;
    assert!(
        completion.is_array() || completion.is_object(),
        "completion answers mid-typing: {completion}"
    );

    // Hover over the partial word answers null (nothing resolved yet).
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        wire::position_of("int main() {\nret\n}\n", URI, "ret", 1),
    )
    .await;
    assert_eq!(hover, serde_json::Value::Null, "nothing resolves mid-word");
}

#[tokio::test]
async fn diagnostics_flip_while_the_author_breaks_and_fixes_a_type() {
    let mut harness = wire::setup().await;
    let steps = [
        // A clean start…
        "int main() {\nint hp = 100\nreturn hp\n}",
        // …the author mistypes the initializer type…
        "int main() {\nint hp = \"full\"\nreturn hp\n}",
        // …and fixes it again.
        "int main() {\nint hp = 100\nreturn hp\n}",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    assert_clean(&seen[0], 0);
    assert_any(&seen[1], "mismatch");
    assert_clean(&seen[2], 2);
}

#[tokio::test]
async fn an_unfinished_declaration_reports_and_recovers() {
    let mut harness = wire::setup().await;
    let steps = [
        "int main() {\nint hp\nreturn hp\n}",
        "int main() {\nint hp = 1\nreturn hp\n}",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    assert!(
        !seen[0].is_empty(),
        "missing initializer reports: {:?}",
        seen[0]
    );
    assert_clean(&seen[1], 1);
}

// ---------------------------------------------------------------------------
// Hover materializes as the author types declarations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hover_shows_the_crystallized_type_once_the_declaration_completes() {
    let mut harness = wire::setup().await;
    let text = "int main() {\ninfer flags = 0xFF\nreturn flags\n}\n";
    wire::open_and_drain(&mut harness, URI, text).await;

    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        wire::position_of(text, URI, "return flags", 4),
    )
    .await;
    assert!(
        hover.to_string().contains("int"),
        "hex literal crystallizes to int on hover: {hover}"
    );
}

#[tokio::test]
async fn hover_keeps_working_while_the_tail_of_the_file_is_still_being_typed() {
    let mut harness = wire::setup().await;
    let steps = [
        "struct vec2 {\nfloat x\n}",
        "struct vec2 {\nfloat x\nfloat y\n}",
        "struct vec2 {\nfloat x\nfloat y\n}\n\nint main() {\ninfer v = vec2(x: 1.0, y: 2.0)\nreturn v.x",
    ];
    type_steps(&mut harness, &steps).await;
    let text = steps[2];
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        wire::position_of(text, URI, "v.x", 2),
    )
    .await;
    assert!(
        hover.to_string().contains("vec2"),
        "hover works with the file still missing its tail: {hover}"
    );
}

// ---------------------------------------------------------------------------
// Typing numbers: every half-written literal shape stays friendly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn typing_a_hex_literal_one_keystroke_at_a_time_stays_clean() {
    let mut harness = wire::setup().await;
    // `0x` is a (digit-led) identifier while half-written, so the
    // declaration is an unknown name; the moment the last digit lands the
    // file is clean and the value is the hex one.
    let steps = [
        "int main() {\nint hp = \nreturn hp\n}\n",
        "int main() {\nint hp = 0\nreturn hp\n}\n",
        "int main() {\nint hp = 0x\nreturn hp\n}\n",
        "int main() {\nint hp = 0x1\nreturn hp\n}\n",
        "int main() {\nint hp = 0x1F\nreturn hp\n}\n",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    assert_any(&seen[2], "0x");
    assert_clean(&seen[4], 4);
}

#[tokio::test]
async fn typing_separated_and_exponent_literals_lands_clean() {
    let mut harness = wire::setup().await;
    let steps = [
        "int main() {\nint grouped = 1_000\nfloat grow = 1e3\nreturn grouped\n}\n",
        // A trailing separator turns the literal into an identifier: the
        // author sees an unknown name mid-edit, not a lexer crash.
        "int main() {\nint grouped = 1_000_\nfloat grow = 1e3\nreturn grouped\n}\n",
        // Completing the digit repairs it.
        "int main() {\nint grouped = 1_000_0\nfloat grow = 1e3\nreturn grouped\n}\n",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    assert_clean(&seen[0], 0);
    assert_any(&seen[1], "1_000_");
    assert_clean(&seen[2], 2);
}

#[tokio::test]
async fn a_byte_declaration_converges_through_an_out_of_range_literal() {
    let mut harness = wire::setup().await;
    let steps = [
        "int main() {\nbyte mask = 0x1FF\nreturn 0\n}",
        "int main() {\nbyte mask = 0xFF\nreturn 0\n}",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    assert_any(&seen[0], "byte literal out of range");
    assert_clean(&seen[1], 1);
}

#[tokio::test]
async fn member_access_on_a_literal_stays_parseable_while_typing_the_field() {
    let mut harness = wire::setup().await;
    let steps = [
        "int main() {\nint n = 5\nreturn n\n}",
        // The author explores `.length` on a literal: the dot-splitting
        // keeps the tokens sane, the checker reports the field.
        "int main() {\nint n = 5\nreturn 5.length\n}",
        // …and the author backs out to a real array.
        "int main() {\nint[] ns = [5]\nreturn ns.length\n}",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    assert_any(&seen[1], "length");
    assert_clean(&seen[2], 2);
}

// ---------------------------------------------------------------------------
// Structure-driven authoring: structs, calls, match, imports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn completing_a_struct_value_mid_constructor_call() {
    let mut harness = wire::setup().await;
    let text = "struct vec2 {\nfloat x\nfloat y\n}\n\nint main() {\ninfer v = vec2(x: 1.0, y: 2.0)\nreturn v.\n}\n";
    wire::open_and_drain(&mut harness, URI, text).await;
    let completion = wire::request(
        &mut harness,
        "textDocument/completion",
        wire::position_of(text, URI, "v.", 0),
    )
    .await;
    assert!(
        completion.to_string().contains("\"x\""),
        "field completion fires right after the author types the dot: {completion}"
    );
}

#[tokio::test]
async fn building_a_match_statement_arm_by_arm() {
    let mut harness = wire::setup().await;
    let steps = [
        "enum state {\nIdle()\nRunning()\n}\n",
        "enum state {\nIdle()\nRunning()\n}\n\nint main() {\ninfer s = state.Idle()\n",
        "enum state {\nIdle()\nRunning()\n}\n\nint main() {\ninfer s = state.Idle()\nmatch (s) {\nIdle() => { }\n",
        "enum state {\nIdle()\nRunning()\n}\n\nint main() {\ninfer s = state.Idle()\nmatch (s) {\nIdle() => { }\nRunning() => { }\n}\nreturn 0\n}\n",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    assert_clean(&seen[3], 3);
}

#[tokio::test]
async fn an_import_statement_completes_and_validates_as_it_grows() {
    let mut harness = wire::setup().await;
    let steps = [
        "import engine.window\n\nint main() {\nreturn 0\n}",
        "import engine.window.missing\n\nint main() {\nreturn 0\n}",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    // A loose file has no schema: host-rooted imports are accepted
    // everywhere (§10), so both buffers stay clean — the pin is that the
    // growing import never wedges the pipeline.
    assert_clean(&seen[0], 0);
    assert_clean(&seen[1], 1);
}

#[tokio::test]
async fn renaming_a_variable_across_uses_updates_references() {
    let mut harness = wire::setup().await;
    let start = "int main() {\nint hp = 10\nreturn hp + hp\n}";
    wire::open_and_drain(&mut harness, URI, start).await;
    let renamed = "int main() {\nint health = 10\nreturn health + health\n}";
    let publish = wire::change_and_drain(&mut harness, URI, 2, renamed).await;
    assert_clean(&wire::messages(&publish), 1);

    let references = wire::request(
        &mut harness,
        "textDocument/references",
        json!({
            "textDocument": { "uri": URI },
            "position": wire::position_of(renamed, URI, "health = 10", 11)["position"],
            "context": { "includeDeclaration": true }
        }),
    )
    .await;
    let count = references.as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(count, 3, "declaration plus two uses: {references}");
}

// ---------------------------------------------------------------------------
// Whole-buffer gestures: paste, undo, multi-cursor-ish edits, versions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pasting_a_whole_function_then_undoing_it() {
    let mut harness = wire::setup().await;
    let base = "int main() {\nreturn 0\n}";
    wire::open_and_drain(&mut harness, URI, base).await;

    let pasted = "struct item {\nint id\nstr name\n}\n\nint main() {\ninfer it = item(id: 1, name: \"sword\")\nreturn it.id\n}";
    let publish = wire::change_and_drain(&mut harness, URI, 2, pasted).await;
    assert_clean(&wire::messages(&publish), 1);

    let publish = wire::change_and_drain(&mut harness, URI, 3, base).await;
    assert_clean(&wire::messages(&publish), 2);
}

#[tokio::test]
async fn version_numbers_may_jump_the_way_real_editors_jump() {
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, URI, "int main() {\nreturn 0\n}").await;
    let publish = wire::change_and_drain(&mut harness, URI, 7, "int main() {\nreturn 1\n}").await;
    assert_clean(&wire::messages(&publish), 0);
    let publish = wire::change_and_drain(&mut harness, URI, 42, "int main() {\nreturn 2\n}").await;
    assert_clean(&wire::messages(&publish), 1);
}

#[tokio::test]
async fn rapid_open_edit_close_reopen_cycles_recover() {
    let mut harness = wire::setup().await;
    for round in 0..3 {
        let broken = format!("int main() {{\nint hp = round{round}\nreturn hp\n}}");
        let publish = wire::open_and_drain(&mut harness, URI, &broken).await;
        assert_any(&wire::messages(&publish), "unknown name");
        wire::close(&mut harness, URI).await;
        let _ = wire::next_publish(&mut harness).await; // the close clears the list

        let fixed = format!("int main() {{\nint hp = {round}\nreturn hp\n}}");
        let publish = wire::open_and_drain(&mut harness, URI, &fixed).await;
        assert_clean(&wire::messages(&publish), round);
    }
}

#[tokio::test]
async fn two_buffers_of_the_same_language_do_not_leak_into_each_other() {
    let mut harness = wire::setup().await;
    let other = "file:///authoring/other.cm";
    wire::open_and_drain(&mut harness, URI, "int main() {\nint hp = 1\nreturn hp\n}").await;
    wire::open_and_drain(
        &mut harness,
        other,
        "int main() {\nint hp = tr\nreturn hp\n}",
    )
    .await;

    // Fixing one leaves the other's damage intact.
    let publish = wire::change_and_drain(
        &mut harness,
        URI,
        2,
        "int main() {\nint hp = 2\nreturn hp\n}",
    )
    .await;
    assert_clean(&wire::messages(&publish), 0);
    let hover = wire::request(
        &mut harness,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": other },
            "position": { "line": 1, "character": 11 }
        }),
    )
    .await;
    assert_eq!(hover, serde_json::Value::Null, "`tr` never resolves");
}

#[tokio::test]
async fn a_file_without_a_trailing_newline_publishes_cleanly() {
    let mut harness = wire::setup().await;
    let publish = wire::open_and_drain(&mut harness, URI, "int main() {\nreturn 0\n}").await;
    assert_clean(&wire::messages(&publish), 0);
    let symbols = wire::request(
        &mut harness,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": URI } }),
    )
    .await;
    assert!(
        symbols.to_string().contains("main"),
        "outline works without the trailing newline: {symbols}"
    );
}

#[tokio::test]
async fn typing_an_expression_across_lines_keeps_the_continuation_clean() {
    let mut harness = wire::setup().await;
    let steps = [
        // Trailing operator continues the expression (§A.8)…
        "int main() {\nint total = 1 +\n2\nreturn total\n}",
        // …and wrapping inside parens is free.
        "int main() {\nint total = (1 +\n2)\nreturn total\n}",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    assert_clean(&seen[0], 0);
    assert_clean(&seen[1], 1);
}

#[tokio::test]
async fn the_underscore_road_to_a_grouped_number_never_wedges_the_server() {
    let mut harness = wire::setup().await;
    let steps = [
        "int main() {\nint m = 1_0\nreturn m\n}",
        "int main() {\nint m = 1_0_\nreturn m\n}",
        "int main() {\nint m = 1_0_0\nreturn m\n}",
        "int main() {\nint m = 1_0_0_0\nreturn m\n}",
    ];
    let seen = type_steps(&mut harness, &steps).await;
    assert_clean(&seen[0], 0);
    assert_any(&seen[1], "1_0_");
    assert_clean(&seen[3], 3);
}

#[tokio::test]
async fn completions_listed_mid_authoring_are_stable_after_the_fix() {
    let mut harness = wire::setup().await;
    let text = "int main() {\nint hp = 0xFF\n}\n";
    wire::open_and_drain(&mut harness, URI, text).await;
    let before = wire::request(
        &mut harness,
        "textDocument/completion",
        wire::position_of(text, URI, "int hp", 4),
    )
    .await;
    // Break the file, then fix it: completion still answers the same way.
    let _ = wire::change_and_drain(&mut harness, URI, 2, "int main() {\nint hp = \"\"\n}\n").await;
    let _ = wire::change_and_drain(&mut harness, URI, 3, text).await;
    let after = wire::request(
        &mut harness,
        "textDocument/completion",
        wire::position_of(text, URI, "int hp", 4),
    )
    .await;
    assert_eq!(
        before.to_string(),
        after.to_string(),
        "completion survives a break/fix cycle unchanged"
    );
}

#[tokio::test]
async fn semantic_tokens_refresh_after_every_edit_of_the_session() {
    let mut harness = wire::setup().await;
    wire::open_and_drain(&mut harness, URI, "int main() {\nint hp = 1\nreturn hp\n}").await;
    let tokens = wire::request(
        &mut harness,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": URI } }),
    )
    .await;
    let count = tokens["data"].as_array().map(Vec::len).unwrap_or_default();
    assert!(count > 0, "tokens exist: {tokens}");

    let publish = wire::change_and_drain(
        &mut harness,
        URI,
        2,
        "int main() {\nint armor = 2\nreturn armor\n}",
    )
    .await;
    assert_clean(&wire::messages(&publish), 1);
    let tokens = wire::request(
        &mut harness,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": URI } }),
    )
    .await;
    let count = tokens["data"].as_array().map(Vec::len).unwrap_or_default();
    assert!(count > 0, "tokens still exist after the edit: {tokens}");
}
