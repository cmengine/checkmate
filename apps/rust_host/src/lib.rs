//! The Rust host application for Checkmate — the §13.1 consumer behind a
//! testable surface.
//!
//! `cme-rust-host <file.cm | mod_dir> <entry> [args...] [options]`
//!
//! - The first argument names a `.cm` file, a mod directory, or a
//!   `mod.toml` — the host picks the shape, the API compiles it.
//! - `<entry>` is the function (or `"target.member"` for a §10.4 impl
//!   member) the host chooses to invoke — Checkmate has no implicit entry
//!   point (§2.1).
//! - Arguments are host literals: `42`, `-3`, `2.5`, `true`, `false`,
//!   `"text"`.
//! - Options: `--fuel N`, `--deadline-ms N`, `--depth N` (0 = unset, the
//!   API defaults), and `--schema <file.cm>` (repeatable, §9.2: the host
//!   registers its contract so loads are schema-gated and capability
//!   calls dispatch to providers).
//! - `cme-rust-host --schema-demo` runs the full §9.6 generated-bindings
//!   flow over `schemas/game.cm`: a compile-time-verified provider, a
//!   schema-gated program, and typed proxy calls into the script.
//!
//! Exit codes: `0` success, `1` execution failure, `2` compile failure,
//! `3` usage error, `4` IO failure. The binary prints the invocation
//! result in the language's canonical CMON rendering (§11.1) — the exact
//! text a script would print.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cme::{Engine, ExecutionLimits, Value};

pub mod bindings {
    // WHITEPAPER §9.6, Rust half: the macro runs the REAL schema front end
    // over `schemas/game.cm` at THIS crate's compile time and generates the
    // capability trait, the interface proxies, the boundary types, and the
    // runtime descriptor. The generated code references the facade's `api`
    // re-export, so the host consumes everything through `cme`.
    cme::cme_schema_bindings!(path = "schemas/game.cm", crate = ::cme::api);
}

/// The process exit codes, kept testable as plain integers.
pub mod exit_code {
    pub const OK: i32 = 0;
    pub const EXECUTION: i32 = 1;
    pub const COMPILE: i32 = 2;
    pub const USAGE: i32 = 3;
    pub const IO: i32 = 4;
}

/// What the host did and what came of it: the binary maps this to stdout,
/// stderr, and a process code; the tests assert on it directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The invocation returned a CMON-rendered value ("" for void).
    Returned(String),
    /// The invocation failed (rendered [`cme::api::ExecutionError`]).
    Failed(String),
    /// The program did not compile (rendered [`cme::api::CompileError`]).
    Compile(String),
    /// The program could not be read.
    Io(String),
    /// The command line was wrong.
    Usage(String),
}

impl Outcome {
    /// The process exit code the outcome maps to.
    pub fn exit_code(&self) -> i32 {
        match self {
            Outcome::Returned(_) => exit_code::OK,
            Outcome::Failed(_) => exit_code::EXECUTION,
            Outcome::Compile(_) => exit_code::COMPILE,
            Outcome::Io(_) => exit_code::IO,
            Outcome::Usage(_) => exit_code::USAGE,
        }
    }

    /// Whether the outcome prints to stdout (success) or stderr (everything).
    pub fn to_stdout(&self) -> bool {
        matches!(self, Outcome::Returned(_))
    }

    /// The text to print.
    pub fn message(&self) -> &str {
        match self {
            Outcome::Returned(text)
            | Outcome::Failed(text)
            | Outcome::Compile(text)
            | Outcome::Io(text)
            | Outcome::Usage(text) => text,
        }
    }
}

/// One parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub path: String,
    pub entry: String,
    pub args: Vec<Value>,
    pub limits: ExecutionLimits,
    /// The §9.2 schema files the host registers before loading — the
    /// contract a load is checked against and capability calls dispatch
    /// through. Empty keeps the pre-schema behavior.
    pub schemas: Vec<String>,
}

/// A top-level command. `--schema-demo` needs no program of its own; the
/// default runs the caller's invocation.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Run(Invocation),
    /// The full §9.6 generated-bindings walkthrough over `schemas/game.cm`.
    SchemaDemo,
}

/// Parses the argument list (without the program name). Anything wrong is
/// a [`Outcome::Usage`] error.
pub fn parse_arguments(args: &[String]) -> Result<Invocation, Outcome> {
    match parse_command(args)? {
        Command::Run(invocation) => Ok(invocation),
        Command::SchemaDemo => Err(Outcome::Usage(
            "--schema-demo takes no further arguments".to_string(),
        )),
    }
}

/// Parses the full command surface, including `--schema-demo`.
pub fn parse_command(args: &[String]) -> Result<Command, Outcome> {
    if args.first().map(String::as_str) == Some("--schema-demo") {
        if args.len() != 1 {
            return Err(Outcome::Usage(
                "--schema-demo takes no further arguments".to_string(),
            ));
        }
        return Ok(Command::SchemaDemo);
    }

    let mut positional: Vec<String> = Vec::new();
    let mut limits = ExecutionLimits::default();
    let mut schemas: Vec<String> = Vec::new();

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let mut option_value = |name: &str| -> Result<String, Outcome> {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| Outcome::Usage(format!("{name} requires a value")))
        };

        if arg == "--fuel" {
            // The CLI's 0 is "unset", matching the C ABI's convention.
            limits.fuel = match option_value("--fuel")?.parse::<u64>() {
                Ok(0) => None,
                Ok(fuel) => Some(fuel),
                Err(_) => {
                    return Err(Outcome::Usage(
                        "--fuel requires a non-negative integer".to_string(),
                    ));
                }
            };
        } else if arg == "--deadline-ms" {
            limits.deadline_ms = match option_value("--deadline-ms")?.parse::<u64>() {
                Ok(0) => None,
                Ok(ms) => Some(ms),
                Err(_) => {
                    return Err(Outcome::Usage(
                        "--deadline-ms requires a non-negative integer".to_string(),
                    ));
                }
            };
        } else if arg == "--depth" {
            let depth = option_value("--depth")?.parse::<usize>().map_err(|_| {
                Outcome::Usage("--depth requires a non-negative integer".to_string())
            })?;
            limits.max_call_depth = if depth == 0 {
                cme::MAX_CALL_DEPTH
            } else {
                depth
            };
        } else if arg == "--schema" {
            schemas.push(option_value("--schema")?);
        } else if arg.starts_with("--") {
            return Err(Outcome::Usage(format!("unknown option {arg:?}")));
        } else {
            positional.push(arg.clone());
        }
        index += 1;
    }

    let [path, entry, script_args_slice @ ..] = positional.as_slice() else {
        return Err(Outcome::Usage(
            "usage: cme-rust-host <file.cm | mod_dir | mod.toml> <entry> [args...]\n\
             entry is a function name, or `target.member` for an impl member\n\
             options: --fuel N, --deadline-ms N, --depth N (0 = unset), --schema <file.cm>\n\
             or: cme-rust-host --schema-demo"
                .to_string(),
        ));
    };

    let mut script_args = Vec::new();
    for literal in script_args_slice {
        match parse_literal(literal) {
            Some(value) => script_args.push(value),
            None => {
                return Err(Outcome::Usage(format!(
                    "argument {literal:?} is not a Checkmate literal (int, float, bool, or \"str\")"
                )));
            }
        }
    }

    Ok(Command::Run(Invocation {
        path: path.clone(),
        entry: entry.clone(),
        args: script_args,
        limits,
        schemas,
    }))
}

/// A Checkmate scalar literal as the CLI accepts it.
fn parse_literal(text: &str) -> Option<Value> {
    if let Ok(int) = text.parse::<i64>() {
        return Some(Value::Int(int));
    }
    if let Ok(float) = text.parse::<f64>() {
        // Whole-number tokens already parsed as ints above; reject tokens
        // like `inf`/`nan` that are not literals of the language.
        if float.is_finite() && text.contains(['.', 'e', 'E']) {
            return Some(Value::Float(float));
        }
    }
    match text {
        "true" => return Some(Value::Bool(true)),
        "false" => return Some(Value::Bool(false)),
        _ => {}
    }
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        return Some(Value::Str(text[1..text.len() - 1].to_string()));
    }
    None
}

/// Runs one invocation end to end: load (file, mod, or source text),
/// create the context under the parsed limits, invoke the chosen entry.
/// Schema files from `--schema` register first, so loads are gated by the
/// §9 contract.
pub fn run(invocation: &Invocation) -> Outcome {
    let mut engine = Engine::new();
    for schema_path in &invocation.schemas {
        if let Err(error) = engine.load_schema_file(schema_path) {
            return Outcome::Io(error.message());
        }
    }
    let path = Path::new(&invocation.path);

    // A mod is a directory (or its mod.toml); everything else loads as a
    // single source file.
    let is_mod = path.is_dir()
        || (path.is_file() && path.file_name().is_some_and(|name| name == "mod.toml"));
    let program = if is_mod {
        engine.load_mod(path)
    } else {
        match std::fs::read_to_string(path) {
            Ok(source) => engine.load_source(&source),
            Err(error) => {
                // A missing file could still be a mod directory typo; the
                // API distinguishes IO from compile either way.
                return Outcome::Io(format!("cannot read {}: {error}", path.display()));
            }
        }
    };

    let program = match program {
        Ok(program) => program,
        Err(error) => return Outcome::Compile(error.message()),
    };

    let context = engine.create_context(&program, invocation.limits.clone());

    // `target.member` enters the impl registry (§10.4); a bare name calls
    // a top-level function (§2.1). The split is on the LAST dot, so the
    // target keeps its full dotted path (`engine.gamemode.InitGame`).
    let result = match invocation.entry.rsplit_once('.') {
        Some((target, member)) if !member.contains('.') && !target.is_empty() => {
            context.invoke_member(target, member, &invocation.args)
        }
        _ => context.invoke(&invocation.entry, &invocation.args),
    };

    match result {
        Ok(value) => Outcome::Returned(render(&value)),
        Err(error) => Outcome::Failed(error.render()),
    }
}

/// The canonical CMON rendering; void prints as nothing (the CLI
/// convention: void results print nothing at all).
fn render(value: &Value) -> String {
    if value.is_void() {
        String::new()
    } else {
        value.to_string()
    }
}

/// Deadline computation shared with the binary's clock; exposed for tests.
pub fn deadline_from_ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

// ---------------------------------------------------------------------------
// The §9.6 generated-bindings flow (WHITEPAPER §9, §13.1) — `--schema-demo`
// ---------------------------------------------------------------------------

/// The host side of capability `game.window` (§9.1): implementing the
/// GENERATED trait is the compile-time verification — a missing member, a
/// wrong parameter type, or a wrong return type breaks THIS crate's build,
/// never a runtime call.
pub struct WindowService {
    pub next_id: std::sync::atomic::AtomicI64,
    pub drawn: std::sync::atomic::AtomicI64,
}

impl Default for WindowService {
    fn default() -> Self {
        WindowService {
            next_id: std::sync::atomic::AtomicI64::new(100),
            drawn: std::sync::atomic::AtomicI64::new(0),
        }
    }
}

impl bindings::game::GameWindowCapability for WindowService {
    fn open_window(&self, title: String) -> bindings::game::Sprite {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        bindings::game::Sprite {
            id,
            name: title,
            scale: 1.0,
        }
    }

    fn draw(&self, sprite: bindings::game::Sprite) {
        // The typed argument arrives unpacked; the host reads fields freely.
        self.drawn
            .store(sprite.id, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The demo program: imports the capability, calls it, and implements the
/// `game.gamemode` interface the host calls back into through the proxy.
const DEMO_PROGRAM: &str = "import game.window\n\
     impl game.gamemode {\n\
     \x20int OnEvent(Event event) {\n\
     \x20return match (event) {\n\
     \x20\x20Started() => 0\n\
     \x20\x20Scored(int points) => points * 2\n\
     \x20}\n\
     \x20}\n\
     \x20int Tick(int frame) {\n\
     \x20return frame + 1\n\
     \x20}\n\
     }\n\
     int main() {\n\
     \x20Sprite sprite = game.window.OpenWindow(\"demo\")\n\
     \x20game.window.Draw(sprite)\n\
     \x20return sprite.id\n\
     }\n";

/// The whole schema flow, end to end, printed as the CLI's output:
///
/// 1. the generated descriptor registers the schema (§9.2);
/// 2. the generated `register_game_window` wires the compile-checked
///    provider to the engine (§9.1);
/// 3. a program that imports the capability and implements the interface
///    loads through the schema-gated pipeline;
/// 4. `Context::get_interface` builds the typed proxy (§13.1) and the host
///    calls INTO the script with schema types (the `Event` enum with its
///    payload crosses the boundary packed, and unpacks typed);
/// 5. a plain invocation runs `main`, dispatching capability calls to the
///    provider.
///
/// Every step renders one `key: value` line; a failure short-circuits as
/// the matching [`Outcome`].
pub fn run_schema_demo() -> Outcome {
    let mut engine = Engine::new();

    if let Err(error) = bindings::game::register_schema(&mut engine) {
        return Outcome::Compile(error.message());
    }
    if let Err(error) =
        bindings::game::register_game_window(&mut engine, Arc::new(WindowService::default()))
    {
        return Outcome::Usage(error);
    }

    let program = match engine.load_source(DEMO_PROGRAM) {
        Ok(program) => program,
        Err(error) => return Outcome::Compile(error.message()),
    };
    let context = engine.create_context(&program, ExecutionLimits::default());

    // The §13.1 shape, generic constructor and all.
    let gamemode: bindings::game::GameGamemodeProxy = match context.get_interface() {
        Ok(proxy) => proxy,
        Err(error) => return Outcome::Failed(error.render()),
    };

    let started = match gamemode.on_event(bindings::game::Event::Started) {
        Ok(points) => points,
        Err(error) => return Outcome::Failed(error.render()),
    };
    let scored = match gamemode.on_event(bindings::game::Event::Scored { points: 21 }) {
        Ok(points) => points,
        Err(error) => return Outcome::Failed(error.render()),
    };
    let tick = match gamemode.tick(41) {
        Ok(frame) => frame,
        Err(error) => return Outcome::Failed(error.render()),
    };

    let main = match context.invoke("main", &[]) {
        Ok(value) => value,
        Err(error) => return Outcome::Failed(error.render()),
    };

    Outcome::Returned(format!(
        "schema: game {} (namespace {})\n\
         proxy: OnEvent(Started) = {started}\n\
         proxy: OnEvent(Scored(21)) = {scored}\n\
         proxy: Tick(41) = {tick}\n\
         main: {}",
        bindings::game::SCHEMA_VERSION,
        bindings::game::NAMESPACE,
        render(&main),
    ))
}
