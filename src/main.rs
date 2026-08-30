#[cfg(feature = "cli")]
use cme_compiler::logos::Logos;
#[cfg(feature = "cli")]
use std::process::ExitCode;

#[cfg(feature = "cli")]
const USAGE: &str = "Usage: cme <lex|ast> <file.cm>";

#[cfg(feature = "cli")]
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [command, path] = args.as_slice() else {
        return Err(format!("expected exactly two arguments\n{USAGE}"));
    };

    let source =
        std::fs::read_to_string(path).map_err(|error| format!("failed to read {path}: {error}"))?;

    match command.as_str() {
        "lex" => {
            let tokens: Vec<_> = cme_compiler::lexer::Token::lexer(&source)
                .map(|token| token.map_err(|_| "lexing failed".to_string()))
                .collect::<Result<_, _>>()?;
            for token in tokens {
                println!("{token:?}");
            }
        }
        "ast" => {
            let tokens: Vec<_> = cme_compiler::lexer::Token::lexer(&source)
                .map(|token| token.map_err(|_| "lexing failed".to_string()))
                .collect::<Result<_, _>>()?;
            let tokens = cme_compiler::parser::Parser::strip_insignificant_newlines(tokens)?;
            let ast = cme_compiler::parser::Parser::new(&tokens).parse_program()?;

            println!("{ast:#?}");
        }
        _ => return Err(format!("unknown command: {command}\n{USAGE}")),
    }

    Ok(())
}

// This binary is only compiled if the user installs the CLI toolchain.
#[cfg(feature = "cli")]
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
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
