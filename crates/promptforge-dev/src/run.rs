//! Reusable run environment and single-shot prompt execution.
//!
//! [`RunEnv`] owns the values that stay valid across a whole invocation: the
//! model catalog, the live tool set, the semantic picker, and the gateway
//! client. It is built once (fetching the catalog once) and then lends those
//! values to [`RunEnv::run_prompt`] for each prompt run, so the watch loop does
//! not refetch the catalog or rebuild the picker on every save.
//!
//! Each run mirrors the CLI pipeline: read the file, require a `promptforge:`
//! version, parse, and execute against the gateway. Every observer record
//! streams to stderr; the returned result string is the caller's to print on
//! stdout. After every executed run, success or failure, the run's store is
//! reconciled to `<prompt-stem>.store` beside the prompt file (see
//! [`crate::dump`]). Blocking filesystem work runs off the async runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use promptforge_core::CancelHandle;
use promptforge_core::client::{GatewayClient, GatewayEndpoint, SecretString};
use promptforge_core::debug::DebugCapture;
use promptforge_core::execute::{ResolutionContext, RunConfig, run};
use promptforge_core::model::{ModelCatalog, fetch_model_catalog};
use promptforge_core::observe::Observer;
use promptforge_core::parser::Prompt;
use promptforge_core::store::StoreRef;
use promptforge_tool_picker::{Config as PickerConfig, ToolPicker};

use crate::config::GatewayEnv;
use crate::diagnostics::VerboseObserver;
use crate::dump::{self, SensitiveCapture};
use crate::tools::{self, AvailableTools};

/// Whether a run persists raw, sensitive turn traces.
///
/// Raw capture is off unless explicitly authorized at the process boundary,
/// so an ordinary run never silently persists prompts, tool arguments, or
/// model output to disk.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CapturePolicy {
    /// No raw capture; `.trace/` is never written.
    Off,
    /// Raw, unredacted capture explicitly authorized by the caller.
    RawSensitive(SensitiveCapture),
}

/// Everything reusable across the prompt runs of one invocation.
pub(crate) struct RunEnv {
    models: ModelCatalog,
    tools: AvailableTools,
    picker: ToolPicker,
    client: GatewayClient,
    capture: CapturePolicy,
}

impl std::fmt::Debug for RunEnv {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunEnv")
            .field("tools", &self.tools)
            .field("capture", &self.capture)
            .finish_non_exhaustive()
    }
}

impl RunEnv {
    /// Builds a run environment, fetching the model catalog once.
    ///
    /// # Errors
    /// Returns an error when the catalog cannot be fetched, the live tool set
    /// or picker cannot be built, or the gateway URL or key is unusable.
    pub(crate) async fn initialize(gateway: &GatewayEnv, capture: CapturePolicy) -> Result<RunEnv> {
        let models = fetch_model_catalog(&gateway.base_url, gateway.key.expose())
            .await
            .context("fetch model catalog")?;
        RunEnv::with_catalog(gateway, models, capture)
    }

    /// Builds a run environment from an explicit catalog, skipping the network
    /// fetch. Used by the watch loop's first init and by offline tests.
    ///
    /// # Errors
    /// Returns an error when the live tool set or picker cannot be built, or
    /// the gateway URL or key is unusable.
    pub(crate) fn with_catalog(
        gateway: &GatewayEnv,
        models: ModelCatalog,
        capture: CapturePolicy,
    ) -> Result<RunEnv> {
        let tools = tools::available_tools(&gateway.base_url, gateway.key.expose())
            .context("build the live tool set")?;
        let picker = ToolPicker::build(tools.catalog().clone(), PickerConfig::default())
            .context("build the live tool picker")?;
        let endpoint = GatewayEndpoint::new(&gateway.base_url)
            .with_context(|| format!("gateway URL {:?}", gateway.base_url))?;
        let key =
            SecretString::new(gateway.key.expose()).context("gateway key must not be empty")?;
        let client = GatewayClient::new(endpoint, key);
        Ok(RunEnv {
            models,
            tools,
            picker,
            client,
            capture,
        })
    }

    /// Runs one prompt file once and returns its final result string.
    ///
    /// # Errors
    /// Returns an error when the file cannot be read, declares no `promptforge:`
    /// version, fails to parse, or execution fails. On a successful run whose
    /// store dump fails, the dump error is returned; on a failed run, the
    /// execution error is primary and the dump failure is reported to stderr.
    pub(crate) async fn run_prompt(
        &self,
        prompt_path: &Path,
        input: &str,
        observer: Arc<dyn Observer>,
        cancel: CancelHandle,
    ) -> Result<String> {
        // Read the prompt off the runtime worker so a slow filesystem cannot
        // stall the executor.
        let source = tokio::fs::read_to_string(prompt_path)
            .await
            .with_context(|| format!("read {}", prompt_path.display()))?;
        if promptforge_core::promptforge_version(&source).is_none() {
            bail!(
                "{} is not a promptforge prompt: its frontmatter declares no `promptforge:` version. promptforge runs only promptforge prompts.",
                prompt_path.display()
            );
        }

        let execution = new_execution_id();
        // Banner before observer traffic so a fresh process is obvious in
        // scrollback even when an earlier run's lines are still on screen.
        eprintln!("run id: {execution}");
        let prompt = Prompt::parse(&source, &execution, observer.as_ref())
            .with_context(|| format!("parse {}", prompt_path.display()))?;

        // Clear the previous run's dump before starting so stale store files and
        // traces never masquerade as the current run. A failure to clear is an
        // error, not something to proceed past. Only `NotFound` satisfies the
        // invariant already.
        clear_previous_dump(prompt_path).await?;

        // The store stays in memory during execution: no filesystem write runs
        // on the async execution path. All disk work happens in the single
        // end-of-run reconcile below, which runs off the runtime.
        let store = StoreRef::memory();

        // Raw capture is installed only when explicitly authorized.
        let capture = match self.capture {
            CapturePolicy::Off => None,
            CapturePolicy::RawSensitive(authorization) => Some(Arc::new(dump::TraceCapture::new(
                prompt_path,
                authorization,
            ))),
        };

        let mut config = RunConfig::new(&execution)
            .observer(Arc::clone(&observer))
            .client(self.client.clone())
            .cancel(cancel);
        if let Some(capture) = &capture {
            let concrete = Arc::clone(capture);
            let debug: Arc<dyn DebugCapture> = concrete;
            config = config.debug(debug);
        }

        let result = run(
            &prompt,
            input,
            ResolutionContext::new(&self.picker, &self.models),
            self.tools.tools(),
            &store,
            config,
        )
        .await
        .with_context(|| format!("run {}", prompt_path.display()));

        // Flush the trace worker so every queued write lands before reconcile.
        // `finish` drains and joins the I/O worker, which can block, so it runs
        // off the async runtime.
        if let Some(capture) = &capture {
            let handle = Arc::clone(capture);
            if let Err(join_error) = tokio::task::spawn_blocking(move || handle.finish()).await {
                eprintln!("trace flush task failed: {join_error}");
            }
        }

        // Reconcile on success and failure alike: a failed run's partial store
        // is exactly what a debugging author needs. Runs off the runtime.
        let dump_result = reconcile_dump(store, prompt_path.to_path_buf()).await;
        combine(result, dump_result)
    }
}

/// Runs one prompt file once against a ready gateway.
///
/// Builds a [`RunEnv`] (fetching the catalog once) and runs the prompt.
///
/// # Errors
/// Returns an error when the environment cannot be built or the run fails.
pub(crate) async fn run_once(
    prompt_path: &Path,
    input: &str,
    gateway: &GatewayEnv,
    capture: CapturePolicy,
    cancel: CancelHandle,
) -> Result<String> {
    let env = RunEnv::initialize(gateway, capture).await?;
    let observer: Arc<dyn Observer> = Arc::new(VerboseObserver::new(std::io::stderr()));
    env.run_prompt(prompt_path, input, observer, cancel).await
}

/// Removes the previous run's dump directory off the runtime, treating any
/// failure other than `NotFound` as fatal.
async fn clear_previous_dump(prompt_path: &Path) -> Result<()> {
    let dump_dir = dump::dump_directory(prompt_path);
    tokio::task::spawn_blocking(move || -> Result<()> {
        match std::fs::remove_dir_all(&dump_dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(anyhow::Error::from(error))
                .with_context(|| format!("clear the previous dump {}", dump_dir.display())),
        }
    })
    .await
    .context("join the dump-clear task")?
}

/// Reconciles the run's store to disk off the runtime.
async fn reconcile_dump(store: StoreRef, prompt_path: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let mut status = std::io::stderr();
        dump::dump_store(&store, &prompt_path, &mut status)
    })
    .await
    .context("join the store-dump task")?
}

/// Combines the execution and dump outcomes per the module's error policy.
fn combine(run_result: Result<String>, dump_result: Result<()>) -> Result<String> {
    match (run_result, dump_result) {
        (Ok(output), Ok(())) => Ok(output),
        (Ok(_), Err(dump_error)) => {
            Err(dump_error.context("the run succeeded but its store dump failed"))
        }
        (Err(run_error), Ok(())) => Err(run_error),
        (Err(run_error), Err(dump_error)) => {
            // The execution error is primary; report the dump failure alongside
            // it rather than dropping it silently.
            eprintln!("store dump also failed: {dump_error:#}");
            Err(run_error)
        }
    }
}

/// Mints a fresh per-invocation execution id: `dev-` plus 128 random bits.
fn new_execution_id() -> String {
    format!("dev-{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use promptforge_core::CancelHandle;
    use promptforge_core::model::ModelCatalog;
    use promptforge_core::observe::{Observation, Observer};

    use crate::config::{GatewayEnv, GatewayKey};

    use super::{CapturePolicy, RunEnv, new_execution_id};

    #[derive(Debug, Default)]
    struct Recorder(Mutex<Vec<(String, String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, execution: &str, section: &str, event: Observation) {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push((
                execution.to_owned(),
                section.to_owned(),
                event.to_string(),
            ));
        }
    }

    fn fixture_gateway() -> GatewayEnv {
        GatewayEnv {
            base_url: "http://127.0.0.1:1/v1".to_owned(),
            key: GatewayKey::new("key"),
        }
    }

    fn offline_env() -> RunEnv {
        RunEnv::with_catalog(
            &fixture_gateway(),
            ModelCatalog::empty(),
            CapturePolicy::Off,
        )
        .expect("offline env builds")
    }

    #[test]
    fn successive_execution_ids_differ() {
        let first = new_execution_id();
        let second = new_execution_id();
        assert_ne!(first, second, "each mint must be unique");
        for id in [&first, &second] {
            let nonce = id.strip_prefix("dev-").expect("dev- prefix");
            assert_eq!(nonce.len(), 32);
            assert!(nonce.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[tokio::test]
    async fn run_reuses_one_generated_execution_id_for_parse_and_execution() {
        let directory = tempfile::tempdir().expect("create lifecycle fixture directory");
        let path = directory.path().join("lifecycle.md");
        std::fs::write(
            &path,
            "---\nname: lifecycle\ndescription: lifecycle fixture\npromptforge: 1\n---\n\n\
             # Lifecycle\n\n## Run\n\n```lua\nreturn 'done'\n```\n",
        )
        .expect("write the lifecycle fixture");
        let recorder = Arc::new(Recorder::default());
        let result = offline_env()
            .run_prompt(
                &path,
                "",
                Arc::clone(&recorder) as Arc<dyn Observer>,
                CancelHandle::new(),
            )
            .await
            .expect("the model-free lifecycle fixture must run offline");

        assert_eq!(result, "done");
        let records = recorder.0.lock().unwrap_or_else(PoisonError::into_inner);
        let execution = records
            .first()
            .map(|(execution, _, _)| execution.as_str())
            .expect("the run must emit observations");
        let nonce = execution
            .strip_prefix("dev-")
            .expect("the runner must generate its documented execution id prefix");
        assert!(
            nonce.len() == 32 && nonce.chars().all(|c| c.is_ascii_hexdigit()),
            "the execution id must carry a 32-hex-digit nonce: {execution}"
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
                "the lifecycle must include {expected:?}: {records:#?}"
            );
        }
    }

    async fn run_fixture(directory: &tempfile::TempDir, lua: &str) -> anyhow::Result<String> {
        let path = directory.path().join("fixture.md");
        std::fs::write(
            &path,
            format!(
                "---\nname: fixture\ndescription: dump fixture\npromptforge: 1\n---\n\n\
                 # Fixture\n\n## Run\n\n```lua\n{lua}\n```\n"
            ),
        )
        .expect("write the dump fixture");
        offline_env()
            .run_prompt(
                &path,
                "",
                Arc::new(Recorder::default()),
                CancelHandle::new(),
            )
            .await
    }

    #[tokio::test]
    async fn a_successful_run_dumps_the_store_beside_the_prompt() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        let result = run_fixture(
            &directory,
            "store.write('evidence.md', 'found\\nit\\n')\n\
             store.write('notes/deep.txt', 'nested')\n\
             return 'done'",
        )
        .await
        .expect("the model-free fixture must run offline");

        assert_eq!(result, "done");
        let dump_dir = directory.path().join("fixture.store");
        assert_eq!(
            std::fs::read_to_string(dump_dir.join("evidence.md")).expect("read dumped file"),
            "found\nit\n",
            "the dump must carry raw contents, trailing newline included"
        );
        assert_eq!(
            std::fs::read_to_string(dump_dir.join("notes").join("deep.txt"))
                .expect("read nested dumped file"),
            "nested"
        );
    }

    #[tokio::test]
    async fn a_failed_run_still_dumps_its_partial_store() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        let error = run_fixture(
            &directory,
            "store.write('partial.md', 'kept for debugging')\nerror('boom')",
        )
        .await
        .expect_err("the fixture raises after writing");

        assert!(
            format!("{error:#}").contains("boom"),
            "unexpected failure: {error:#}"
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("fixture.store").join("partial.md"))
                .expect("read the failed run's dump"),
            "kept for debugging",
            "a failed run's partial store is exactly what a debugging author needs"
        );
    }

    #[tokio::test]
    async fn a_rerun_with_an_empty_store_removes_the_previous_dump() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        run_fixture(&directory, "store.write('stale.txt', 'old')\nreturn 'one'")
            .await
            .expect("the first run must succeed");
        let dump_dir = directory.path().join("fixture.store");
        assert!(dump_dir.join("stale.txt").is_file());

        run_fixture(&directory, "return 'two'")
            .await
            .expect("the second run must succeed");

        assert!(
            !dump_dir.exists(),
            "an empty rerun must leave no dump directory"
        );
    }

    #[tokio::test]
    async fn refuses_a_file_that_declares_no_promptforge_version() {
        let directory = tempfile::tempdir().expect("create refusal fixture directory");
        let path = directory.path().join("plain.md");
        std::fs::write(&path, "---\nname: plain\n---\n\n# Plain\n").expect("write refusal fixture");

        let error = offline_env()
            .run_prompt(
                &path,
                "",
                Arc::new(Recorder::default()),
                CancelHandle::new(),
            )
            .await
            .expect_err("a non-promptforge file must be refused");

        assert!(
            format!("{error:#}").contains("is not a promptforge prompt"),
            "unexpected refusal error: {error:#}"
        );
    }

    #[tokio::test]
    async fn unreadable_path_reports_the_read_failure() {
        let directory = tempfile::tempdir().expect("create missing-path fixture directory");
        let path = directory.path().join("absent.md");

        let error = offline_env()
            .run_prompt(
                &path,
                "",
                Arc::new(Recorder::default()),
                CancelHandle::new(),
            )
            .await
            .expect_err("a missing prompt file must fail");

        let detail = format!("{error:#}");
        assert!(
            detail.contains("read") && detail.contains("absent.md"),
            "unexpected read error: {detail}"
        );
    }
}
