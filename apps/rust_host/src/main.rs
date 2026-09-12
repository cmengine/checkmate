//! The thin binary shell: parse the command line, run, print, exit.

use std::process::ExitCode;

use cme_rust_host::{Command, parse_command, run, run_schema_demo};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse_command(&args) {
        Ok(command) => command,
        Err(outcome) => {
            eprintln!("{}", outcome.message());
            return ExitCode::from(outcome.exit_code() as u8);
        }
    };

    let outcome = match command {
        Command::SchemaDemo => run_schema_demo(),
        Command::Run(invocation) => run(&invocation),
    };
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
