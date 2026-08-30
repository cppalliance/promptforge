//! Application orchestration for the `promptforge` CLI.
//!
//! `main` owns the process boundary (argument parsing, signal installation,
//! output, exit status). This module owns everything between: it validates the
//! gateway environment into a [`Gateway`], reads and parses the prompt, assembles
//! the tool set and model catalog, and runs the prompt, returning either the
//! rendered output or an [`anyhow::Error`] whose chain the boundary prints.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use promptforge_core::CancelHandle;
use promptforge_core::execute::{self, ResolutionContext, RunConfig};
use promptforge_core::model::{ModelCatalog, fetch_model_catalog};
use promptforge_core::observe::Observer;
use promptforge_core::parser::Prompt;
use promptforge_core::store::{FileStore, StoreRef};
use promptforge_tool_picker::{Config as PickerConfig, Model, ToolPicker};

use crate::progress::SetupProgress;
use crate::tools::{self, Gateway, Remote};

/// The `promptforge` command-line interface.
#[derive(Debug, Parser)]
#[command(name = "promptforge", version, about = "Run PromptForge prompts.")]
pub(crate) struct Cli {
    /// The subcommand to execute.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// The subcommands the CLI accepts.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Parse a prompt file and execute its sections top to bottom (fall-through).
    Run(RunArgs),
}

/// Arguments for `promptforge run`.
#[derive(Debug, clap::Args)]
pub(crate) struct RunArgs {
    /// Path to the promptforge prompt file.
    pub(crate) file: PathBuf,
    /// Raw input string exposed to the prompt as `args` (defaults to empty).
    pub(crate) input: Option<String>,
    /// Directory for persistent file-backed store. When absent, an ephemeral
    /// in-memory store is used.
    #[arg(long = "store", value_name = "DIR")]
    pub(crate) store_dir: Option<PathBuf>,
}

/// A single prompt run: what to run, and the run-scoped I/O and cancellation.
pub(crate) struct RunRequest<'a> {
    /// The prompt file to execute.
    pub(crate) file: &'a Path,
    /// The raw `args` input exposed to the prompt.
    pub(crate) input: &'a str,
    /// Optional directory for persistent file-backed store. `None` means
    /// ephemeral in-memory.
    pub(crate) store_dir: Option<&'a Path>,
    /// The observer that records the run lifecycle.
    pub(crate) observer: Arc<dyn Observer>,
    /// The cooperative cancellation handle wired to Ctrl-C.
    pub(crate) cancel: CancelHandle,
}

impl std::fmt::Debug for RunRequest<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunRequest")
            .field("file", &self.file)
            .field("input", &self.input)
            .finish_non_exhaustive()
    }
}

/// Runs one prompt using the gateway configuration read from the environment.
///
/// # Errors
/// Returns an error if the gateway environment is inconsistent, the file cannot
/// be read, it is not a promptforge prompt, parsing fails, tool or model setup
/// fails, or execution fails (including cooperative cancellation).
pub(crate) async fn run(request: RunRequest<'_>) -> Result<String> {
    let gateway = gateway_from_env()?;
    run_with_gateway(request, gateway).await
}

/// Reads the gateway environment variables and validates them into a [`Gateway`].
fn gateway_from_env() -> Result<Gateway> {
    let url = env_optional("PROMPTFORGE_GATEWAY_URL")?;
    let key = env_optional("PROMPTFORGE_GATEWAY_API_KEY")?;
    gateway_from_parts(url.as_deref(), key.as_deref())
}

/// Reads an optional environment variable, distinguishing an absent variable
/// (`Ok(None)`) from a present-but-unreadable one.
///
/// A missing variable is expected and yields `None`; a non-Unicode value is a
/// real error and is propagated with context rather than silently dropped.
fn env_optional(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error @ std::env::VarError::NotUnicode(_)) => {
            Err(error).with_context(|| format!("read environment variable {name}"))
        }
    }
}

/// Validates a raw URL/key pair into a [`Gateway`], rejecting the invalid state.
///
/// Empty and whitespace-only values are treated as absent. A token without a
/// usable endpoint is rejected here so it can never reach tool assembly and
/// silently downgrade to a local-only tool set. A valid pair is validated once
/// more by [`Remote::new`], the sole constructor of the remote state.
fn gateway_from_parts(url: Option<&str>, key: Option<&str>) -> Result<Gateway> {
    let token = key.map(str::trim).filter(|value| !value.is_empty());
    let endpoint = url.map(str::trim).filter(|value| !value.is_empty());
    match (token, endpoint) {
        (Some(token), Some(endpoint)) => Ok(Gateway::Remote(Remote::new(endpoint, token)?)),
        (Some(_), None) => bail!(
            "PROMPTFORGE_GATEWAY_API_KEY is set but PROMPTFORGE_GATEWAY_URL is missing or empty; both are required to reach the gateway"
        ),
        (None, _) => Ok(Gateway::LocalOnly),
    }
}

async fn run_with_gateway(request: RunRequest<'_>, gateway: Gateway) -> Result<String> {
    let RunRequest {
        file,
        input,
        store_dir,
        observer,
        cancel,
    } = request;
    let execution = format!("cli-{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..));

    let source = tokio::fs::read_to_string(file)
        .await
        .with_context(|| format!("read prompt file {}", file.display()))?;

    if promptforge_core::promptforge_version(&source).is_none() {
        bail!(
            "{} is not a promptforge prompt: its frontmatter declares no `promptforge:` version",
            file.display()
        );
    }

    let prompt = Prompt::parse(&source, &execution, observer.as_ref())
        .with_context(|| format!("parse prompt file {}", file.display()))?;

    let available = tools::available_tools(&gateway).context("assemble the CLI tool set")?;

    let progress = SetupProgress::new();
    let models = match &gateway {
        Gateway::Remote(remote) => fetch_model_catalog(remote.endpoint(), remote.token())
            .await
            .context("fetch the model catalog")?,
        Gateway::LocalOnly => ModelCatalog::empty(),
        #[cfg(test)]
        Gateway::Disabled => ModelCatalog::empty(),
    };
    progress.catalog.complete();

    let model = Model::load_with_progress(Some(&progress.model))
        .context("load the tool embedding model")?;
    let picker = build_picker(available.catalog().clone(), &model, &progress)?;
    // Setup is done: detach the tree and stop the renderer before the run's
    // own output begins.
    drop(progress);

    let store = match store_dir {
        Some(dir) => {
            let backend = FileStore::new(dir)
                .with_context(|| format!("create store directory {}", dir.display()))?;
            StoreRef::new(Box::new(backend))
        }
        None => StoreRef::memory(),
    };
    let config = RunConfig::new(execution.as_str())
        .observer(observer)
        .cancel(cancel);
    let config = with_test_client(config, &gateway);

    let output = execute::run(
        &prompt,
        input,
        ResolutionContext::new(&picker, &models, available.tools()),
        &store,
        config,
    )
    .await?;
    Ok(output)
}

/// Builds the semantic picker over `catalog` with an already loaded `model`,
/// reporting one tool-count step per embedded tool through `progress`.
fn build_picker(
    catalog: promptforge_tool_picker::Catalog,
    model: &Model,
    progress: &SetupProgress,
) -> Result<ToolPicker> {
    ToolPicker::build_with_model(
        model,
        catalog,
        PickerConfig::default(),
        Some(&progress.tools),
    )
    .context("build the tool picker")
}

/// Installs a disabled gateway client for the test-only [`Gateway::Disabled`]
/// seam so tests never touch the network; a no-op in normal builds.
#[cfg(test)]
fn with_test_client(config: RunConfig, gateway: &Gateway) -> RunConfig {
    if matches!(gateway, Gateway::Disabled) {
        config.client(promptforge_core::client::GatewayClient::disabled())
    } else {
        config
    }
}

#[cfg(not(test))]
fn with_test_client(config: RunConfig, _gateway: &Gateway) -> RunConfig {
    config
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use clap::Parser;
    use promptforge_core::CancelHandle;
    use promptforge_core::observe::{Observation, Observer};

    use super::{
        Cli, Command, Gateway, RunRequest, build_picker, gateway_from_parts, run_with_gateway,
    };
    use crate::progress::SetupProgress;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, execution: &str, section: &str, event: Observation) {
            self.0
                .lock()
                .expect("the CLI recorder mutex must not be poisoned")
                .push((execution.to_owned(), section.to_owned(), event.to_string()));
        }
    }

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("promptforge-cli-{:016x}.md", fastrand::u64(..)))
    }

    async fn run_disabled(file: &Path, observer: Arc<dyn Observer>) -> anyhow::Result<String> {
        run_with_gateway(
            RunRequest {
                file,
                input: "",
                store_dir: None,
                observer,
                cancel: CancelHandle::new(),
            },
            Gateway::Disabled,
        )
        .await
    }

    #[test]
    fn parser_accepts_run_with_optional_input() {
        let cli = Cli::parse_from(["promptforge", "run", "prompt.md"]);
        let Command::Run(args) = cli.command;
        assert_eq!(args.file, Path::new("prompt.md"));
        assert_eq!(args.input, None);
        assert_eq!(args.store_dir, None);

        let cli = Cli::parse_from(["promptforge", "run", "prompt.md", "hello world"]);
        let Command::Run(args) = cli.command;
        assert_eq!(args.input.as_deref(), Some("hello world"));
    }

    #[test]
    fn parser_accepts_store_flag() {
        let cli = Cli::parse_from([
            "promptforge",
            "run",
            "--store",
            "/tmp/my-store",
            "prompt.md",
        ]);
        let Command::Run(args) = cli.command;
        assert_eq!(args.store_dir, Some(PathBuf::from("/tmp/my-store")));

        let cli = Cli::parse_from(["promptforge", "run", "prompt.md"]);
        let Command::Run(args) = cli.command;
        assert_eq!(args.store_dir, None);
    }

    #[test]
    fn parser_rejects_missing_file_unknown_command_and_extra_arguments() {
        assert!(Cli::try_parse_from(["promptforge", "run"]).is_err());
        assert!(Cli::try_parse_from(["promptforge", "walk", "prompt.md"]).is_err());
        assert!(Cli::try_parse_from(["promptforge"]).is_err());
        assert!(
            Cli::try_parse_from(["promptforge", "run", "prompt.md", "input", "extra"]).is_err(),
            "clap must reject a trailing argument instead of silently dropping it",
        );
    }

    #[test]
    fn gateway_parsing_validates_url_and_key_combinations() {
        assert!(matches!(
            gateway_from_parts(None, None).expect("no credentials is local-only"),
            Gateway::LocalOnly
        ));
        assert!(matches!(
            gateway_from_parts(Some("http://gw/v1"), None).expect("url alone is local-only"),
            Gateway::LocalOnly
        ));
        assert!(matches!(
            gateway_from_parts(Some(""), Some("  ")).expect("blank key is local-only"),
            Gateway::LocalOnly
        ));

        match gateway_from_parts(Some("http://gw/v1"), Some("token"))
            .expect("url plus key is remote")
        {
            Gateway::Remote(remote) => {
                assert_eq!(remote.endpoint(), "http://gw/v1");
                assert_eq!(remote.token(), "token");
            }
            other => panic!("expected remote, got {other:?}"),
        }

        // The formerly silent downgrade: a key with no usable URL (absent or
        // blank) must be an explicit error, not a local-only fallback.
        assert!(gateway_from_parts(None, Some("token")).is_err());
        assert!(gateway_from_parts(Some("   "), Some("token")).is_err());
    }

    #[tokio::test]
    async fn missing_file_reports_a_read_context_error() {
        let error = run_disabled(&temp_path(), Arc::new(Recorder::default()))
            .await
            .expect_err("a missing prompt file must fail");
        assert!(
            format!("{error:?}").contains("read prompt file"),
            "error must carry read context: {error:?}",
        );
    }

    #[tokio::test]
    async fn non_promptforge_file_is_declined() {
        let path = temp_path();
        std::fs::write(&path, "# Not a prompt\n\njust markdown\n")
            .expect("write the non-prompt fixture");
        let error = run_disabled(&path, Arc::new(Recorder::default())).await;
        std::fs::remove_file(&path).expect("remove the non-prompt fixture");
        let error = error.expect_err("a non-promptforge file must be declined");
        assert!(
            format!("{error:?}").contains("not a promptforge prompt"),
            "error must explain the decline: {error:?}",
        );
    }

    #[tokio::test]
    async fn prompt_parse_error_is_reported_with_context() {
        // Valid `promptforge:` frontmatter (so the version gate passes) but a
        // body the parser rejects: no H1 and no sections.
        let path = temp_path();
        std::fs::write(
            &path,
            "---\nname: broken\ndescription: parse-error fixture\npromptforge: 1\n---\n\n\
             just prose with no heading and no sections\n",
        )
        .expect("write the parse-error fixture");
        let error = run_disabled(&path, Arc::new(Recorder::default())).await;
        std::fs::remove_file(&path).expect("remove the parse-error fixture");
        let error = error.expect_err("an unparsable prompt must fail");
        assert!(
            format!("{error:?}").contains("parse prompt file"),
            "error must carry parse context: {error:?}",
        );
    }

    #[tokio::test]
    async fn exit_code_maps_operational_failures_to_one() {
        // A plain non-cancellation error maps to the generic failure code.
        assert_eq!(crate::exit_code(&anyhow::anyhow!("boom")), 1);
        // A real operational failure from the runner maps the same way.
        let error = run_disabled(&temp_path(), Arc::new(Recorder::default()))
            .await
            .expect_err("a missing file must fail");
        assert_eq!(crate::exit_code(&error), 1);
    }

    #[tokio::test]
    async fn exit_code_maps_a_cancelled_run_to_one_hundred_thirty() {
        // A pre-cancelled handle: the section's Lua loop trips the instruction
        // hook, which observes the set cancel flag and aborts to a cancelled
        // RunError. This exercises the 130 branch end to end.
        let path = temp_path();
        std::fs::write(
            &path,
            "---\nname: loop\ndescription: cancellation fixture\npromptforge: 1\n---\n\n\
             # Loop\n\n## Spin\n\n```lua\nlocal n = 0\nfor i = 1, 5000000 do n = n + 1 end\nreturn 'done'\n```\n",
        )
        .expect("write the cancellation fixture");
        let cancel = CancelHandle::new();
        cancel.cancel();
        let result = run_with_gateway(
            RunRequest {
                file: &path,
                input: "",
                store_dir: None,
                observer: Arc::new(Recorder::default()),
                cancel,
            },
            Gateway::Disabled,
        )
        .await;
        std::fs::remove_file(&path).expect("remove the cancellation fixture");
        let error = result.expect_err("a cancelled run must fail");
        assert!(
            error.chain().any(|cause| {
                cause
                    .downcast_ref::<promptforge_core::execute::RunError>()
                    .is_some_and(promptforge_core::execute::RunError::is_cancelled)
            }),
            "the failure chain must include a cancelled RunError: {error:?}"
        );
        assert_eq!(crate::exit_code(&error), 130);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn non_utf8_path_is_carried_through_as_a_path() {
        use std::os::unix::ffi::OsStrExt;
        let mut name = std::env::temp_dir().into_os_string();
        name.push(std::ffi::OsStr::from_bytes(b"/promptforge-cli-\xff\xfe.md"));
        let path = std::path::PathBuf::from(name);
        // A non-UTF-8 path flows through `PathBuf`/`&Path` without panicking, and
        // the read failure names it via `Path::display`.
        let error = run_disabled(&path, Arc::new(Recorder::default()))
            .await
            .expect_err("a non-UTF-8, nonexistent path must fail to read");
        assert!(
            format!("{error:?}").contains("read prompt file"),
            "error must carry read context for a non-UTF-8 path: {error:?}",
        );
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
    fn setup_progress_reaches_completion_after_a_real_build() {
        let available =
            super::tools::available_tools(&Gateway::LocalOnly).expect("local tools build");
        let progress = SetupProgress::new();
        let model = promptforge_tool_picker::Model::load_with_progress(Some(&progress.model))
            .expect("the compiled-in model loads");
        build_picker(available.catalog().clone(), &model, &progress)
            .expect("the picker builds over the local catalog");
        progress.catalog.complete();

        assert_eq!(progress.model.fraction(), 1.0, "the model leaf must finish");
        assert_eq!(progress.tools.fraction(), 1.0, "the tools leaf must finish");
        assert_eq!(
            progress.fraction(),
            1.0,
            "every setup leaf must report completion"
        );
    }

    #[tokio::test]
    async fn hermetic_run_reuses_one_execution_id_across_the_lifecycle() {
        let path = temp_path();
        std::fs::write(
            &path,
            "---\nname: lifecycle\ndescription: CLI lifecycle fixture\npromptforge: 1\n---\n\n\
             # Lifecycle\n\n## Run\n\n```lua\nreturn 'done'\n```\n",
        )
        .expect("write the CLI lifecycle fixture");
        let recorder = Arc::new(Recorder::default());

        let output = run_disabled(&path, Arc::clone(&recorder) as Arc<dyn Observer>).await;
        std::fs::remove_file(&path).expect("remove the CLI lifecycle fixture");
        assert_eq!(output.expect("the hermetic run must succeed"), "done");

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
            .map(|(_, _, detail)| detail.clone())
            .collect::<Vec<_>>();
        for expected in [
            Observation::ParseStarted,
            Observation::ParseSucceeded,
            Observation::RunStarted,
            Observation::RunSucceeded,
        ] {
            assert!(
                details.contains(&expected.to_string()),
                "the CLI lifecycle must include {expected:?}: {records:#?}"
            );
        }
    }
}
