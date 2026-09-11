//! End-to-end tests for the Rust host application: argument parsing,
//! literal handling, mod loading, impl-member invocation, limits flags,
//! and every failure shape with its exit code.

use cme::Value;
use cme_rust_host::{Outcome, exit_code, parse_arguments, run};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn args<const N: usize>(list: [&str; N]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn args_vec(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn run_simple(list: &[&str]) -> Outcome {
    match parse_arguments(&args_vec(list)) {
        Ok(invocation) => run(&invocation),
        Err(outcome) => outcome,
    }
}

fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("cme_rust_host_{name}_{id}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

#[test]
fn minimal_invocation_parses() {
    let invocation = parse_arguments(&args(["game.cm", "main"])).unwrap();
    assert_eq!(invocation.path, "game.cm");
    assert_eq!(invocation.entry, "main");
    assert!(invocation.args.is_empty());
    assert_eq!(invocation.limits, ExecutionLimits::default());
}

use cme::ExecutionLimits;

#[test]
fn all_option_flags_parse_into_limits() {
    let invocation = parse_arguments(&args([
        "game.cm",
        "main",
        "--fuel",
        "1000",
        "--deadline-ms",
        "50",
        "--depth",
        "64",
    ]))
    .unwrap();
    assert_eq!(invocation.limits.fuel, Some(1000));
    assert_eq!(invocation.limits.deadline_ms, Some(50));
    assert_eq!(invocation.limits.max_call_depth, 64);
}

#[test]
fn zero_flags_mean_unset() {
    let invocation = parse_arguments(&args([
        "game.cm",
        "main",
        "--fuel",
        "0",
        "--deadline-ms",
        "0",
        "--depth",
        "0",
    ]))
    .unwrap();
    assert_eq!(invocation.limits.fuel, None);
    assert_eq!(invocation.limits.deadline_ms, None);
    assert_eq!(invocation.limits.max_call_depth, cme::MAX_CALL_DEPTH);
}

#[test]
fn options_may_precede_positionals() {
    let invocation = parse_arguments(&args(["--fuel", "5", "game.cm", "main", "1", "2"])).unwrap();
    assert_eq!(invocation.limits.fuel, Some(5));
    assert_eq!(invocation.args, vec![Value::Int(1), Value::Int(2)]);
}

#[test]
fn literals_parse_to_every_scalar_kind() {
    let invocation = parse_arguments(&args([
        "a.cm", "f", "42", "-7", "2.5", "true", "false", "\"text\"",
    ]))
    .unwrap();
    assert_eq!(
        invocation.args,
        vec![
            Value::Int(42),
            Value::Int(-7),
            Value::Float(2.5),
            Value::Bool(true),
            Value::Bool(false),
            Value::Str("text".into()),
        ]
    );
}

#[test]
fn bad_literals_and_unknown_options_are_usage_errors() {
    let outcome = run_simple(&["a.cm", "f", "banana"]);
    assert!(matches!(outcome, Outcome::Usage(_)), "{outcome:?}");
    assert_eq!(outcome.exit_code(), exit_code::USAGE);

    let outcome = run_simple(&["a.cm", "f", "nan"]);
    assert!(matches!(outcome, Outcome::Usage(_)), "{outcome:?}");

    let outcome = run_simple(&["a.cm", "f", "--fuel"]);
    assert!(matches!(outcome, Outcome::Usage(_)), "{outcome:?}");

    let outcome = run_simple(&["a.cm", "f", "--wat", "1"]);
    assert!(matches!(outcome, Outcome::Usage(_)), "{outcome:?}");

    // Not enough positionals.
    let outcome = run_simple(&["a.cm"]);
    assert!(matches!(outcome, Outcome::Usage(_)), "{outcome:?}");
    let outcome = run_simple(&[]);
    assert!(matches!(outcome, Outcome::Usage(_)), "{outcome:?}");
}

// ---------------------------------------------------------------------------
// Execution through the facade
// ---------------------------------------------------------------------------

#[test]
fn a_source_file_invokes_and_prints_cmon() {
    let dir = temp_dir("src_ok");
    let path = dir.join("app.cm");
    std::fs::write(
        &path,
        "int fib(int n) {\nif (n < 2) {\nreturn n\n}\nreturn fib(n - 1) + fib(n - 2)\n}\n",
    )
    .unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "fib", "10"]);
    assert_eq!(outcome, Outcome::Returned("55".into()));
    assert_eq!(outcome.exit_code(), exit_code::OK);
    assert!(outcome.to_stdout());
}

#[test]
fn string_arguments_flow_through() {
    let dir = temp_dir("str_arg");
    let path = dir.join("greet.cm");
    std::fs::write(&path, "str greet(str name) {\nreturn \"hi \" + name\n}\n").unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "greet", "\"checkmate\""]);
    assert_eq!(outcome, Outcome::Returned("hi checkmate".into()));
}

#[test]
fn void_results_print_nothing() {
    let dir = temp_dir("void_result");
    let path = dir.join("quiet.cm");
    std::fs::write(&path, "void quiet() {\n}\n").unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "quiet"]);
    assert_eq!(outcome, Outcome::Returned(String::new()));
    assert_eq!(outcome.exit_code(), exit_code::OK);
}

#[test]
fn composite_results_render_cmon() {
    let dir = temp_dir("composite");
    let path = dir.join("shapes.cm");
    std::fs::write(
        &path,
        concat!(
            "struct vec2 {\nfloat x\nfloat y\n}\n",
            "vec2 mk(float x, float y) {\nreturn vec2(x: x, y: y)\n}\n",
            "int[] pair(int a, int b) {\nreturn [a, b]\n}\n",
        ),
    )
    .unwrap();
    let base = path.to_str().unwrap().to_string();

    let outcome = run_simple(&[&base, "mk", "1.0", "2.5"]);
    assert_eq!(outcome, Outcome::Returned("vec2(x: 1, y: 2.5)".into()));

    let outcome = run_simple(&[&base, "pair", "3", "4"]);
    assert_eq!(outcome, Outcome::Returned("[3, 4]".into()));
}

#[test]
fn impl_member_entry_uses_target_dot_member() {
    let dir = temp_dir("impl_member");
    let path = dir.join("iface.cm");
    std::fs::write(
        &path,
        concat!(
            "impl engine.gamemode {\n",
            "int InitGame(int seed) {\nreturn seed * 2\n}\n",
            "}\n",
        ),
    )
    .unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "engine.gamemode.InitGame", "21"]);
    assert_eq!(outcome, Outcome::Returned("42".into()));
}

#[test]
fn a_mod_directory_loads_and_runs() {
    let dir = temp_dir("mod_ok");
    std::fs::write(
        dir.join("mod.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\ncheckmate_version = \"0.2.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/main.cm"), "int main() {\nreturn 7 * 6\n}\n").unwrap();

    let outcome = run_simple(&[dir.to_str().unwrap(), "main"]);
    assert_eq!(outcome, Outcome::Returned("42".into()));

    // The manifest path works too.
    let manifest = dir.join("mod.toml");
    let outcome = run_simple(&[manifest.to_str().unwrap(), "main"]);
    assert_eq!(outcome, Outcome::Returned("42".into()));
}

// ---------------------------------------------------------------------------
// Failure shapes
// ---------------------------------------------------------------------------

#[test]
fn compile_failures_exit_with_code_two() {
    let dir = temp_dir("compile_err");
    let path = dir.join("broken.cm");
    std::fs::write(&path, "int f() {\nreturn 1 + \"x\"\n}\n").unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "f"]);
    assert!(matches!(outcome, Outcome::Compile(_)), "{outcome:?}");
    assert_eq!(outcome.exit_code(), exit_code::COMPILE);
    assert!(outcome.message().contains("2:1"), "{}", outcome.message());
}

#[test]
fn execution_failures_exit_with_code_one() {
    let dir = temp_dir("runtime_err");
    let path = dir.join("boom.cm");
    std::fs::write(&path, "int f() {\nreturn 1 / 0\n}\n").unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "f"]);
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    assert_eq!(outcome.exit_code(), exit_code::EXECUTION);
    assert!(
        outcome.message().starts_with("line 2,"),
        "{}",
        outcome.message()
    );
}

#[test]
fn unknown_entries_exit_with_code_one() {
    let dir = temp_dir("entry_miss");
    let path = dir.join("small.cm");
    std::fs::write(&path, "int f() {\nreturn 1\n}\n").unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "ghost"]);
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    assert_eq!(outcome.exit_code(), exit_code::EXECUTION);
    assert_eq!(outcome.message(), "unknown function `ghost`");
}

#[test]
fn missing_files_exit_with_code_four() {
    let outcome = run_simple(&["/definitely/not/here.cm", "f"]);
    assert!(matches!(outcome, Outcome::Io(_)), "{outcome:?}");
    assert_eq!(outcome.exit_code(), exit_code::IO);
}

// ---------------------------------------------------------------------------
// Limits flags actually constrain
// ---------------------------------------------------------------------------

#[test]
fn the_fuel_flag_bounds_a_runaway_program() {
    let dir = temp_dir("fuel_flag");
    let path = dir.join("spin.cm");
    std::fs::write(&path, "int spin(int n) {\nreturn spin(n + 1)\n}\n").unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "spin", "0", "--fuel", "64"]);
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    assert_eq!(outcome.exit_code(), exit_code::EXECUTION);
    assert!(
        outcome.message().contains("fuel budget exhausted"),
        "{}",
        outcome.message()
    );
}

#[test]
fn the_depth_flag_bounds_recursion_tight() {
    let dir = temp_dir("depth_flag");
    let path = dir.join("spin.cm");
    std::fs::write(&path, "int spin(int n) {\nreturn spin(n + 1)\n}\n").unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "spin", "0", "--depth", "8"]);
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    assert!(
        outcome.message().contains("call depth limit of 8"),
        "{}",
        outcome.message()
    );
}

#[test]
fn the_deadline_flag_stops_an_infinite_loop() {
    let dir = temp_dir("deadline_flag");
    let path = dir.join("loop.cm");
    std::fs::write(&path, "int loop() {\nwhile (true) {\n}\nreturn 0\n}\n").unwrap();
    let outcome = run_simple(&[path.to_str().unwrap(), "loop", "--deadline-ms", "300"]);
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    assert!(
        outcome.message().contains("deadline"),
        "{}",
        outcome.message()
    );
}

#[test]
fn deadline_duration_conversion_is_the_obvious_one() {
    assert_eq!(
        cme_rust_host::deadline_from_ms(50),
        std::time::Duration::from_millis(50)
    );
}
