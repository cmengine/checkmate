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
    Compiler(Vec<Diagnostic>, String),
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
            let (tokens, errors) = cme_compiler::lexer::lex_with_errors(&source);
            let errors = errors.into_iter().map(Diagnostic::Lex).collect::<Vec<_>>();
            for token in tokens {
                println!("{token:?}");
            }
            render_diagnostics(errors, &source)
        }
        "ast" => {
            let (tokens, lex_errors) = cme_compiler::lexer::lex_with_errors(&source);
            let mut errors = lex_errors
                .into_iter()
                .map(Diagnostic::Lex)
                .collect::<Vec<_>>();
            let (tokens, strip_errors) =
                Parser::strip_insignificant_newlines_with_errors(tokens, source.len());
            errors.extend(strip_errors);
            let (ast, parse_errors) =
                Parser::new(&tokens, source.len()).parse_program_with_errors();
            errors.extend(parse_errors);

            println!("{ast:#?}");
            render_diagnostics(errors, &source)
        }
        _ => Err(CliError::Usage(format!(
            "unknown command: {command}\n{USAGE}"
        ))),
    }
}

#[cfg(feature = "cli")]
fn render_diagnostics(errors: Vec<Diagnostic>, source: &str) -> Result<(), CliError> {
    if !errors.is_empty() {
        return Err(CliError::Compiler(errors, source.to_string()));
    }
    Ok(())
}

#[cfg(feature = "cli")]
fn render_error(error: &Diagnostic, source: &str, path: &str) -> String {
    let span = match error {
        Diagnostic::Lex(error) => error.span(),
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
        Err(CliError::Compiler(errors, source)) => {
            for error in errors {
                eprintln!("error: {}", render_error(&error, &source, &source_path));
            }
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
