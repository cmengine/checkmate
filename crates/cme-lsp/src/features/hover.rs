//! Hover: resolution under the cursor rendered as Checkmate markdown.

use crate::analysis::Analysis;
use crate::resolve::{Resolved, hover_markdown};
use tower_lsp_server::ls_types;

/// Builds the hover response for `offset`, if something resolvable is
/// there.
pub fn hover(analysis: &Analysis<'_>, offset: usize) -> Option<ls_types::Hover> {
    let resolved = analysis.resolve(offset)?;
    Some(ls_types::Hover {
        contents: ls_types::HoverContents::Markup(ls_types::MarkupContent {
            kind: ls_types::MarkupKind::Markdown,
            value: hover_markdown(&resolved),
        }),
        range: None,
    })
}

/// The signature line of a resolution, used by completion details too.
pub fn signature(resolved: &Resolved<'_>) -> String {
    match resolved {
        Resolved::Local(local) => format!(
            "{}: {}",
            local.name,
            crate::analysis::render_type(&local.ty)
        ),
        Resolved::Function { function, .. } => {
            let params: Vec<String> = function
                .params
                .iter()
                .map(|param| format!("{} {}", crate::analysis::render_type(&param.ty), param.name))
                .collect();
            format!(
                "{} {}({})",
                crate::analysis::render_type(&function.return_ty),
                function.name,
                params.join(", ")
            )
        }
        Resolved::Struct(struct_type) => format!("struct {}", struct_type.name),
        Resolved::Enum(enum_type) => format!("enum {}", enum_type.name),
        Resolved::Variant { enum_type, variant } => format!("{}.{}", enum_type.name, variant.name),
        Resolved::Field { struct_type, index } => {
            let (name, ty, _) = &struct_type.fields[*index];
            format!("{}: {}", name, crate::analysis::render_type(ty))
        }
        Resolved::ImportSegment { import, .. } => {
            let path: Vec<&str> = import
                .segments
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            format!("import {}", path.join("."))
        }
        Resolved::BuiltinType { name, .. } => name.clone(),
        Resolved::BuiltinConstructor { name, .. } => name.clone(),
        Resolved::ArrayLength => "array.length".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analysis;

    #[test]
    fn hover_on_local_shows_type() {
        let source = "int main() {\n    int hp = 100\n    return hp\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let analysis = Analysis::build(source, &outcome.statements);
        let offset = source.find("return hp").unwrap() + "return ".len();
        let hover = hover(&analysis, offset).expect("hp resolves");
        let ls_types::HoverContents::Markup(markup) = hover.contents else {
            panic!("markdown hover");
        };
        assert!(markup.value.contains("hp: int"), "{}", markup.value);
    }

    #[test]
    fn hover_on_function_shows_signature() {
        let source = "int add(int a, int b) {\n    return a + b\n}\n\nint main() {\n    return add(1, 2)\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let analysis = Analysis::build(source, &outcome.statements);
        let offset = source.rfind("add").unwrap();
        let hover = hover(&analysis, offset).expect("add resolves");
        let ls_types::HoverContents::Markup(markup) = hover.contents else {
            panic!("markdown hover");
        };
        assert!(
            markup.value.contains("int add(int a, int b)"),
            "{}",
            markup.value
        );
    }
}
