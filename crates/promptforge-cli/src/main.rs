//! The `promptforge` command-line tool.
//!
//! `promptforge run <file.md> [input]` parses the prompt and executes its entry
//! section. `input` is the single raw argument string exposed to the prompt as
//! `args`; it defaults to empty.

use std::process::ExitCode;

use promptforge_core::tools::Tool;
use promptforge_core::{execute, parser::Prompt};

mod tools;

/// Entry point. Dispatches subcommands and maps errors to a non-zero exit.
#[tokio::main]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    match command.as_deref() {
        Some("run") => {
            let Some(path) = args.next() else {
                eprintln!("usage: promptforge run <file.md> [input]");
                return ExitCode::FAILURE;
            };
            let input = args.next().unwrap_or_default();
            run(&path, &input).await
        }
        Some(other) => {
            eprintln!("unknown command: {other}\nusage: promptforge run <file.md> [input]");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("usage: promptforge run <file.md> [input]");
            ExitCode::FAILURE
        }
    }
}

/// Parse the file, execute its entry section with `input` as `args`, and print
/// the result.
async fn run(path: &str, input: &str) -> ExitCode {
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

    let base_url = std::env::var("PROMPTFORGE_BASE_URL").ok();
    let token = std::env::var("PROMPTFORGE_TOKEN").ok();
    let boxed = match tools::select_tools(
        &prompt.frontmatter.tools,
        base_url.as_deref(),
        token.as_deref(),
    ) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tools: Vec<&dyn Tool> = boxed.iter().map(AsRef::as_ref).collect();

    match execute::run(&prompt, input, &tools).await {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
