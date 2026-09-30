use cme_compiler::check::prepare;
use cme_compiler::mods::{LoadedModule, assemble};
use cme_compiler::schema::{SchemaContext, SchemaSet, parse_schema_file};
use cme_core::ast::{ExprKind, ImportBinding, StmtKind};
use cme_interp::{Interpreter, Value};

fn modules(sources: &[(&str, &str)]) -> cme_compiler::mods::AssembledProgram {
    let modules: Vec<LoadedModule> = sources
        .iter()
        .map(|(name, source)| LoadedModule {
            module_path: vec!["self".into(), (*name).into()],
            display_path: format!("src/{name}.cm"),
            source: (*source).into(),
        })
        .collect();
    assemble(&modules)
}

fn schema() -> SchemaContext {
    let file = parse_schema_file(
        "schema demo 1.0.0\n\nsince 1.0.0 capability one {\n    int Read()\n}\n\nsince 1.0.0 capability two {\n    int Read()\n}\n",
    );
    assert!(file.is_clean());
    SchemaContext::grant_all(SchemaSet::build(vec![file.file.unwrap()]).unwrap())
}

#[test]
fn parser_keeps_all_three_import_spellings() {
    let parsed = cme_compiler::parse_source(
        "import demo.one\nimport demo.two as peach\nimport self.rules as *\n",
    );
    assert!(parsed.is_clean(), "{:?}", parsed.diagnostics);
    assert!(matches!(
        parsed.statements[0].kind,
        StmtKind::Import { binding: None, .. }
    ));
    assert!(
        matches!(parsed.statements[1].kind, StmtKind::Import { binding: Some(ImportBinding::Alias(ref name)), .. } if name == "peach")
    );
    assert!(matches!(
        parsed.statements[2].kind,
        StmtKind::Import {
            binding: Some(ImportBinding::Glob),
            ..
        }
    ));
}

#[test]
fn capability_alias_and_star_lower_to_the_full_path() {
    let contract = schema();
    for (import, call) in [
        ("import demo.one", "demo.one.Read()"),
        ("import demo.one as peach", "peach.Read()"),
        ("import demo.one as *", "Read()"),
    ] {
        let source = format!("{import}\nint main() {{\n    return {call}\n}}\n");
        let parsed = cme_compiler::parse_source(&source);
        assert!(parsed.is_clean(), "{:?}", parsed.diagnostics);
        let (resolved, errors) = prepare(&parsed.statements, &[], Some(&contract));
        assert!(errors.is_empty(), "{import}: {errors:?}");
        let StmtKind::FuncDecl { body, .. } = &resolved[1].kind else {
            panic!()
        };
        let StmtKind::Return { value: Some(expr) } = &body.stmts[0].kind else {
            panic!()
        };
        assert!(
            matches!(&expr.kind, ExprKind::PathCall { path, .. } if path == &vec!["demo", "one", "Read"])
        );
    }
}

#[test]
fn self_alias_star_and_qualified_types_execute() {
    let program = modules(&[
        (
            "main",
            "import self.util as u\nimport self.util as *\nint main() {\n    u.Point point = u.Point(value: twice(21))\n    return point.value\n}\n",
        ),
        (
            "util",
            "struct Point {\n    int value\n}\nint twice(int x) {\n    return x + x\n}\n",
        ),
    ]);
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let (resolved, errors) = prepare(&program.statements, &program.ranges, None);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        Interpreter::new(&resolved).invoke("main", &[]),
        Ok(Value::Int(42))
    );
}

#[test]
fn imports_are_file_scoped_and_bare_cross_file_calls_need_one() {
    let program = modules(&[
        (
            "a",
            "import self.util as *\nint a() {\n    return twice(1)\n}\n",
        ),
        ("main", "int main() {\n    return twice(21)\n}\n"),
        ("util", "int twice(int x) {\n    return x + x\n}\n"),
    ]);
    let (_, errors) = prepare(&program.statements, &program.ranges, None);
    assert!(
        errors
            .iter()
            .any(|error| error.message().contains("import that module in this file")),
        "{errors:?}"
    );
}

#[test]
fn wildcard_collision_names_both_sources() {
    let contract = schema();
    let parsed = cme_compiler::parse_source(
        "import demo.one as *\nimport demo.two as *\nint main() {\n    return Read()\n}\n",
    );
    let (_, errors) = prepare(&parsed.statements, &[], Some(&contract));
    assert!(
        errors.iter().any(|error| {
            let message = error.message();
            message.contains("demo.one") && message.contains("demo.two") && message.contains("Read")
        }),
        "{errors:?}"
    );
}

#[test]
fn wildcard_collides_with_a_local_declaration() {
    let program = modules(&[
        (
            "main",
            "import self.util as *\nint twice(int x) {\n    return x\n}\nint main() {\n    return 0\n}\n",
        ),
        ("util", "int twice(int x) {\n    return x + x\n}\n"),
    ]);
    let (_, errors) = prepare(&program.statements, &program.ranges, None);
    assert!(
        errors.iter().any(|error| {
            error.message().contains("import binding `twice`")
                && error.message().contains("self.util")
                && error.message().contains("declaration `twice`")
        }),
        "{errors:?}"
    );
}

#[test]
fn plain_import_keeps_the_full_path() {
    let program = modules(&[
        (
            "main",
            "import self.util\nint main() {\n    return twice(21)\n}\n",
        ),
        ("util", "int twice(int x) {\n    return x + x\n}\n"),
    ]);
    let (_, errors) = prepare(&program.statements, &program.ranges, None);
    assert!(
        errors.iter().any(|error| {
            error.message().contains("self.util.twice") && error.message().contains("as *")
        }),
        "{errors:?}"
    );
}

#[test]
fn nested_self_paths_resolve_the_most_specific_module() {
    let sources = vec![
        LoadedModule {
            module_path: vec!["self".into(), "a".into()],
            display_path: "src/a.cm".into(),
            source: "int parent() {\n    return 1\n}\n".into(),
        },
        LoadedModule {
            module_path: vec!["self".into(), "a".into(), "b".into()],
            display_path: "src/a/b.cm".into(),
            source: "int child() {\n    return 42\n}\n".into(),
        },
        LoadedModule {
            module_path: vec!["self".into(), "main".into()],
            display_path: "src/main.cm".into(),
            source: "import self.a.b\nint main() {\n    return self.a.b.child()\n}\n".into(),
        },
    ];
    let program = assemble(&sources);
    let (resolved, errors) = prepare(&program.statements, &program.ranges, None);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        Interpreter::new(&resolved).invoke("main", &[]),
        Ok(Value::Int(42))
    );
}
