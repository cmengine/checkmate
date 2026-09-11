//! The error surface of the Rust host API: kinds, positions, rendering,
//! and the guarantee that no host input can panic the pipeline.

use cme_api::{Engine, ErrorKind, ExecutionError, ExecutionLimits, Value};

fn engine() -> Engine {
    Engine::new()
}

#[test]
fn error_kinds_map_one_to_one_from_the_interpreter() {
    let program = engine()
        .load_source("int main() {\nreturn 1 / 0\n}\n")
        .unwrap();
    let ctx = engine().create_context(&program, ExecutionLimits::default());
    assert_eq!(
        ctx.invoke("main", &[]).unwrap_err().kind,
        ErrorKind::Runtime
    );

    let program = engine().load_source("int f() {\nreturn 1\n}\n").unwrap();
    let ctx = engine().create_context(&program, ExecutionLimits::default());
    assert_eq!(
        ctx.invoke("g", &[]).unwrap_err().kind,
        ErrorKind::UnknownEntry
    );
}

#[test]
fn execution_error_display_is_the_render() {
    let program = engine().load_source("int f() {\nreturn 1\n}\n").unwrap();
    let ctx = engine().create_context(&program, ExecutionLimits::default());
    let error: ExecutionError = ctx.invoke("g", &[]).unwrap_err();
    assert_eq!(format!("{error}"), error.render());
    assert_eq!(error.render(), "unknown function `g`");
}

#[test]
fn loose_source_errors_render_line_column_prefix() {
    let source = "int f() {\nint x = 1\nreturn x / 0\n}\n";
    let program = engine().load_source(source).unwrap();
    let ctx = engine().create_context(&program, ExecutionLimits::default());
    let error = ctx.invoke("f", &[]).unwrap_err();
    // The span anchors at the failing operation: `x / 0` begins at column 8.
    assert!(
        error.render().starts_with("line 3, column 8:"),
        "{}",
        error.render()
    );
}

#[test]
fn spans_point_at_byte_offsets_of_the_expanded_source() {
    // The division operation begins at the start of its expression.
    let source = "int f() {\nreturn 1 / 0\n}\n";
    let program = engine().load_source(source).unwrap();
    let ctx = engine().create_context(&program, ExecutionLimits::default());
    let error = ctx.invoke("f", &[]).unwrap_err();
    let span = error.span.expect("runtime errors carry spans");
    assert_eq!(&source[span.start..span.end], "1 / 0");
}

#[test]
fn multibyte_lines_keep_character_columns_honest() {
    // The error is on a line with multibyte text before the failing op:
    // columns count CHARACTERS, so the position stays human-meaningful.
    let source = "int f() {\nstr s = \"héllo\" + 1 / 0\nreturn 0\n}\n";
    let program = engine().load_source(source).unwrap();
    let ctx = engine().create_context(&program, ExecutionLimits::default());
    let error = ctx.invoke("f", &[]).unwrap_err();
    assert_eq!(error.line, 2);
    assert!(error.column > 1);
    assert_ne!(error.column, 0);
}

#[test]
fn compile_errors_render_one_line_per_diagnostic() {
    let error = engine()
        .load_source("int a() {\nreturn 1 +\n}\nint b() {\nreturn 2 +\n}\n")
        .unwrap_err();
    assert_eq!(error.messages().len(), error.diagnostics().len());
    assert_eq!(error.messages().len(), error.message().lines().count());
}

#[test]
fn hostile_sources_never_panic_the_loader() {
    // Fuzzing-lite: a batch of adversarial inputs must all fail (or pass)
    // cleanly, never panic.
    let hostile = [
        "",
        "\n",
        "\u{0}",
        "int f() {\nreturn 1\n",
        "int f() {\nreturn \"\n}\n",
        "/*",
        "*/ int f() {\nreturn 1\n}\n",
        "int f() { return $ }\n",
        "int f() {\nreturn 99999999999999999999999999\n}\n",
        "int f() {\nreturn f(f(f(f(f(f(f(f(1))))))))\n}\n",
        "struct s {\nint x\n}\nint f() {\nreturn s()\n}\n",
        "int f() {\nreturn [1, 2\n}\n",
        "int 1f() {\nreturn 1\n}\n",
        "int f() {\nwhile (true) {\n}\nreturn 1\n}\n",
        "enum e {\nA()\n}\nint f() {\nreturn e.B()\n}\n",
        "int f() {\nint x = \nreturn 1\n}\n",
        "\"just a string\"",
        "}}}}",
        "int f() {\nreturn 1\n}\nint f() {\nreturn 2\n}\n",
    ];
    for source in hostile {
        let _ = engine().load_source(source);
    }
}

#[test]
fn hostile_arguments_never_panic_invocation() {
    let program = engine()
        .load_source("int echo(int v) {\nreturn v\n}\n")
        .unwrap();
    let ctx = engine().create_context(&program, ExecutionLimits::default());
    let hostile = [
        vec![Value::Struct {
            name: String::new(),
            fields: vec![],
        }],
        vec![Value::Enum {
            name: String::new(),
            variant: String::new(),
            payload: vec![Value::Void],
        }],
        vec![Value::Map(vec![(Value::Void, Value::Void)])],
        vec![Value::Array(vec![Value::Array(vec![])])],
        vec![Value::Float(f64::NAN)],
        vec![Value::Float(f64::INFINITY)],
        vec![Value::Int(i64::MAX)],
        vec![Value::Str("\u{0}\n\t🚀".into())],
    ];
    for args in hostile {
        let _ = ctx.invoke("echo", &args);
    }
}

#[test]
fn deeply_nested_values_round_trip_without_recursion_limits() {
    // 64-deep nesting is far beyond what any real mod ships, and the
    // walker handles it fine.
    let mut value = Value::Int(1);
    for _ in 0..64 {
        value = Value::Array(vec![value]);
    }
    let program = engine()
        .load_source("int[][] wrap(int[] v) {\nreturn [v]\n}\n")
        .unwrap();
    let ctx = engine().create_context(&program, ExecutionLimits::default());
    let result = ctx.invoke("wrap", &[value.clone()]).unwrap();
    // Peel the 64 host layers plus the one script-side wrap.
    let mut current = &result;
    let mut depth = 0;
    while let Value::Array(inner) = current {
        current = &inner[0];
        depth += 1;
    }
    assert_eq!(current, &Value::Int(1));
    assert!(depth >= 65, "peeled {depth} layers");
}

#[test]
fn the_fuel_error_names_the_safepoint_span() {
    let program = engine()
        .load_source("int spin() {\nwhile (true) {\n}\nreturn 0\n}\n")
        .unwrap();
    let ctx = engine().create_context(
        &program,
        ExecutionLimits {
            fuel: Some(10),
            ..ExecutionLimits::default()
        },
    );
    let error = ctx.invoke("spin", &[]).unwrap_err();
    assert!(error.span.is_some(), "{error:?}");
    assert_eq!(error.line, 2);
}
