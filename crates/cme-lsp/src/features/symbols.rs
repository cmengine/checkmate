//! Document symbols: the outline tree over top-level declarations, with
//! impl blocks as containers for their members (§10.4).

use tower_lsp_server::ls_types;

use crate::analysis::Analysis;
use crate::convert::LineIndex;

/// The document symbol tree for a script file.
pub fn document_symbols(
    analysis: &Analysis<'_>,
    line_index: &LineIndex,
    text: &str,
) -> Vec<ls_types::DocumentSymbol> {
    let range = |span: cme_core::Span| line_index.range(text, span);
    let mut symbols = Vec::new();

    for import in &analysis.imports {
        let path: Vec<&str> = import
            .segments
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        symbols.push(symbol(
            format!("import {}", path.join(".")),
            ls_types::SymbolKind::MODULE,
            range(import.span),
            range(
                import
                    .segments
                    .first()
                    .map(|(_, span)| *span)
                    .unwrap_or(import.span),
            ),
            Vec::new(),
        ));
    }
    for struct_type in &analysis.structs {
        symbols.push(symbol(
            struct_type.name.clone(),
            ls_types::SymbolKind::STRUCT,
            range(struct_type.span),
            range(struct_type.name_span),
            struct_type
                .fields
                .iter()
                .map(|(name, _, span)| {
                    symbol(
                        name.clone(),
                        ls_types::SymbolKind::FIELD,
                        range(*span),
                        range(*span),
                        Vec::new(),
                    )
                })
                .collect(),
        ));
    }
    for enum_type in &analysis.enums {
        symbols.push(symbol(
            enum_type.name.clone(),
            ls_types::SymbolKind::ENUM,
            range(enum_type.span),
            range(enum_type.name_span),
            enum_type
                .variants
                .iter()
                .map(|variant| {
                    symbol(
                        variant.name.clone(),
                        ls_types::SymbolKind::ENUM_MEMBER,
                        range(variant.name_span),
                        range(variant.name_span),
                        Vec::new(),
                    )
                })
                .collect(),
        ));
    }
    for impl_block in &analysis.impls {
        let target: Vec<&str> = impl_block
            .target
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        let members: Vec<ls_types::DocumentSymbol> = impl_block
            .member_indices
            .iter()
            .filter_map(|index| analysis.functions.get(*index))
            .map(|function| {
                symbol(
                    function.name.clone(),
                    ls_types::SymbolKind::METHOD,
                    range(function.span),
                    range(function.name_span),
                    Vec::new(),
                )
            })
            .collect();
        symbols.push(symbol(
            format!("impl {}", target.join(".")),
            ls_types::SymbolKind::MODULE,
            range(impl_block.span),
            range(
                impl_block
                    .target
                    .first()
                    .map(|(_, span)| *span)
                    .unwrap_or(impl_block.span),
            ),
            members,
        ));
    }
    for function in &analysis.functions {
        if function.impl_index.is_some() {
            continue; // listed under their impl block
        }
        symbols.push(symbol(
            function.name.clone(),
            ls_types::SymbolKind::FUNCTION,
            range(function.span),
            range(function.name_span),
            Vec::new(),
        ));
    }
    symbols.sort_by_key(|symbol| symbol.range.start);
    symbols
}

fn symbol(
    name: String,
    kind: ls_types::SymbolKind,
    range: ls_types::Range,
    selection_range: ls_types::Range,
    children: Vec<ls_types::DocumentSymbol>,
) -> ls_types::DocumentSymbol {
    #[allow(deprecated)] // the field is mandatory in this ls-types revision
    ls_types::DocumentSymbol {
        name,
        detail: None,
        kind,
        tags: None,
        range,
        selection_range,
        children: if children.is_empty() {
            None
        } else {
            Some(children)
        },
        deprecated: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analysis;
    use crate::convert::LineIndex;

    #[test]
    fn outline_lists_declarations_and_impl_members() {
        let source = "import engine.graphics\n\nstruct vec2 {\n    float x\n    float y\n}\n\nimpl vec2 {\n    float dot(vec2 other) {\n        return 0.0\n    }\n}\n\nint main() {\n    return 0\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let analysis = Analysis::build(source, &outcome.statements);
        let index = LineIndex::new(source);
        let symbols = document_symbols(&analysis, &index, source);

        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["import engine.graphics", "vec2", "impl vec2", "main"],
            "impl blocks render with target prefix; got {names:?}"
        );

        // The impl block carries its member as a child.
        let impl_symbol = symbols
            .iter()
            .find(|s| s.name.starts_with("impl"))
            .expect("impl in outline");
        let children = impl_symbol.children.as_ref().expect("members as children");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "dot");
        assert_eq!(children[0].kind, ls_types::SymbolKind::METHOD);
    }
}
