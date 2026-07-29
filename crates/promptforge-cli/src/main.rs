//! The `promptforge` command-line tool.
//!
//! Tranche 1 supports one command: `promptforge run <file.md>`. It parses the
//! prompt, executes the entry section against the model, and prints the reply.

use std::process::ExitCode;

use promptforge_core::{client::Client, execute, parser::Prompt};

/// Entry point. Dispatches subcommands and maps errors to a non-zero exit.
#[tokio::main]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    match command.as_deref() {
        Some("run") => {
            let Some(path) = args.next() else {
                eprintln!("usage: promptforge run <file.md>");
                return ExitCode::FAILURE;
            };
            run(&path).await
        }
        Some(other) => {
            eprintln!("unknown command: {other}\nusage: promptforge run <file.md>");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("usage: promptforge run <file.md>");
            ExitCode::FAILURE
        }
    }
}

/// Parse the file, execute its entry section, and print the model's reply.
async fn run(path: &str) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let prompt = match Prompt::parse(&source) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let client = match Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    match execute::run(&prompt, &client).await {
        Ok(reply) => {
            println!("{reply}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
