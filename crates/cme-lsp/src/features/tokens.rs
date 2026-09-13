//! Semantic tokens: every identifier classified through the same
//! resolution path hover uses, delta-encoded per the LSP. Declaration
//! tokens carry the `declaration` modifier; impl members report `method`.
//!
//! Keywords, literals, and operators stay with the tree-sitter grammar —
//! these tokens complement it (Zed's `semantic_tokens: "combined"`).

use tower_lsp_server::ls_types;

use crate::analysis::{Analysis, Tok};
use crate::convert::LineIndex;
use crate::resolve::Resolved;

/// The legend this server emits. Index positions are part of the wire
/// format.
pub fn legend() -> ls_types::SemanticTokensLegend {
    ls_types::SemanticTokensLegend {
        token_types: vec![
            ls_types::SemanticTokenType::NAMESPACE,
            ls_types::SemanticTokenType::FUNCTION,
            ls_types::SemanticTokenType::METHOD,
            ls_types::SemanticTokenType::PROPERTY,
            ls_types::SemanticTokenType::VARIABLE,
            ls_types::SemanticTokenType::PARAMETER,
            ls_types::SemanticTokenType::ENUM_MEMBER,
            ls_types::SemanticTokenType::ENUM,
            ls_types::SemanticTokenType::STRUCT,
        ],
        token_modifiers: vec![ls_types::SemanticTokenModifier::DECLARATION],
    }
}

/// The full semantic-token payload for a document. Tokens are emitted in
/// document order with delta-encoded line/start fields, per the LSP.
pub fn semantic_tokens(
    analysis: &Analysis<'_>,
    line_index: &LineIndex,
    text: &str,
) -> ls_types::SemanticTokens {
    // First pass: absolute positions.
    let mut absolute: Vec<(u32, u32, u32, u32, u32)> = Vec::new();
    for token in &analysis.tokens {
        let Tok::Ident(name) = &token.kind else {
            continue;
        };
        let Some(resolved) = analysis.resolve(token.span.start) else {
            continue;
        };
        let (token_type, is_declaration) = match &resolved {
            Resolved::ImportSegment { .. } => (0, false),
            Resolved::Function { function, .. } => {
                if function.impl_index.is_some() {
                    (2, token.span == function.name_span)
                } else {
                    (1, token.span == function.name_span)
                }
            }
            Resolved::Field { .. } => (3, false),
            Resolved::Local(local) => match local.kind {
                crate::analysis::LocalKind::Param => (5, token.span == local.span),
                _ => (4, token.span == local.span),
            },
            Resolved::Variant { .. } => (6, false),
            Resolved::Enum(_) => (7, false),
            Resolved::Struct(_) => (8, false),
            // Built-ins are keywords or carry no semantics worth painting.
            Resolved::BuiltinType { .. }
            | Resolved::BuiltinConstructor { .. }
            | Resolved::ArrayLength => continue,
        };

        let position = line_index.position(text, token.span.start);
        let length = name.chars().map(char::len_utf16).sum::<usize>() as u32;
        let modifiers = if is_declaration { 1 } else { 0 }; // DECLARATION
        absolute.push((
            position.line,
            position.character,
            length,
            token_type,
            modifiers,
        ));
    }

    // Second pass: delta-encode.
    let (mut previous_line, mut previous_character) = (0u32, 0u32);
    let data = absolute
        .into_iter()
        .map(|(line, character, length, token_type, modifiers)| {
            let delta_line = line - previous_line;
            let delta_start = if delta_line == 0 {
                character - previous_character
            } else {
                character
            };
            previous_line = line;
            previous_character = character;
            ls_types::SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type,
                token_modifiers_bitset: modifiers,
            }
        })
        .collect();

    ls_types::SemanticTokens {
        result_id: None,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analysis;

    #[test]
    fn tokens_are_sorted_and_delta_encoded() {
        let source = "struct vec2 {\n    float x\n}\n\nint main() {\n    infer v = vec2(x: 1.0)\n    return v.x\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let analysis = Analysis::build(source, &outcome.statements);
        let index = LineIndex::new(source);
        let tokens = semantic_tokens(&analysis, &index, source);

        // Decode back and confirm monotonic positions.
        let mut line = 0u32;
        let mut character = 0u32;
        for token in &tokens.data {
            let new_line = line + token.delta_line;
            let new_character = if token.delta_line > 0 {
                token.delta_start
            } else {
                character + token.delta_start
            };
            assert!(
                new_line > line || new_character >= character,
                "token positions must not move backwards"
            );
            line = new_line;
            character = new_character;
        }
        // struct name, field name, main, v (decl), vec2 (use), v (use)
        assert!(tokens.data.len() >= 6, "at least six named tokens");
    }
}
