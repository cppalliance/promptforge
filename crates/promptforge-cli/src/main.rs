//! The `promptforge` command-line tool.
//!
//! `promptforge run <file.md> [input]` parses the prompt and executes its
//! sections top to bottom (fall-through). `input` is the single raw argument
//! string exposed to the prompt as `args`; it defaults to empty. The file must
//! be a promptforge prompt - its frontmatter must declare a `promptforge:`
//! version - or the CLI declines to run it.

use std::process::ExitCode;

use promptforge_core::CancelHandle;
use promptforge_core::bind::bind_prompt;
use promptforge_core::cancel;
use promptforge_core::execute::RunOptions;
use promptforge_core::model::{ModelCatalog, fetch_model_catalog};
use promptforge_core::observe::{NullObserver, Observer};
use promptforge_core::store::StoreRef;
use promptforge_core::{execute, parser::Prompt};
use promptforge_tool_picker::{Config as PickerConfig, ToolPicker};

mod tools;

/// Entry point. Dispatches subcommands and maps errors to a non-zero exit.
#[tokio::main]
async fn main() -> ExitCode {
    let cancel = CancelHandle::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancel.cancel();
        }
    });

    let mut args = std::env::args().skip(1);
    let command = args.next();
    cancel::scope(cancel, async {
        match command.as_deref() {
            Some("run") => {
                let Some(path) = args.next() else {
                    eprintln!("usage: promptforge run <file.md> [input]");
                    return ExitCode::FAILURE;
                };
                let input = args.next().unwrap_or_default();
                run(&path, &input, &NullObserver).await
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
    })
    .await
}

/// Parse the file, execute its sections with `input` as `args`, and print the
/// result.
async fn run(path: &str, input: &str, observer: &dyn Observer) -> ExitCode {
    let execution = format!("cli-{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..));
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

    let prompt = match Prompt::parse(&source, &execution, observer) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let key = std::env::var("PROMPTFORGE_GATEWAY_KEY").ok();
    let base_url = match std::env::var("PROMPTFORGE_GATEWAY_URL") {
        Ok(url) => url,
        Err(_) if key.is_some() => {
            eprintln!("error: missing environment variable PROMPTFORGE_GATEWAY_URL");
            return ExitCode::FAILURE;
        }
        Err(_) => String::new(),
    };
    let available = tools::available_tools(&base_url, key.as_deref());
    let picker = match ToolPicker::build(available.catalog().clone(), PickerConfig::default()) {
        Ok(picker) => picker,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let models = match &key {
        Some(key) => match fetch_model_catalog(&base_url, key).await {
            Ok(catalog) => catalog,
            Err(error) => {
                eprintln!("error: fetch model catalog: {error}");
                return ExitCode::FAILURE;
            }
        },
        None => ModelCatalog::empty(),
    };
    let registry = available.registry();
    let bound = match bind_prompt(prompt, &picker, &registry, &models, &execution, observer) {
        Ok(bound) => bound,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // One run-scoped store, created once and shared by every section. The CLI
    // uses the in-memory sandbox backend by default.
    let store = StoreRef::memory();

    // The CLI prints the run's result and nothing else, so it discards
    // progress; its gateway client comes from the environment, which is what
    // `client: None` selects.
    let options = RunOptions {
        execution: &execution,
        observer,
        client: None,
        debug: None,
    };

    match execute::run(&bound, input, available.tools(), &store, options).await {
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use promptforge_core::observe::{Observer, detail};

    use super::run;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, execution: &str, section: &str, detail: &str) {
            self.0
                .lock()
                .expect("the CLI recorder mutex must not be poisoned")
                .push((execution.to_owned(), section.to_owned(), detail.to_owned()));
        }
    }

    #[tokio::test]
    async fn run_reuses_one_generated_execution_id_for_parse_bind_and_execution() {
        let path = std::env::temp_dir().join(format!(
            "promptforge-cli-execution-{:016x}.md",
            fastrand::u64(..)
        ));
        std::fs::write(
            &path,
            "---\nname: lifecycle\ndescription: CLI lifecycle fixture\npromptforge: 1\n---\n\n\
             # Lifecycle\n\n## Run\n\n```lua\nreturn 'done'\n```\n",
        )
        .expect("write the CLI lifecycle fixture");
        let recorder = Recorder::default();

        let status = run(
            path.to_str().expect("the fixture path must be UTF-8"),
            "",
            &recorder,
        )
        .await;
        std::fs::remove_file(&path).expect("remove the CLI lifecycle fixture");

        assert_eq!(status, std::process::ExitCode::SUCCESS);
        let records = recorder
            .0
            .lock()
            .expect("the CLI recorder mutex must not be poisoned");
        let execution = records
            .first()
            .map(|(execution, _, _)| execution.as_str())
            .expect("the CLI run must emit observations");
        assert!(
            execution.starts_with("cli-") && execution.len() == 36,
            "the CLI must generate its documented execution id: {execution}"
        );
        assert!(
            records
                .iter()
                .all(|(record_execution, _, _)| record_execution == execution),
            "parse, bind, and execution must reuse one id: {records:#?}"
        );
        let details = records
            .iter()
            .map(|(_, _, detail)| detail.as_str())
            .collect::<Vec<_>>();
        for expected in [
            detail::PARSE_STARTED,
            detail::PARSE_SUCCEEDED,
            detail::TOOL_BINDING_STARTED,
            detail::TOOL_BINDING_SUCCEEDED,
            detail::RUN_STARTED,
            detail::RUN_SUCCEEDED,
        ] {
            assert!(
                details.contains(&expected),
                "the CLI lifecycle must include {expected:?}: {records:#?}"
            );
        }
    }
}
