//! Shared helpers for the cme-lsp integration suites: build an analysis
//! from source text and aim features at named positions without hand
//! counting byte offsets.

#![allow(dead_code)]

use cme_lsp::analysis::Analysis;
use tower_lsp_server::ls_types;

/// Parses `source` (tolerantly) and builds its symbol table. The parse
/// outcome is leaked so the analysis can borrow its AST for the rest of
/// the test — the amount of data is tiny and tests are short-lived.
pub fn analysis(source: &str) -> Analysis<'_> {
    let outcome: &'static mut cme_compiler::ParseOutcome =
        Box::leak(Box::new(cme_compiler::parse_source(source)));
    Analysis::build(source, &outcome.statements)
}

/// The byte offset just after `occurrence`'s first `skip` bytes, searching
/// from the start of `source`.
pub fn offset_of(source: &str, needle: &str, skip: usize) -> usize {
    let start = source.find(needle).unwrap_or_else(|| {
        panic!("fixture must contain {needle:?}:\n{source}");
    });
    start + skip
}

/// The byte offset just after the LAST occurrence of `needle`.
pub fn last_offset_of(source: &str, needle: &str) -> usize {
    let start = source.rfind(needle).unwrap_or_else(|| {
        panic!("fixture must contain {needle:?}:\n{source}");
    });
    start + needle.len()
}

/// Completion items at `offset`.
pub fn complete_at(
    analysis: &Analysis<'_>,
    source: &str,
    offset: usize,
) -> Vec<ls_types::CompletionItem> {
    cme_lsp::features::completion::completions(analysis, source, offset)
}

/// Completion item labels at `offset`.
pub fn labels_at(analysis: &Analysis<'_>, source: &str, offset: usize) -> Vec<String> {
    complete_at(analysis, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect()
}

/// Completion (label, insert_text) pairs at `offset` — the "filling" view.
pub fn fills_at(
    analysis: &Analysis<'_>,
    source: &str,
    offset: usize,
) -> Vec<(String, Option<String>)> {
    complete_at(analysis, source, offset)
        .into_iter()
        .map(|item| (item.label, item.insert_text))
        .collect()
}

/// (label, detail) pairs at `offset` — the "what does it mean" view.
pub fn details_at(
    analysis: &Analysis<'_>,
    source: &str,
    offset: usize,
) -> Vec<(String, Option<String>)> {
    complete_at(analysis, source, offset)
        .into_iter()
        .map(|item| (item.label, item.detail))
        .collect()
}

/// Hover markdown at `offset`.
pub fn hover_at(analysis: &Analysis<'_>, offset: usize) -> Option<String> {
    let hover = cme_lsp::features::hover::hover(analysis, offset)?;
    let ls_types::HoverContents::Markup(markup) = hover.contents else {
        panic!("hover must render markdown");
    };
    Some(markup.value)
}

/// The definition span at `offset`.
pub fn definition_at(analysis: &Analysis<'_>, offset: usize) -> Option<cme_core::Span> {
    cme_lsp::features::definition::definition(analysis, offset)
}

/// The reference spans at `offset`.
pub fn references_at(
    analysis: &Analysis<'_>,
    offset: usize,
    include_declaration: bool,
) -> Vec<cme_core::Span> {
    cme_lsp::features::definition::references(analysis, offset, include_declaration)
}

/// The semantic-token payload, decoded back to absolute (line, character,
/// length, type, modifiers) rows.
pub fn decoded_tokens(
    analysis: &Analysis<'_>,
    line_index: &cme_lsp::convert::LineIndex,
    text: &str,
) -> Vec<(u32, u32, u32, u32, u32)> {
    let tokens = cme_lsp::features::tokens::semantic_tokens(analysis, line_index, text);
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

/// The document-symbol outline as (name, kind, child names) triples.
pub fn outline(
    analysis: &Analysis<'_>,
    line_index: &cme_lsp::convert::LineIndex,
    text: &str,
) -> Vec<(String, ls_types::SymbolKind, Vec<String>)> {
    cme_lsp::features::symbols::document_symbols(analysis, line_index, text)
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
