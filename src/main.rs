#[cfg(feature = "cli")]
use cme_compiler::diagnostics::Diagnostic;
#[cfg(feature = "cli")]
use std::process::ExitCode;
#[cfg(feature = "cli")]
const USAGE: &str = "Usage: cme <lex|ast|check> <file.cm>";

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
            let errors = errors.into_iter().map(Diagnostic::lex).collect::<Vec<_>>();
            for token in tokens {
                println!("{token:?}");
            }
            render_diagnostics(errors, &source)
        }
        "ast" => {
            let outcome = cme_compiler::parse_source(&source);
            let errors = outcome.diagnostics;
            let ast = outcome.statements;

            println!("{ast:#?}");
            render_diagnostics(errors, &source)
        }
        "check" => {
            let outcome = cme_compiler::parse_source(&source);
            let mut errors = outcome.diagnostics;
            errors.extend(cme_compiler::check::check(&outcome.statements));
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
    let span = error.span();
    let (line, column) = line_column(source, span.start);
    let line_text = source
        .split_inclusive(['\n'])
        .nth(line - 1)
        .unwrap_or_default()
        .trim_end_matches('\n')
        .to_string();
    let start_byte = line_start_byte(source, line);
    let leading = span.start.saturating_sub(start_byte);
    let prefix =
        String::from_utf8_lossy(&line_text.as_bytes()[..leading.min(line_text.len())]).len();
    // A span that crosses a line break renders only its first-line
    // portion; `...` marks that the span continues on a later line. A
    // span that ends with the line break itself is still single-line.
    let line_end = start_byte + line_text.len();
    let visible_end = span.end.min(line_end);
    let width = visible_end.saturating_sub(span.start).max(1);
    let mut caret = String::new();
    caret.push_str(&" ".repeat(prefix));
    caret.push_str(&"^".repeat(width));
    if span.end > line_end + 1 {
        caret.push_str("...");
    }
    format!(
        "{path}:{line}:{column}: {message}\n{line_text}\n{caret}",
        message = error.message()
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

#[cfg(all(test, feature = "cli"))]
mod tests {
    use super::render_error;
    use cme_compiler::diagnostics::Diagnostic;
    use cme_core::Span;

    fn rendered(source: &str, start: usize, end: usize) -> String {
        render_error(
            &Diagnostic::type_error("boom", Span::new(start, end)),
            source,
            "t.cm",
        )
    }

    fn caret_line(rendered: &str) -> &str {
        rendered.lines().nth(2).expect("message, line, caret")
    }

    #[test]
    fn single_line_span_renders_an_exact_caret_run() {
        let source = "infer x = 1\n";
        let rendered = rendered(source, 10, 11);
        assert!(rendered.starts_with("t.cm:1:11: boom\n"));
        assert_eq!(caret_line(&rendered), "          ^");
    }

    #[test]
    fn span_swallowing_the_line_break_is_still_single_line() {
        // The span covers "x = 1" plus the trailing newline and nothing
        // beyond it: no continuation marker, carets stop at line end.
        let source = "infer x = 1\n";
        let rendered = rendered(source, 6, 12);
        assert_eq!(caret_line(&rendered), "      ^^^^^");
    }

    #[test]
    fn multi_line_span_clamps_its_carets_to_the_first_line() {
        // The span covers `{`, a newline, a whole statement, and `}`;
        // only the `{` sits on line 1, so one caret plus `...`.
        let source = "int f() {\nint x = 1\n}\n";
        let rendered = rendered(source, 8, 21);
        assert!(rendered.starts_with("t.cm:1:9: boom\n"));
        assert_eq!(caret_line(&rendered), "        ^...");
    }

    #[test]
    fn missing_return_over_a_multi_line_body_renders_clamped() {
        // End to end: the body block span is multi-line, and the caret
        // stays on the first line instead of one long `^` run.
        let source = "int f() {\nint x = 1\n}\n";
        let outcome = cme_compiler::parse_source(source);
        let errors = cme_compiler::check::check(&outcome.statements);
        assert_eq!(errors.len(), 1);
        let rendered = render_error(&errors[0], source, "t.cm");
        assert!(rendered.starts_with("t.cm:1:9: missing return in non-void function `f`\n"));
        assert_eq!(caret_line(&rendered), "        ^...");
    }
}
