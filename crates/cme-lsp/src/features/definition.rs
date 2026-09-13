//! Go-to-definition and find-references over the symbol table.

use cme_core::Span;
use tower_lsp_server::ls_types;

use crate::analysis::Analysis;
use crate::convert;
use crate::resolve::Resolved;

/// Goes to the declaration of whatever is under the cursor.
pub fn definition(analysis: &Analysis<'_>, offset: usize) -> Option<Span> {
    let resolved = analysis.resolve(offset)?;
    declaration_span(&resolved)
}

/// The exact span to jump to for a resolution (its name token).
pub fn declaration_span(resolved: &Resolved<'_>) -> Option<Span> {
    match resolved {
        Resolved::Local(local) => Some(local.span),
        Resolved::Function { function, .. } => Some(function.name_span),
        Resolved::Struct(struct_type) => Some(struct_type.name_span),
        Resolved::Enum(enum_type) => Some(enum_type.name_span),
        Resolved::Variant { variant, .. } => Some(variant.name_span),
        Resolved::Field {
            struct_type, index, ..
        } => Some(struct_type.fields[*index].2),
        Resolved::ImportSegment { import, segment } => {
            import.segments.get(*segment).map(|(_, span)| *span)
        }
        // Built-ins have no declaration in this file.
        Resolved::BuiltinType { .. }
        | Resolved::BuiltinConstructor { .. }
        | Resolved::ArrayLength => None,
    }
}

/// All references to the symbol under the cursor, as spans. Resolution runs
/// once for every identifier token in the file, so shadowed names resolve
/// to their own declarations and are excluded naturally.
pub fn references(analysis: &Analysis<'_>, offset: usize, include_declaration: bool) -> Vec<Span> {
    let Some(target) = analysis.resolve(offset) else {
        return Vec::new();
    };
    let Some(target_name) = name_of(&target) else {
        return Vec::new();
    };

    let mut spans = Vec::new();
    for token in &analysis.tokens {
        let crate::analysis::Tok::Ident(ident) = &token.kind else {
            continue;
        };
        if ident != target_name {
            continue;
        }
        let Some(resolved) = analysis.resolve(token.span.start) else {
            continue;
        };
        if !same_symbol(&target, &resolved) {
            continue;
        }
        let is_declaration = declaration_span(&resolved)
            .map(|span| span == token.span)
            .unwrap_or(false);
        if is_declaration && !include_declaration {
            continue;
        }
        spans.push(token.span);
    }
    spans
}

/// The identifier spelling of a resolution, if it has one.
fn name_of<'a>(resolved: &'a Resolved<'_>) -> Option<&'a str> {
    match resolved {
        Resolved::Local(local) => Some(&local.name),
        Resolved::Function { function, .. } => Some(&function.name),
        Resolved::Struct(struct_type) => Some(&struct_type.name),
        Resolved::Enum(enum_type) => Some(&enum_type.name),
        Resolved::Variant { variant, .. } => Some(&variant.name),
        Resolved::Field {
            struct_type, index, ..
        } => Some(&struct_type.fields[*index].0),
        Resolved::ImportSegment { import, segment } => {
            import.segments.get(*segment).map(|(name, _)| name.as_str())
        }
        Resolved::BuiltinType { name, .. } => Some(name),
        Resolved::BuiltinConstructor { name, .. } => Some(name),
        Resolved::ArrayLength => Some("length"),
    }
}

/// Identity of two resolutions: same declaration (by name-token span).
fn same_symbol(left: &Resolved<'_>, right: &Resolved<'_>) -> bool {
    match (left, right) {
        (Resolved::Local(a), Resolved::Local(b)) => a.span == b.span,
        (Resolved::Function { function: a, .. }, Resolved::Function { function: b, .. }) => {
            a.name_span == b.name_span
        }
        (Resolved::Struct(a), Resolved::Struct(b)) => a.name_span == b.name_span,
        (Resolved::Enum(a), Resolved::Enum(b)) => a.name_span == b.name_span,
        (Resolved::Variant { variant: a, .. }, Resolved::Variant { variant: b, .. }) => {
            a.name_span == b.name_span
        }
        (
            Resolved::Field {
                struct_type: a,
                index: i,
                ..
            },
            Resolved::Field {
                struct_type: b,
                index: j,
                ..
            },
        ) => a.name_span == b.name_span && i == j,
        (
            Resolved::ImportSegment {
                import: a,
                segment: i,
            },
            Resolved::ImportSegment {
                import: b,
                segment: j,
            },
        ) => a.span == b.span && i == j,
        (Resolved::BuiltinType { name: a, .. }, Resolved::BuiltinType { name: b, .. }) => a == b,
        (
            Resolved::BuiltinConstructor { name: a, .. },
            Resolved::BuiltinConstructor { name: b, .. },
        ) => a == b,
        (Resolved::ArrayLength, Resolved::ArrayLength) => true,
        _ => false,
    }
}

/// Converts a span to a location in `uri`.
pub fn location(
    line_index: &convert::LineIndex,
    text: &str,
    uri: ls_types::Uri,
    span: Span,
) -> ls_types::Location {
    ls_types::Location {
        uri,
        range: line_index.range(text, span),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analysis;

    #[test]
    fn definition_jumps_to_the_declaration() {
        let source = "int add(int a, int b) {\n    return a + b\n}\n\nint main() {\n    return add(1, 2)\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let analysis = Analysis::build(source, &outcome.statements);
        let use_site = source.rfind("add").unwrap();
        let span = definition(&analysis, use_site).expect("resolves");
        let declared = source.find("int add").unwrap() + "int ".len();
        assert_eq!(span.start, declared);
    }

    #[test]
    fn references_find_uses_but_respect_shadowing() {
        let source = "int twice(int v) {\n    return v + v\n}\n\nint main() {\n    int v = 3\n    return twice(v)\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let analysis = Analysis::build(source, &outcome.statements);

        // All `twice` tokens: declaration + the call site.
        let use_site = source.rfind("twice").unwrap();
        let refs = references(&analysis, use_site, true);
        assert_eq!(refs.len(), 2, "declaration plus one call site");

        // The parameter `v` resolves separately from main's local `v`.
        let param_use = source.find("return v + v").unwrap() + "return ".len();
        let param_refs = references(&analysis, param_use, true);
        assert_eq!(
            param_refs.len(),
            3,
            "parameter declaration plus two body uses"
        );
    }
}
