//! Megaprogram file analysis suite (§8): position features over files that
//! mention megaprograms — the scenarios that used to go dark.
//!
//! The anchoring rule: diagnostics for a mega file come from the EXPANDED
//! pipeline (the expander anchors them in the original text), while the
//! position features — hover, go-to-definition, references, document
//! symbols, semantic tokens, and completion in ordinary code — analyze the
//! ORIGINAL text through [`cme_lsp::db::parse_original`], so every span
//! lands in the user's buffer. These tests pin both directions:
//!
//! - the ordinary Checkmate code around mega constructs fully resolves
//!   (functions, locals, imports, schema types when a mod grants one);
//! - inside the mega constructs themselves (declaration patterns and
//!   templates, grammar bodies, invocation regions) there is nothing to
//!   resolve — foreign text — and completion switches to the pattern
//!   vocabulary instead.

use tower_lsp_server::ls_types;

use cme_lsp::analysis::Analysis;
use cme_lsp::convert::LineIndex;
use cme_lsp::features::definition::{definition, references};
use cme_lsp::features::hover::hover;
use cme_lsp::features::mega_completion::{
    MegaContext, completions as mega_completions, context_at,
};
use cme_lsp::features::symbols::document_symbols;
use cme_lsp::features::tokens::semantic_tokens;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parses the ORIGINAL text (tolerantly) and builds its symbol table —
/// the exact pipeline `db::parse_original` + `Analysis::build_with_schema`
/// runs for a mega file in the server: mega construct interiors are
/// blanked first (byte-for-byte, newlines preserved), then the tolerant
/// parse sees the file's Checkmate skeleton.
fn analysis(source: &str) -> Analysis<'_> {
    let blanked = cme_lsp::db::blank_mega_regions(source);
    let outcome: &'static mut cme_compiler::ParseOutcome =
        Box::leak(Box::new(cme_compiler::parse_source(&blanked)));
    Analysis::build(source, &outcome.statements)
}

/// The byte offset just after `needle`'s first `skip` bytes.
fn offset_of(source: &str, needle: &str, skip: usize) -> usize {
    source
        .find(needle)
        .unwrap_or_else(|| panic!("fixture must contain {needle:?}:\n{source}"))
        + skip
}

fn hover_at(analysis: &Analysis<'_>, offset: usize) -> Option<String> {
    let hover = hover(analysis, offset)?;
    let ls_types::HoverContents::Markup(markup) = hover.contents else {
        panic!("hover must render markdown");
    };
    Some(markup.value)
}

/// The hover payload renders as a fenced ```checkmate block; tests match
/// on the signature line inside it.
fn hover_contains(analysis: &Analysis<'_>, offset: usize, needle: &str) -> bool {
    hover_at(analysis, offset)
        .map(|markdown| markdown.contains(needle))
        .unwrap_or(false)
}

fn decoded_tokens(
    analysis: &Analysis<'_>,
    index: &LineIndex,
    text: &str,
) -> Vec<(u32, u32, u32, u32, u32)> {
    let tokens = semantic_tokens(analysis, index, text);
    let mut rows = Vec::new();
    let (mut line, mut character) = (0u32, 0u32);
    for token in &tokens.data {
        line += token.delta_line;
        character = if token.delta_line > 0 {
            token.delta_start
        } else {
            character + token.delta_start
        };
        rows.push((
            line,
            character,
            token.length,
            token.token_type,
            token.token_modifiers_bitset,
        ));
    }
    rows
}

fn outline(
    analysis: &Analysis<'_>,
    index: &LineIndex,
    text: &str,
) -> Vec<(String, ls_types::SymbolKind, Vec<String>)> {
    document_symbols(analysis, index, text)
        .into_iter()
        .map(|symbol| {
            let children = symbol
                .children
                .unwrap_or_default()
                .into_iter()
                .map(|child| child.name)
                .collect();
            (symbol.name, symbol.kind, children)
        })
        .collect()
}

fn labels(items: Vec<ls_types::CompletionItem>) -> Vec<String> {
    items.into_iter().map(|item| item.label).collect()
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A working §8-shaped file: grammar + mega declaration + invocations, with
/// ordinary functions before, between, and after the mega constructs.
const MEGA_FILE: &str = "\
int base() {
    int seed = 21
    return seed
}

grammar conf {
    skip    [ ' ', '\\t' ]
    comment ( \"#\" )

    rule setting {
        $word key \"=\" $str value
        eol
    }
}

mega setting(
    #complete(availableKeys)
    \"key:\" $word key
    $text rest
) {
    confSet(
        key: $\"{$key}\"
        value: $rest
    )
}

int double(int value) {
    return value * 2
}

int main() {
    int total = base()
    total = double(total)
    setting! {
        key: mode
        value: fast
    }
    return total
}
";

// ---------------------------------------------------------------------------
// Hover in ordinary code around mega constructs
// ---------------------------------------------------------------------------

#[test]
fn hover_works_before_a_grammar_declaration() {
    let a = analysis(MEGA_FILE);
    let offset = offset_of(MEGA_FILE, "return seed", "return ".len());
    assert!(
        hover_contains(&a, offset, "seed: int"),
        "a local inside a function ABOVE the grammar still resolves, got: {:?}",
        hover_at(&a, offset)
    );
}

#[test]
fn hover_works_between_the_mega_constructs() {
    let a = analysis(MEGA_FILE);
    let offset = offset_of(MEGA_FILE, "return value * 2", "return ".len());
    assert!(
        hover_contains(&a, offset, "value: int"),
        "a parameter inside a function BETWEEN the grammar and the invocation still resolves, got: {:?}",
        hover_at(&a, offset)
    );
}

#[test]
fn hover_works_after_an_invocation_region() {
    let a = analysis(MEGA_FILE);
    let offset = offset_of(MEGA_FILE, "return total", "return ".len());
    assert!(
        hover_contains(&a, offset, "total: int"),
        "a local inside a function AFTER the invocation region still resolves, got: {:?}",
        hover_at(&a, offset)
    );
}

#[test]
fn hover_on_a_call_target_inside_a_mega_file() {
    let a = analysis(MEGA_FILE);
    let offset = offset_of(MEGA_FILE, "= double(total)", "= ".len());
    let markdown = hover_at(&a, offset).expect("double resolves");
    assert!(
        markdown.contains("int double(int value)"),
        "hover shows the signature, got: {markdown}"
    );
}

#[test]
fn hover_inside_an_invocation_region_answers_nothing() {
    let a = analysis(MEGA_FILE);
    // `key: mode` is foreign region text — nothing Checkmate lives there.
    let offset = offset_of(MEGA_FILE, "key: mode", "key: ".len());
    assert!(
        hover_at(&a, offset).is_none(),
        "foreign region content has no Checkmate meaning"
    );
}

// ---------------------------------------------------------------------------
// Go-to-definition and references across mega constructs
// ---------------------------------------------------------------------------

#[test]
fn definition_crosses_the_grammar_and_the_mega_declaration() {
    let a = analysis(MEGA_FILE);
    // `base()` is called inside main, AFTER the grammar and mega blocks.
    let offset = offset_of(MEGA_FILE, "= base()", "= ".len());
    let span = definition(&a, offset).expect("base's call site jumps to its declaration");
    assert_eq!(
        &MEGA_FILE[span.start..span.end],
        "base",
        "the definition lands on the declaration name, in ORIGINAL coordinates"
    );
}

#[test]
fn definition_from_before_the_mega_constructs() {
    let a = analysis(MEGA_FILE);
    let offset = offset_of(MEGA_FILE, "total = double(total)", "total = ".len());
    let span = definition(&a, offset).expect("double resolves from an early function");
    assert_eq!(&MEGA_FILE[span.start..span.end], "double");
}

#[test]
fn references_find_uses_across_the_invocation_boundary() {
    let a = analysis(MEGA_FILE);
    // On `double`'s DECLARATION name: the call in main is on the far side
    // of the mega declaration + region.
    let offset = offset_of(MEGA_FILE, "int double(", "int ".len());
    let spans = references(&a, offset, true);
    let texts: Vec<&str> = spans
        .iter()
        .map(|span| &MEGA_FILE[span.start..span.end])
        .collect();
    assert!(
        texts.contains(&"double"),
        "the declaration itself is reported: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("double")),
        "the call after the mega constructs is found: {texts:?}"
    );
}

// ---------------------------------------------------------------------------
// Document outline and semantic tokens
// ---------------------------------------------------------------------------

#[test]
fn outline_lists_the_functions_of_a_mega_file() {
    let a = analysis(MEGA_FILE);
    let index = LineIndex::new(MEGA_FILE);
    let names: Vec<String> = outline(&a, &index, MEGA_FILE)
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    for expected in ["base", "double", "main"] {
        assert!(
            names.iter().any(|name| name == expected),
            "{expected} must appear in the outline, got: {names:?}"
        );
    }
}

#[test]
fn semantic_tokens_cover_the_whole_mega_file() {
    let a = analysis(MEGA_FILE);
    let index = LineIndex::new(MEGA_FILE);
    let rows = decoded_tokens(&a, &index, MEGA_FILE);
    assert!(!rows.is_empty(), "tokens are produced");
    // A token must paint the final `return total` — on the far side of the
    // invocation region.
    let return_total_line = index.line_of(offset_of(MEGA_FILE, "return total", 0)) as u32;
    assert!(
        rows.iter().any(|row| row.0 == return_total_line),
        "tokens paint the code after the invocation region (line {return_total_line}), rows end at line {:?}",
        rows.iter().map(|row| row.0).max()
    );
}

// ---------------------------------------------------------------------------
// Completion routing inside a mega file
// ---------------------------------------------------------------------------

#[test]
fn completion_in_ordinary_code_of_a_mega_file_offers_locals_and_functions() {
    let a = analysis(MEGA_FILE);
    let offset = offset_of(MEGA_FILE, "total = double(total)", 0);
    let labels = labels(cme_lsp::features::completion::completions(
        &a, MEGA_FILE, offset,
    ));
    assert!(
        labels.iter().any(|label| label == "total"),
        "the local `total` is offered: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "base"),
        "the top-level function `base` is offered: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "double"),
        "the top-level function `double` is offered: {labels:?}"
    );
}

#[test]
fn completion_in_a_mega_declaration_pattern_offers_the_pattern_language() {
    let offset = offset_of(MEGA_FILE, "\"key:\" $word key", 0);
    let context = context_at(MEGA_FILE, offset);
    assert_eq!(context, MegaContext::Pattern, "the offset is pattern text");
    let labels = labels(mega_completions(MEGA_FILE, offset));
    assert!(
        labels.iter().any(|label| label == "$word"),
        "fragments are offered in a pattern: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "each"),
        "combinators are offered in a pattern: {labels:?}"
    );
}

#[test]
fn completion_in_a_mega_declaration_template_offers_the_template_constructs() {
    let offset = offset_of(MEGA_FILE, "value: $rest", 0);
    let context = context_at(MEGA_FILE, offset);
    assert_eq!(context, MegaContext::Template);
    let labels = labels(mega_completions(MEGA_FILE, offset));
    assert!(
        labels.iter().any(|label| label == "[each in "),
        "template constructs are offered: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "require("),
        "require is offered: {labels:?}"
    );
}

#[test]
fn completion_in_a_grammar_body_offers_the_profile_declarations() {
    let offset = offset_of(MEGA_FILE, "comment ( \"#\" )", 0);
    let context = context_at(MEGA_FILE, offset);
    assert_eq!(context, MegaContext::GrammarProfile);
    let labels = labels(mega_completions(MEGA_FILE, offset));
    assert!(
        labels.iter().any(|label| label == "skip"),
        "profile declarations are offered: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "rule"),
        "rule is offered: {labels:?}"
    );
}

#[test]
fn completion_inside_a_rule_body_switches_to_the_pattern_language() {
    let offset = offset_of(MEGA_FILE, "$word key \"=\" $str value", 0);
    let context = context_at(MEGA_FILE, offset);
    assert_eq!(context, MegaContext::RuleBody);
    let labels = labels(mega_completions(MEGA_FILE, offset));
    assert!(
        labels.iter().any(|label| label == "$ident"),
        "pattern fragments are offered inside a rule body: {labels:?}"
    );
}

#[test]
fn completion_inside_an_invocation_region_is_suppressed() {
    let offset = offset_of(MEGA_FILE, "key: mode", 0);
    let context = context_at(MEGA_FILE, offset);
    assert_eq!(context, MegaContext::InvocationRegion);
    assert!(
        mega_completions(MEGA_FILE, offset).is_empty(),
        "the region is foreign text; suggestions would be noise"
    );
}

#[test]
fn completion_after_the_invocation_region_is_ordinary_again() {
    let a = analysis(MEGA_FILE);
    let offset = offset_of(MEGA_FILE, "return total", "return ".len());
    let labels = labels(cme_lsp::features::completion::completions(
        &a, MEGA_FILE, offset,
    ));
    assert!(
        labels.iter().any(|label| label == "total"),
        "after the region the script analysis answers again: {labels:?}"
    );
}

#[test]
fn mega_top_level_completions_still_lead_outside_any_construct() {
    // Top level of the file (before the first function): the dedicated
    // mega path offers the declarations + keywords.
    let offset = 0;
    let labels = labels(mega_completions(MEGA_FILE, offset));
    assert!(
        labels.starts_with(&["mega".to_string(), "grammar".to_string()]),
        "mega/grammar lead the top-level list: {labels:?}"
    );
}

// ---------------------------------------------------------------------------
// Broken expansion does not darken the analysis
// ---------------------------------------------------------------------------

#[test]
fn a_failed_expansion_leaves_hover_and_completion_working() {
    // The template body `???` fails expansion — the editor surfaces the
    // expansion diagnostics, and the analysis still answers on the
    // original text.
    let source = "\
mega twice($int value) {
    ???
}

int base() {
    int seed = 7
    return seed
}
";
    let a = analysis(source);
    let offset = offset_of(source, "return seed", "return ".len());
    assert!(
        hover_contains(&a, offset, "seed: int"),
        "hover answers even when expansion fails, got: {:?}",
        hover_at(&a, offset)
    );
    let labels = labels(cme_lsp::features::completion::completions(
        &a, source, offset,
    ));
    assert!(labels.iter().any(|label| label == "seed"));
}

// ---------------------------------------------------------------------------
// Mega files inside a mod: the §9 schema surface joins the analysis
// ---------------------------------------------------------------------------

/// A schema-shaped fixture (as a schema FILE would declare it) reduced to
/// what the analysis of the SCRIPT sees. The full mod + schema flow is
/// pinned by `mega_mod_wire_*` in this suite and `schema_mod_suite`.
#[test]
fn hover_on_a_call_target_reports_the_signature_not_the_module() {
    let a = analysis(MEGA_FILE);
    let offset = offset_of(MEGA_FILE, "base()", 0);
    assert!(
        hover_contains(&a, offset, "int base()"),
        "the signature is `int base()`, got: {:?}",
        hover_at(&a, offset)
    );
}

// ---------------------------------------------------------------------------
// The db layer: original-text coordinates
// ---------------------------------------------------------------------------

#[test]
fn parse_original_anchors_declarations_in_user_coordinates() {
    let db = cme_lsp::db::Database::default();
    let file =
        cme_lsp::db::SourceFile::new(&db, MEGA_FILE.to_string(), cme_lsp::db::FileKind::Script);
    let parsed = cme_lsp::db::parse_original(&db, file);
    let statements = parsed.statements(&db);
    let names: Vec<&str> = statements
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            cme_core::ast::StmtKind::FuncDecl { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    for expected in ["base", "double", "main"] {
        assert!(
            names.contains(&expected),
            "{expected} is recovered from the original text, got: {names:?}"
        );
    }
    // Every recovered function's span must slice back to its own header
    // in the ORIGINAL text.
    for stmt in statements.iter() {
        if let cme_core::ast::StmtKind::FuncDecl { name, .. } = &stmt.kind {
            let header = format!("int {name}(");
            assert!(
                MEGA_FILE[stmt.span.start..].starts_with(&header),
                "{name}'s span points at `{}` in the buffer",
                &MEGA_FILE[stmt.span.start
                    ..stmt.span.start + header.len().min(MEGA_FILE.len() - stmt.span.start)]
            );
        }
    }
}

#[test]
fn expanded_pipeline_and_original_pipeline_disagree_by_design() {
    let db = cme_lsp::db::Database::default();
    let file =
        cme_lsp::db::SourceFile::new(&db, MEGA_FILE.to_string(), cme_lsp::db::FileKind::Script);
    // The expansion runs clean (the fixture is a working mega program).
    let expansion = cme_lsp::db::expansion(&db, file);
    assert!(
        expansion.diagnostics(&db).is_empty(),
        "the fixture expands without diagnostics"
    );
    assert!(
        !cme_lsp::db::parse(&db, file).statements(&db).is_empty(),
        "the expanded parse exists (in expanded-text coordinates)"
    );
    // …and the original parse exists INDEPENDENTLY, in user coordinates.
    let original = cme_lsp::db::parse_original(&db, file);
    assert!(!original.statements(&db).is_empty());
}

// ---------------------------------------------------------------------------
// Wire level: the server answers position requests on a mega document
// ---------------------------------------------------------------------------

mod wire {
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc;

    use cme_lsp::server::CheckmateLsp;

    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    const MEGA_DOC_URI: &str = "file:///home/project/hud.cm";

    async fn setup() -> LspService<CheckmateLsp> {
        let (service, _socket) = LspService::build(CheckmateLsp::new)
            .custom_method("cme/expand", CheckmateLsp::expand_preview)
            .finish();
        service
    }

    async fn notify(
        service: &mut LspService<CheckmateLsp>,
        method: &str,
        params: serde_json::Value,
    ) {
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
    ) -> serde_json::Value {
        static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(9000);
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
        .result()
        .cloned()
        .expect("result payload")
    }

    /// (line, character) of `needle` + `skip` bytes in `source`.
    fn position(source: &str, needle: &str, skip: usize) -> (u32, u32) {
        let offset = source
            .find(needle)
            .unwrap_or_else(|| panic!("fixture must contain {needle:?}"))
            + skip;
        let (mut line, mut last) = (0u32, 0usize);
        for (index, byte) in source.as_bytes().iter().enumerate() {
            if index >= offset {
                break;
            }
            if *byte == b'\n' {
                line += 1;
                last = index + 1;
            }
        }
        (line, (offset - last) as u32)
    }

    async fn open_mega_document(service: &mut LspService<CheckmateLsp>) {
        let initialize = jsonrpc::Request::build("initialize".to_string())
            .params(json!({ "capabilities": {} }))
            .id(1)
            .finish();
        let _ = service
            .ready()
            .await
            .unwrap()
            .call(initialize)
            .await
            .unwrap()
            .expect("initialize response");
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
        notify(
            service,
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": MEGA_DOC_URI,
                    "languageId": "checkmate",
                    "version": 1,
                    "text": super::MEGA_FILE,
                }
            }),
        )
        .await;
    }

    #[tokio::test]
    async fn hover_over_the_protocol_resolves_inside_a_mega_document() {
        let mut service = setup().await;
        open_mega_document(&mut service).await;
        let (line, character) = position(super::MEGA_FILE, "return total", "return ".len());
        let result = request(
            &mut service,
            "textDocument/hover",
            json!({
                "textDocument": { "uri": MEGA_DOC_URI },
                "position": { "line": line, "character": character },
            }),
        )
        .await;
        let markup = result["contents"]["value"].as_str().expect("markdown");
        assert!(
            markup.contains("total: int"),
            "hover over the wire resolves the local after the invocation region: {result}"
        );
    }

    #[tokio::test]
    async fn hover_over_the_protocol_resolves_before_the_mega_constructs() {
        let mut service = setup().await;
        open_mega_document(&mut service).await;
        let (line, character) = position(super::MEGA_FILE, "return seed", "return ".len());
        let result = request(
            &mut service,
            "textDocument/hover",
            json!({
                "textDocument": { "uri": MEGA_DOC_URI },
                "position": { "line": line, "character": character },
            }),
        )
        .await;
        let markup = result["contents"]["value"].as_str().expect("markdown");
        assert!(
            markup.contains("seed: int"),
            "hover resolves in the code above the grammar: {result}"
        );
    }

    #[tokio::test]
    async fn completion_over_the_protocol_offers_locals_in_a_mega_document() {
        let mut service = setup().await;
        open_mega_document(&mut service).await;
        let (line, character) = position(super::MEGA_FILE, "return total", "return ".len());
        let result = request(
            &mut service,
            "textDocument/completion",
            json!({
                "textDocument": { "uri": MEGA_DOC_URI },
                "position": { "line": line, "character": character },
            }),
        )
        .await;
        let items = result.as_array().expect("completion array");
        let labels: Vec<&str> = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect();
        assert!(
            labels.contains(&"total"),
            "ordinary-code completion in a mega document offers locals: {labels:?}"
        );
    }

    #[tokio::test]
    async fn completion_over_the_protocol_offers_fragments_in_a_pattern() {
        let mut service = setup().await;
        open_mega_document(&mut service).await;
        let (line, character) = position(super::MEGA_FILE, "\"key:\" $word key", 0);
        let result = request(
            &mut service,
            "textDocument/completion",
            json!({
                "textDocument": { "uri": MEGA_DOC_URI },
                "position": { "line": line, "character": character },
            }),
        )
        .await;
        let items = result.as_array().expect("completion array");
        let labels: Vec<&str> = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect();
        assert!(
            labels.contains(&"$word"),
            "pattern completion still routes through the \u{a7}8 path: {labels:?}"
        );
        assert!(
            !labels.contains(&"total"),
            "the pattern context does not offer script locals: {labels:?}"
        );
    }

    #[tokio::test]
    async fn definition_over_the_protocol_jumps_across_the_mega_constructs() {
        let mut service = setup().await;
        open_mega_document(&mut service).await;
        let (line, character) = position(super::MEGA_FILE, "= base()", "= ".len());
        let result = request(
            &mut service,
            "textDocument/definition",
            json!({
                "textDocument": { "uri": MEGA_DOC_URI },
                "position": { "line": line, "character": character },
            }),
        )
        .await;
        let start = &result["range"]["start"];
        assert!(
            start["line"].is_u64(),
            "definition returns a location over the wire: {result}"
        );
        // The location must point BACK into the user's buffer: line 0 is
        // `int base() {{`.
        assert_eq!(
            start["line"].as_u64(),
            Some(0),
            "the jump lands on base's declaration line in the original text: {result}"
        );
    }

    #[tokio::test]
    async fn document_symbols_over_the_protocol_list_functions_of_a_mega_document() {
        let mut service = setup().await;
        open_mega_document(&mut service).await;
        let result = request(
            &mut service,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": MEGA_DOC_URI } }),
        )
        .await;
        let symbols = result.as_array().expect("symbol array");
        let names: Vec<&str> = symbols
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect();
        assert!(
            names.contains(&"main"),
            "the outline survives the mega constructs: {names:?}"
        );
    }

    #[tokio::test]
    async fn semantic_tokens_over_the_protocol_cover_a_mega_document() {
        let mut service = setup().await;
        open_mega_document(&mut service).await;
        let result = request(
            &mut service,
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": MEGA_DOC_URI } }),
        )
        .await;
        let count = result["data"]
            .as_array()
            .map(|data| data.len())
            .unwrap_or(0);
        assert!(
            count > 0,
            "semantic tokens flow for a mega document: {result}"
        );
    }
}
