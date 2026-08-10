//! The `promptforge` command-line tool.
//!
//! `promptforge run <file.md> [input]` parses the prompt and executes its
//! sections top to bottom (fall-through). `input` is the single raw argument
//! string exposed to the prompt as `args`; it defaults to empty. The file must
//! be a promptforge prompt - its frontmatter must declare a `promptforge:`
//! version - or the CLI declines to run it.

use std::process::ExitCode;
use std::sync::Arc;

use promptforge_core::CancelHandle;
use promptforge_core::client::GatewayClient;
use promptforge_core::execute::{ResolutionContext, RunConfig};
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
    match command.as_deref() {
        Some("run") => {
            let Some(path) = args.next() else {
                eprintln!("usage: promptforge run <file.md> [input]");
                return ExitCode::FAILURE;
            };
            let input = args.next().unwrap_or_default();
            let observer: Arc<dyn Observer> = Arc::new(NullObserver);
            run(&path, &input, observer, cancel).await
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
async fn run(
    path: &str,
    input: &str,
    observer: Arc<dyn Observer>,
    cancel: CancelHandle,
) -> ExitCode {
    let gateway_key = std::env::var("PROMPTFORGE_GATEWAY_KEY").ok();
    let gateway_url = std::env::var("PROMPTFORGE_GATEWAY_URL").ok();
    run_with_gateway(
        path,
        input,
        observer,
        cancel,
        Gateway::Environment {
            url: gateway_url.as_deref(),
            key: gateway_key.as_deref(),
        },
    )
    .await
}

enum Gateway<'a> {
    Environment {
        url: Option<&'a str>,
        key: Option<&'a str>,
    },
    #[cfg(test)]
    Disabled,
}

async fn run_with_gateway(
    path: &str,
    input: &str,
    observer: Arc<dyn Observer>,
    cancel: CancelHandle,
    gateway: Gateway<'_>,
) -> ExitCode {
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

    let prompt = match Prompt::parse(&source, &execution, observer.as_ref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    match gateway {
        Gateway::Environment { url, key } => {
            let base_url = match url {
                Some(url) => url,
                None if key.is_some() => {
                    eprintln!("error: missing environment variable PROMPTFORGE_GATEWAY_URL");
                    return ExitCode::FAILURE;
                }
                None => "",
            };
            let available = tools::available_tools(base_url, key);
            let picker =
                match ToolPicker::build(available.catalog().clone(), PickerConfig::default()) {
                    Ok(picker) => picker,
                    Err(e) => {
                        eprintln!("error: {e}");
                        return ExitCode::FAILURE;
                    }
                };
            let models = match key {
                Some(key) => match fetch_model_catalog(base_url, key).await {
                    Ok(catalog) => catalog,
                    Err(error) => {
                        eprintln!("error: fetch model catalog: {error}");
                        return ExitCode::FAILURE;
                    }
                },
                None => ModelCatalog::empty(),
            };
            execute_prompt(
                &prompt,
                input,
                observer,
                cancel,
                &execution,
                &picker,
                &models,
                available.tools(),
                None,
            )
            .await
        }
        #[cfg(test)]
        Gateway::Disabled => {
            let picker = match ToolPicker::build(
                promptforge_tool_picker::Catalog::default(),
                PickerConfig::default(),
            ) {
                Ok(picker) => picker,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            execute_prompt(
                &prompt,
                input,
                observer,
                cancel,
                &execution,
                &picker,
                &ModelCatalog::empty(),
                &[],
                Some(GatewayClient::disabled()),
            )
            .await
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the CLI passes one explicit execution environment to the core runner"
)]
async fn execute_prompt(
    prompt: &Prompt,
    input: &str,
    observer: Arc<dyn Observer>,
    cancel: CancelHandle,
    execution: &str,
    picker: &ToolPicker,
    models: &ModelCatalog,
    tools: &[Arc<dyn promptforge_core::tools::Tool>],
    client: Option<GatewayClient>,
) -> ExitCode {
    let store = StoreRef::memory();

    let mut config = RunConfig::new(execution).observer(observer).cancel(cancel);
    if let Some(client) = client {
        config = config.client(client);
    }

    match execute::run(
        prompt,
        input,
        ResolutionContext::new(picker, models),
        tools,
        &store,
        config,
    )
    .await
    {
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

    use super::{Gateway, run_with_gateway};

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
    async fn injected_no_gateway_run_is_hermetic_and_reuses_one_execution_id() {
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

        let status = run_with_gateway(
            path.to_str().expect("the fixture path must be UTF-8"),
            "",
            &recorder,
            Gateway::Disabled,
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
            "parse and execution must reuse one id: {records:#?}"
        );
        let details = records
            .iter()
            .map(|(_, _, detail)| detail.as_str())
            .collect::<Vec<_>>();
        for expected in [
            detail::PARSE_STARTED,
            detail::PARSE_SUCCEEDED,
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
