//! The thin binary shell: parse the command line, run, print, exit.

use std::process::ExitCode;

use cme_rust_host::{parse_arguments, run};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match parse_arguments(&args) {
        Ok(invocation) => invocation,
        Err(outcome) => {
            eprintln!("{}", outcome.message());
            return ExitCode::from(outcome.exit_code() as u8);
        }
    };

    let outcome = run(&invocation);
    if outcome.to_stdout() {
        let message = outcome.message();
        if !message.is_empty() {
            println!("{message}");
        }
    } else {
        eprintln!("{}", outcome.message());
    }
    ExitCode::from(outcome.exit_code() as u8)
}
