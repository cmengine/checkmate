//! Compilation-stage tests for the Rust host API: what the [`Engine`]
//! accepts, what it refuses, and the exact error surface for every refusal
//! (WHITEPAPER §13.1: the gate must match the CLI's — no diagnostic, no
//! program).

use cme_api::{Engine, LoadError, ProgramKind};

const CLEAN: &str = "int add(int a, int b) {\nreturn a + b\n}\n";

fn compile(source: &str) -> Result<cme_api::CompiledProgram, cme_api::CompileError> {
    Engine::new().load_source(source)
}

#[test]
fn a_clean_program_compiles_and_reports_its_entry_points() {
    let program = compile(CLEAN).expect("clean source compiles");
    assert_eq!(program.entry_points(), ["add"]);
    assert!(!program.is_mod());
    assert!(program.manifest().is_none());
    assert_eq!(program.statement_count(), 1);
    assert!(program.modules().is_none());
    assert!(matches!(
        program.kind(),
        ProgramKind::Source { name: None, .. }
    ));
}

#[test]
fn entry_points_list_every_function_in_declaration_order() {
    let program = compile(concat!(
        "int first() {\nreturn 1\n}\n",
        "float second() {\nreturn 2.0\n}\n",
        "void third() {\n}\n",
    ))
    .unwrap();
    assert_eq!(program.entry_points(), ["first", "second", "third"]);
}

#[test]
fn interface_targets_list_impl_paths_deduplicated_in_order() {
    let program = compile(concat!(
        "struct vec2 {\nfloat x\n}\n",
        "impl vec2 {\nfloat getX(vec2 v) {\nreturn v.x\n}\n}\n",
        "impl vec2 {\nfloat getY(vec2 v) {\nreturn 0.0\n}\n}\n",
        "impl engine.gamemode {\nvoid OnTick() {\n}\n}\n",
    ))
    .unwrap();
    assert_eq!(program.interface_targets(), ["vec2", "engine.gamemode"]);
}

#[test]
fn a_syntax_error_refuses_the_load_with_a_position() {
    let error = compile("int f() {\nreturn 1\n").expect_err("unclosed brace must fail");
    assert!(!error.diagnostics().is_empty());
    assert_eq!(error.messages().len(), error.diagnostics().len());
    let message = error.message();
    // The unbalanced brace is diagnosed at end-of-file (3:1 here).
    assert!(message.starts_with("3:1:"), "rendered: {message}");
}

#[test]
fn a_type_error_refuses_the_load_and_names_the_mistake() {
    // §A.4: cross-type arithmetic is never permitted.
    let error = compile("int f() {\nreturn 1 + \"x\"\n}\n").expect_err("cross-type + must fail");
    let message = error.message();
    assert!(
        message.to_lowercase().contains("type") || message.contains('+'),
        "rendered: {message}"
    );
}

#[test]
fn an_unknown_type_refuses_the_load() {
    let error = compile("frobnicate f() {\nreturn 1\n}\n").expect_err("unknown type must fail");
    assert!(!error.messages().is_empty());
}

#[test]
fn a_void_variable_refuses_the_load() {
    // §2.4: void is a function return type, not a value type.
    let error = compile("void f() {\n}\nint main() {\nvoid v = f()\nreturn 0\n}\n")
        .expect_err("void local must fail");
    assert!(!error.messages().is_empty());
}

#[test]
fn undeclared_calls_refuse_the_load() {
    let error = compile("int main() {\nreturn ghost()\n}\n").expect_err("unknown call must fail");
    assert!(!error.messages().is_empty());
}

#[test]
fn multiple_diagnostics_all_surface_in_order() {
    // Recovery never stops: both broken declarations report.
    let error = compile(concat!(
        "int a() {\nreturn 1 +\n}\n",
        "int b() {\nreturn unknown()\n}\n",
    ))
    .expect_err("both functions are broken");
    assert!(
        error.messages().len() >= 2,
        "expected several diagnostics, got: {error}"
    );
}

#[test]
fn an_empty_source_compiles_to_an_empty_program() {
    let program = compile("").expect("empty source has no diagnostics");
    assert!(program.entry_points().is_empty());
    assert_eq!(program.statement_count(), 0);
}

#[test]
fn comments_only_sources_compile_empty() {
    let program = compile("// nothing here\n/* really nothing */\n").unwrap();
    assert_eq!(program.statement_count(), 0);
}

#[test]
fn standalone_self_imports_refuse_outside_a_mod() {
    // §10: self-rooted imports only resolve inside a mod tree.
    let error = compile("import self.helper\nint main() {\nreturn 1\n}\n")
        .expect_err("standalone self-import must fail");
    assert!(error.message().contains("import"), "rendered: {error}");
}

#[test]
fn a_lex_error_refuses_the_load_with_a_precise_position() {
    let error = compile("int f() {\nreturn \"unterminated\n}\n")
        .expect_err("unterminated string must fail");
    let message = error.message();
    assert!(message.starts_with("2:"), "rendered: {message}");
}

#[test]
fn full_language_surface_compiles_through_the_api() {
    // syntax.cm is the full-surface fixture; the API accepts everything
    // the CLI accepts.
    let source = include_str!("../../../syntax.cm");
    let program = compile(source).expect("syntax.cm is the language fixture");
    assert!(program.entry_points().contains(&"main".to_string()));
}

#[test]
fn basic_fixture_compiles_through_the_api() {
    let source = include_str!("../../../basic.cm");
    let program = compile(source).expect("basic.cm is clean");
    assert_eq!(
        program.entry_points(),
        ["fib", "append_report", "classify", "main"]
    );
}

#[test]
fn load_file_reports_io_failures_distinctly() {
    let error = Engine::new()
        .load_file("/definitely/not/a/real/path.cm")
        .expect_err("missing file must fail");
    assert!(matches!(error, LoadError::Io(message) if message.contains("cannot read")));
}

#[test]
fn load_file_compiles_and_names_the_file_in_diagnostics() {
    let dir = std::env::temp_dir().join("cme_api_load_file_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("broken_api_fixture.cm");
    std::fs::write(&path, "int f() {\nreturn 1 +\n}\n").unwrap();

    let error = Engine::new()
        .load_file(&path)
        .expect_err("broken file must fail");
    let LoadError::Compile(compile_error) = error else {
        panic!("expected a compile error, got IO");
    };
    let message = compile_error.message();
    assert!(
        message.starts_with(&format!("{}:", path.display())),
        "rendered: {message}"
    );
    std::fs::remove_file(&path).ok();
}

#[test]
fn load_file_happy_path_carries_the_name() {
    let dir = std::env::temp_dir().join("cme_api_load_file_ok");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("clean_api_fixture.cm");
    std::fs::write(&path, CLEAN).unwrap();

    let program = Engine::new().load_file(&path).expect("clean file compiles");
    match program.kind() {
        ProgramKind::Source { name, .. } => {
            assert_eq!(name.as_deref(), Some(path.to_str().unwrap()))
        }
        other => panic!("expected a source program, got {other:?}"),
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn megaprogram_sources_expand_before_checking() {
    // A JSON megaprogram becomes a real map literal: the API expands it
    // exactly like `cme check` does.
    let source = concat!(
        "grammar json {\nskip [ ' ', '\\t', '\\r', '\\n' ]\n",
        "rule value {\noneof {\nnull => \"null\"\n",
        "bool => oneof { t => \"true\", f => \"false\" }\n",
        "number => number\nstring => $str text\n",
        "array => ( \"[\" each sep \",\" { value } as items \"]\" )\n",
        "object => ( \"{\" each sep \",\" { member } as fields \"}\" )\n}\n}\n",
        "rule member {\n$str key \":\" value\n}\n",
        "rule number {\noptional { \"-\" }\n",
        "oneof { zero => \"0\", pos => ( [1-9] as first scan [0-9] as rest ) }\n",
        "optional { \".\" scan [0-9] as frac }\n}\n}\n",
        "mega value(json.value as v) {\n@toValue($v)\n}\n",
    );
    // The grammar/mega DECLARATION alone must expand away cleanly; the
    // empty program that remains has no entry points.
    let program = compile(source).expect("declaration-only megaprogram compiles");
    assert!(program.entry_points().is_empty());
}

#[test]
fn the_source_kind_exposes_the_expanded_text() {
    let program = compile(CLEAN).unwrap();
    assert_eq!(program.source(), CLEAN);
}

#[test]
fn a_broken_megaprogram_region_fails_with_rendered_diagnostics() {
    // An invocation whose pattern cannot consume its region fails expansion.
    let error = compile(concat!(
        "grammar g {\nskip [ ' ' ]\nrule item {\n\"a\" $word w\n}\n}\n",
        "mega item(g.item as i) {\nint x = 1\n}\n",
        "item! {\nb\n}\n",
    ))
    .expect_err("region must fail to match");
    assert!(!error.messages().is_empty(), "rendered: {error}");
}

#[test]
fn statements_are_inspectable_but_not_mutable() {
    let program = compile(CLEAN).unwrap();
    let statements = program.statements();
    assert_eq!(statements.len(), 1);
    match &statements[0].kind {
        cme_core::ast::StmtKind::FuncDecl { name, .. } => assert_eq!(name, "add"),
        other => panic!("expected a function declaration, got {other:?}"),
    }
}

#[test]
fn compile_error_is_a_std_error() {
    fn takes_error(error: &dyn std::error::Error) -> String {
        error.to_string()
    }
    let error = compile("int f() {\nreturn 1 +\n}\n").unwrap_err();
    assert!(!takes_error(&error).is_empty());
}
