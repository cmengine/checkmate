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
//!   API defaults).
//!
//! Exit codes: `0` success, `1` execution failure, `2` compile failure,
//! `3` usage error, `4` IO failure. The binary prints the invocation
//! result in the language's canonical CMON rendering (§11.1) — the exact
//! text a script would print.

use std::path::Path;
use std::time::Duration;

use cme::{Engine, ExecutionLimits, Value};

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
}

/// Parses the argument list (without the program name). Anything wrong is
/// a [`Outcome::Usage`] error.
pub fn parse_arguments(args: &[String]) -> Result<Invocation, Outcome> {
    let mut positional: Vec<String> = Vec::new();
    let mut limits = ExecutionLimits::default();

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let mut option_value = |name: &str| -> Result<u64, Outcome> {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| Outcome::Usage(format!("{name} requires a non-negative integer")))?;
            value.parse().map_err(|_| {
                Outcome::Usage(format!(
                    "{name} requires a non-negative integer, got {value:?}"
                ))
            })
        };

        if arg == "--fuel" {
            // The CLI's 0 is "unset", matching the C ABI's convention.
            limits.fuel = match option_value("--fuel")? {
                0 => None,
                fuel => Some(fuel),
            };
        } else if arg == "--deadline-ms" {
            limits.deadline_ms = match option_value("--deadline-ms")? {
                0 => None,
                ms => Some(ms),
            };
        } else if arg == "--depth" {
            let depth = option_value("--depth")?;
            limits.max_call_depth = usize::try_from(depth).unwrap_or(usize::MAX);
            if limits.max_call_depth == 0 {
                limits.max_call_depth = cme::MAX_CALL_DEPTH;
            }
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
             options: --fuel N, --deadline-ms N, --depth N (0 = unset)"
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

    Ok(Invocation {
        path: path.clone(),
        entry: entry.clone(),
        args: script_args,
        limits,
    })
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
pub fn run(invocation: &Invocation) -> Outcome {
    let engine = Engine::new();
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
