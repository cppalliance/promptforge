//! Single-shot prompt execution against an already-running gateway.
//!
//! [`run_once`] mirrors the CLI pipeline: require gateway credentials, read
//! the file, require a `promptforge:` version, parse, fetch the live model
//! catalog, build the live tool registry, and execute with a
//! [`GatewayClient`]. Every `(execution, section, detail)` observer record
//! streams to stderr; the returned result string is the caller's to print on
//! stdout. After every executed run, success or failure, the run's store is
//! dumped beside the prompt file (see [`crate::dump`]).

use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use anyhow::{Context as _, Result, bail};
use promptforge_core::client::GatewayClient;
use promptforge_core::execute::{ResolutionContext, RunOptions, run};
use promptforge_core::model::{ModelCatalog, fetch_model_catalog};
use promptforge_core::observe::Observer;
use promptforge_core::parser::Prompt;
use promptforge_core::store::StoreRef;
use promptforge_tool_picker::{Config as PickerConfig, ToolPicker};

use crate::dump;
use crate::tools;

/// Gateway URL and bearer required for every run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayEnv {
    /// The gateway API root (`PROMPTFORGE_GATEWAY_URL`).
    pub(crate) base_url: String,
    /// The bearer credential (`PROMPTFORGE_GATEWAY_KEY`).
    pub(crate) key: String,
}

/// Reads and validates `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY`
/// from the process environment.
///
/// # Errors
///
/// Returns a friendly error naming each missing variable when either is unset
/// or empty. This is intended to fail before any prompt parse.
pub(crate) fn require_gateway_env() -> Result<GatewayEnv> {
    require_gateway_env_from(|name| std::env::var(name).ok())
}

/// Formats a failed run so the first line leads with `prompt.md:LINE:` when
/// core mapped a Lua error to an absolute prompt line.
pub(crate) fn format_dev_failure(prompt_path: &Path, error: &anyhow::Error) -> String {
    let detail = format!("{error:#}");
    if let Some(line) = first_mapped_prompt_line(&detail) {
        format!(
            "dev run failed: {}:{}: {detail}",
            prompt_path.display(),
            line
        )
    } else {
        format!("dev run failed: {detail}")
    }
}

/// Pulls the innermost absolute prompt line from a core-mapped Lua error.
///
/// Core prefixes failures as `section \`Name\` epilog:51: ...`. Prefer the
/// last such tag so a fanout parent wrapper does not hide the arm that failed.
fn first_mapped_prompt_line(message: &str) -> Option<u32> {
    let mut found = None;
    let mut rest = message;
    while let Some(idx) = rest.find(':') {
        let after = &rest[idx + 1..];
        let digit_end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if digit_end > 0
            && after.as_bytes().get(digit_end) == Some(&b':')
            && let Ok(line) = after[..digit_end].parse::<u32>()
        {
            // Ignore tiny lines that are just HTTP status noise; prompt lines
            // for real files are almost never in the URL/port range alone, but
            // the surrounding tag (`epilog` / `prologue` / `library`) is the
            // real filter.
            let before = &rest[..idx];
            if before.ends_with("epilog")
                || before.ends_with("prologue")
                || before.ends_with("library")
            {
                found = Some(line);
            }
        }
        rest = &rest[idx + 1..];
    }
    found
}

/// [`require_gateway_env`] with an injected variable lookup for offline tests.
pub(crate) fn require_gateway_env_from(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<GatewayEnv> {
    let base_url = lookup("PROMPTFORGE_GATEWAY_URL").filter(|value| !value.is_empty());
    let key = lookup("PROMPTFORGE_GATEWAY_KEY").filter(|value| !value.is_empty());
    match (base_url, key) {
        (Some(base_url), Some(key)) => Ok(GatewayEnv { base_url, key }),
        (None, None) => bail!(
            "missing environment variables PROMPTFORGE_GATEWAY_URL and PROMPTFORGE_GATEWAY_KEY\n\
             start promptforge-gateway first, then export both before running promptforge-dev"
        ),
        (None, Some(_)) => bail!(
            "missing environment variable PROMPTFORGE_GATEWAY_URL\n\
             start promptforge-gateway first, then export PROMPTFORGE_GATEWAY_URL and PROMPTFORGE_GATEWAY_KEY"
        ),
        (Some(_), None) => bail!(
            "missing environment variable PROMPTFORGE_GATEWAY_KEY\n\
             start promptforge-gateway first, then export PROMPTFORGE_GATEWAY_URL and PROMPTFORGE_GATEWAY_KEY"
        ),
    }
}

/// Runs one prompt file once against a ready gateway and returns the final
/// result string.
///
/// Requires `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY`. Trace
/// records print to stderr; nothing prints to stdout. Whether the run succeeds
/// or fails, its store is dumped to `<prompt-stem>.store` next to the prompt
/// file, one announcement line per file on stderr.
///
/// # Errors
///
/// Returns an error when gateway credentials are missing, the file cannot be
/// read, the file declares no `promptforge:` version, the catalog cannot be
/// fetched, the prompt fails to parse, or execution fails.
pub(crate) async fn run_once(prompt_path: &Path, input: &str) -> Result<String> {
    let gateway = require_gateway_env()?;
    let observer = VerboseObserver::new(std::io::stderr());
    let models = fetch_model_catalog(&gateway.base_url, &gateway.key)
        .await
        .context("fetch model catalog")?;
    run_once_with(prompt_path, input, &gateway, &models, &observer).await
}

/// [`run_once`] with gateway, catalog, and observer injected for offline tests
/// and the watch loop.
pub(crate) async fn run_once_with(
    prompt_path: &Path,
    input: &str,
    gateway: &GatewayEnv,
    models: &ModelCatalog,
    observer: &dyn Observer,
) -> Result<String> {
    let source = std::fs::read_to_string(prompt_path)
        .with_context(|| format!("read {}", prompt_path.display()))?;
    if promptforge_core::promptforge_version(&source).is_none() {
        bail!(
            "{} is not a promptforge prompt: its frontmatter declares no `promptforge:` version. promptforge runs only promptforge prompts.",
            prompt_path.display()
        );
    }

    let execution = new_execution_id();
    // Banner before observer traffic so a fresh process is obvious in scrollback
    // even when an earlier run's lines are still on screen.
    eprintln!("run id: {execution}");
    let prompt = Prompt::parse(&source, &execution, observer)
        .with_context(|| format!("parse {}", prompt_path.display()))?;

    let available = tools::available_tools(&gateway.base_url, Some(gateway.key.as_str()));
    let picker = ToolPicker::build(available.catalog().clone(), PickerConfig::default())
        .context("build the live tool picker")?;
    let client = GatewayClient::new(&gateway.base_url, gateway.key.as_str());
    let capture = dump::TraceCapture::new(prompt_path);

    // Clear the previous run's dump before starting so stale store files and
    // traces never masquerade as the current run. Mid-run writes go through
    // MirrorStore and TraceCapture; end-of-run reconcile never wipes `.trace/`.
    let dump_dir = dump::dump_directory(prompt_path);
    if dump_dir.is_dir() {
        let _ignored = std::fs::remove_dir_all(&dump_dir);
    }

    let store = StoreRef::new(Box::new(dump::MirrorStore::new(dump_dir)));

    let options = RunOptions {
        execution: &execution,
        observer,
        client: Some(client),
        debug: Some(&capture),
    };
    let result = run(
        &prompt,
        input,
        ResolutionContext {
            picker: &picker,
            models,
        },
        available.tools(),
        &store,
        options,
    )
    .await
    .with_context(|| format!("run {}", prompt_path.display()));
    // Reconcile on success and failure alike: a failed run's partial store is
    // exactly what a debugging author needs, and orphans from deletes land
    // here if MirrorStore skipped them. Status lines go to stderr.
    if let Err(error) = dump::dump_store(&store, prompt_path, &mut std::io::stderr()) {
        eprintln!("store dump failed: {error:#}");
    }
    result
}

/// An observer that writes every record as one line to its sink.
struct VerboseObserver<W> {
    sink: Mutex<W>,
}

impl<W> std::fmt::Debug for VerboseObserver<W> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("VerboseObserver").finish()
    }
}

impl<W: Write + Send> VerboseObserver<W> {
    fn new(sink: W) -> Self {
        Self {
            sink: Mutex::new(sink),
        }
    }
}

impl<W: Write + Send> Observer for VerboseObserver<W> {
    fn observe(&self, execution: &str, section: &str, detail: &str) {
        let mut sink = self.sink.lock().unwrap_or_else(PoisonError::into_inner);
        // Observers must not panic and reporting is a side channel, so a
        // failed write is deliberately dropped rather than surfaced.
        let _ignored = writeln!(sink, "{}", format_record(execution, section, detail));
    }
}

/// Formats one `(execution, section, detail)` record as one trace line.
fn format_record(execution: &str, section: &str, detail: &str) -> String {
    format!("[{execution}] {section}: {detail}")
}

/// Mints a fresh per-invocation execution id: `dev-` plus 128 random bits.
fn new_execution_id() -> String {
    format!("dev-{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::{Arc, Mutex, PoisonError};

    use promptforge_core::model::ModelCatalog;
    use promptforge_core::observe::{Observer, detail};

    use std::path::Path;

    use super::{
        GatewayEnv, VerboseObserver, first_mapped_prompt_line, format_dev_failure, format_record,
        new_execution_id, require_gateway_env_from, run_once_with,
    };

    #[derive(Clone, Debug, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn contents(&self) -> String {
            let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8(bytes.clone()).expect("observer output must be UTF-8")
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn lookup_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[derive(Debug, Default)]
    struct Recorder(Mutex<Vec<(String, String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, execution: &str, section: &str, detail: &str) {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push((
                execution.to_owned(),
                section.to_owned(),
                detail.to_owned(),
            ));
        }
    }

    fn fixture_gateway() -> GatewayEnv {
        GatewayEnv {
            base_url: "http://127.0.0.1:1/v1".to_owned(),
            key: "key".to_owned(),
        }
    }

    #[test]
    fn record_formats_as_one_bracketed_trace_line() {
        assert_eq!(
            format_record("dev-00000000deadbeef", "Research", "Lua: checkpoint"),
            "[dev-00000000deadbeef] Research: Lua: checkpoint"
        );
    }

    #[test]
    fn mapped_lua_failure_leads_with_prompt_path_and_line() {
        let detail = "run briefer.md: lua error: lua error: section `Web Search` epilog:51: \
             [string \"section `Web Search` epilog\"]:51: assertion failed!";
        assert_eq!(first_mapped_prompt_line(detail), Some(51));
        let error = anyhow::anyhow!(detail);
        let formatted = format_dev_failure(Path::new("briefer.md"), &error);
        assert!(
            formatted.starts_with("dev run failed: briefer.md:51:"),
            "expected path:line prefix, got {formatted}"
        );
    }

    #[test]
    fn verbose_observer_writes_every_record_as_its_own_line() {
        let buffer = SharedBuffer::default();
        let observer = VerboseObserver::new(buffer.clone());

        observer.observe("dev-1", "Prompt", "RunStarted");
        observer.observe("dev-1", "Section", "Lua: step one");

        assert_eq!(
            buffer.contents(),
            "[dev-1] Prompt: RunStarted\n[dev-1] Section: Lua: step one\n"
        );
    }

    #[test]
    fn missing_both_gateway_vars_fails_before_parse() {
        let error =
            require_gateway_env_from(lookup_from(&[])).expect_err("both vars missing must fail");
        let message = format!("{error:#}");
        assert!(
            message.contains("PROMPTFORGE_GATEWAY_URL")
                && message.contains("PROMPTFORGE_GATEWAY_KEY"),
            "unexpected missing-env message: {message}"
        );
    }

    #[test]
    fn missing_url_alone_fails_with_a_friendly_message() {
        let error = require_gateway_env_from(lookup_from(&[("PROMPTFORGE_GATEWAY_KEY", "secret")]))
            .expect_err("URL missing must fail");
        let message = format!("{error:#}");
        assert!(
            message.starts_with("missing environment variable PROMPTFORGE_GATEWAY_URL\n"),
            "unexpected missing-url message: {message}"
        );
    }

    #[test]
    fn missing_key_alone_fails_with_a_friendly_message() {
        let error =
            require_gateway_env_from(lookup_from(&[("PROMPTFORGE_GATEWAY_URL", "http://x/v1")]))
                .expect_err("key missing must fail");
        let message = format!("{error:#}");
        assert!(
            message.starts_with("missing environment variable PROMPTFORGE_GATEWAY_KEY\n"),
            "unexpected missing-key message: {message}"
        );
    }

    #[test]
    fn empty_gateway_vars_count_as_missing() {
        let error = require_gateway_env_from(lookup_from(&[
            ("PROMPTFORGE_GATEWAY_URL", ""),
            ("PROMPTFORGE_GATEWAY_KEY", ""),
        ]))
        .expect_err("empty vars must fail");
        assert!(
            format!("{error:#}").contains("PROMPTFORGE_GATEWAY_URL"),
            "unexpected empty-env message: {error:#}"
        );
    }

    #[test]
    fn present_gateway_vars_are_accepted() {
        let gateway = require_gateway_env_from(lookup_from(&[
            ("PROMPTFORGE_GATEWAY_URL", "http://10.0.0.7:9999/v1"),
            ("PROMPTFORGE_GATEWAY_KEY", "dev-secret"),
        ]))
        .expect("both vars present must succeed");
        assert_eq!(gateway.base_url, "http://10.0.0.7:9999/v1");
        assert_eq!(gateway.key, "dev-secret");
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
    async fn run_reuses_one_generated_execution_id_for_parse_bind_and_execution() {
        let directory = tempfile::tempdir().expect("create lifecycle fixture directory");
        let path = directory.path().join("lifecycle.md");
        std::fs::write(
            &path,
            "---\nname: lifecycle\ndescription: lifecycle fixture\npromptforge: 1\n---\n\n\
             # Lifecycle\n\n## Run\n\n```lua\nreturn 'done'\n```\n",
        )
        .expect("write the lifecycle fixture");
        let recorder = Recorder::default();
        // The prologue returns a scalar, so no model turn happens and the
        // unreachable server address below is never contacted.
        let result = run_once_with(
            &path,
            "",
            &fixture_gateway(),
            &ModelCatalog::empty(),
            &recorder,
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
        run_once_with(
            &path,
            "",
            &fixture_gateway(),
            &ModelCatalog::empty(),
            &Recorder::default(),
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

        let error = run_once_with(
            &path,
            "",
            &fixture_gateway(),
            &ModelCatalog::empty(),
            &Recorder::default(),
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

        let error = run_once_with(
            &path,
            "",
            &fixture_gateway(),
            &ModelCatalog::empty(),
            &Recorder::default(),
        )
        .await
        .expect_err("a missing prompt file must fail");

        assert!(
            format!("{error:#}").contains("read") && format!("{error:#}").contains("absent.md"),
            "unexpected read error: {error:#}"
        );
    }
}
