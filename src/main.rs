#[cfg(feature = "cli")]
use cme_compiler::lexer::LexError;
#[cfg(feature = "cli")]
use cme_compiler::parser::{Diagnostic, Parser};
#[cfg(feature = "cli")]
use std::process::ExitCode;
#[cfg(feature = "cli")]
#[cfg(feature = "cli")]
const USAGE: &str = "Usage: cme <lex|ast> <file.cm>";

#[cfg(feature = "cli")]
enum CliError {
    Usage(String),
    Io(String),
    Compiler(Diagnostic, String),
}

#[cfg(feature = "cli")]
fn run() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err(CliError::Usage(format!(
            "expected exactly two arguments\n{USAGE}"
        )));
    }
    let (command, path) = (&args[0], &args[1]);

    let source = std::fs::read_to_string(path)
        .map_err(|error| CliError::Io(format!("failed to read {path}: {error}")))?;

    match command.as_str() {
        "lex" => {
            let tokens = cme_compiler::lexer::lex(&source)
                .map_err(|error| CliError::Compiler(Diagnostic::Lex(error), source.clone()))?;
            for token in tokens {
                println!("{token:?}");
            }
        }
        "ast" => {
            let tokens = cme_compiler::lexer::lex(&source)
                .map_err(|error| CliError::Compiler(Diagnostic::Lex(error), source.clone()))?;
            let tokens = Parser::strip_insignificant_newlines(tokens, source.len())
                .map_err(|error| CliError::Compiler(error, source.clone()))?;
            let ast = Parser::new(&tokens, source.len())
                .parse_program()
                .map_err(|error| CliError::Compiler(error, source.clone()))?;

            println!("{ast:#?}");
        }
        _ => {
            return Err(CliError::Usage(format!(
                "unknown command: {command}\n{USAGE}"
            )));
        }
    }

    Ok(())
}

#[cfg(feature = "cli")]
fn render_error(error: &Diagnostic, source: &str, path: &str) -> String {
    let span = match error {
        Diagnostic::Lex(LexError::Invalid { span }) => *span,
        Diagnostic::Parse { span, .. } => *span,
    };
    let (line, column) = line_column(source, span.start);
    let line_text = source
        .split_inclusive(['\n'])
        .nth(line - 1)
        .unwrap_or_default()
        .trim_end_matches('\n')
        .to_string();
    let mut caret = String::new();
    let start_byte = line_start_byte(source, line);
    let leading = span.start.saturating_sub(start_byte);
    let width = span.end.saturating_sub(span.start).max(1);
    let prefix =
        String::from_utf8_lossy(&line_text.as_bytes()[..leading.min(line_text.len())]).len();
    caret.push_str(&" ".repeat(prefix));
    caret.push_str(&"^".repeat(width));
    format!(
        "{path}:{line}:{column}: {message}\n{line_text}\n{caret}",
        message = match error {
            Diagnostic::Lex(_) => "invalid token".to_string(),
            Diagnostic::Parse { message, .. } => message.clone(),
        }
    )
}

#[cfg(feature = "cli")]
fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut line_start = 0usize;
    for (index, byte) in source.bytes().enumerate() {
        if index >= offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
            line_start = index + 1;
        }
    }
    (line, offset - line_start + 1)
}

#[cfg(feature = "cli")]
fn line_start_byte(source: &str, line: usize) -> usize {
    let mut current = 1usize;
    for (index, byte) in source.bytes().enumerate() {
        if current == line {
            return index;
        }
        if byte == b'\n' {
            current += 1;
        }
    }
    source.len()
}

// This binary is only compiled if the user installs the CLI toolchain.
#[cfg(feature = "cli")]
fn main() -> ExitCode {
    let source_path = std::env::args().nth(2).unwrap_or_default();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(message) | CliError::Io(message)) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
        Err(CliError::Compiler(error, source)) => {
            eprintln!("error: {}", render_error(&error, &source, &source_path));
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "cli"))]
fn main() {
    eprintln!("Error: The 'cme' CLI was built without the 'cli' feature.");
    eprintln!("Try compiling with: cargo build --features cli");
    std::process::exit(1);
}
