use std::path::Path;

use cme_interp::Value;

/// Explicit contracts: adding a fixture requires selecting its expected result.
/// The syntax snapshot comes from its documented report, including failures=0.
pub fn expected_result(path: &Path) -> Value {
    match path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("fixture name")
    {
        "basic.cm" => Value::Int(3),
        "syntax.cm" => Value::Str(include_str!("../fixtures/programs/syntax.expected").to_string()),
        "adt.cm"
        | "arithmetic.cm"
        | "collections.cm"
        | "composed_semantics.cm"
        | "control_flow.cm"
        | "functions.cm"
        | "impl_blocks.cm"
        | "infer_crystallization.cm"
        | "mega.cm"
        | "newline_rules.cm"
        | "question_operator.cm"
        | "strings.cm"
        | "value_semantics.cm" => Value::Int(0),
        name => panic!("register the intended result for fixture {name}"),
    }
}

pub fn program_source(path: &Path) -> String {
    let source = std::fs::read_to_string(path).expect("readable fixture");
    if cme_compiler::mega::expand::mentions_megaprogram(&source) {
        cme_compiler::mega::expand::expand_source(&source)
            .unwrap_or_else(|errors| panic!("{path:?}: expansion: {errors:?}"))
            .expanded
    } else {
        source
    }
}
