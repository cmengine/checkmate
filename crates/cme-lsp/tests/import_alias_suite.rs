use std::sync::Arc;

use cme_compiler::schema::{SchemaContext, SchemaSet, parse_schema_file};
use cme_lsp::analysis::Analysis;
use cme_lsp::features::completion::completions;

fn labels(source: &str, marker: &str) -> Vec<String> {
    let schema = parse_schema_file(
        "schema demo 1.0.0\n\nsince 1.0.0 capability one {\n    int Read()\n    void Write(str text)\n}\n",
    );
    assert!(schema.is_clean());
    let context = SchemaContext::grant_all(SchemaSet::build(vec![schema.file.unwrap()]).unwrap());
    let parsed = cme_compiler::parse_source(source);
    let analysis = Analysis::build_with_schema(
        source,
        &parsed.statements,
        Some(Arc::new(context)),
        Vec::new(),
    );
    let offset = source.rfind(marker).unwrap() + marker.len();
    completions(&analysis, source, offset)
        .into_iter()
        .map(|item| item.label)
        .collect()
}

#[test]
fn alias_completes_capability_members() {
    let source = "import demo.one as peach\nint main() {\n    peach.\n    return 0\n}\n";
    let items = labels(source, "peach.");
    assert!(items.contains(&"Read".to_string()), "{items:?}");
    assert!(items.contains(&"Write".to_string()), "{items:?}");
}

#[test]
fn wildcard_completes_bare_members() {
    let source = "import demo.one as *\nint main() {\n    Re\n    return 0\n}\n";
    let items = labels(source, "Re");
    assert!(items.contains(&"Read".to_string()), "{items:?}");
}
