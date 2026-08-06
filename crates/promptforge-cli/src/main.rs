//! The `promptforge` command-line tool.
//!
//! `promptforge run <file.md> [input]` parses the prompt and executes its
//! sections top to bottom (fall-through). `input` is the single raw argument
//! string exposed to the prompt as `args`; it defaults to empty. The file must
//! be a promptforge prompt - its frontmatter must declare a `promptforge:`
//! version - or the CLI declines to run it.

use std::process::ExitCode;

use promptforge_core::execute::RunOptions;
use promptforge_core::observe::NullObserver;
use promptforge_core::store::Store;
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

/// Parse the file, execute its sections with `input` as `args`, and print the
/// result.
async fn run(path: &str, input: &str) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    if promptforge_core::promptforge_version(&source).is_none() {
        eprintln!(
            "error: {path} is not a promptforge prompt: its frontmatter declares no `promptforge:` version. promptforge runs only promptforge prompts."
        );
        return ExitCode::FAILURE;
    }

    let prompt = match Prompt::parse(&source, &NullObserver) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let base_url = std::env::var("PROMPTFORGE_BASE_URL").ok();
    let token = std::env::var("PROMPTFORGE_TOKEN").ok();
    let boxed = match tools::select_tools(&[], base_url.as_deref(), token.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tools: Vec<&dyn Tool> = boxed.iter().map(AsRef::as_ref).collect();

    // One run-scoped store, created once and shared by every section. The CLI
    // uses the in-memory sandbox backend by default.
    let store = Store::memory();

    // The CLI prints the run's result and nothing else, so it discards
    // progress; its gateway client comes from the environment, which is what
    // `client: None` selects.
    let options = RunOptions {
        observer: &NullObserver,
        client: None,
    };

    match execute::run(&prompt, input, &tools, &store, options).await {
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
