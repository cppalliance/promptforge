//! Single-shot dev-mode prompt execution against a guarded local gateway.
//!
//! [`run_once`] mirrors the CLI pipeline: read the file, require a
//! `promptforge:` version, parse, build the live tool registry, bind, and
//! execute with a [`GatewayClient`] pointed at the caller's already-running
//! `promptforge-gateway`. Every `(execution, section, detail)` observer record
//! streams to stderr; the returned result string is the caller's to print on
//! stdout. On failure the runner returns the error so the caller can print it
//! beside the gateway diagnostics it owns. After every executed run, success
//! or failure, the run's store is dumped beside the prompt file (see
//! [`crate::dump`]).

use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use anyhow::{Context as _, Result, bail, ensure};
use promptforge_core::bind::bind_prompt;
use promptforge_core::client::GatewayClient;
use promptforge_core::execute::{RunOptions, run};
use promptforge_core::model::pinned_qwen_dev_catalog;
use promptforge_core::observe::Observer;
use promptforge_core::parser::Prompt;
use promptforge_core::store::StoreRef;
use promptforge_core::tools::{Tool, ToolRegistry, WebSearch};
use promptforge_tool_picker::{
    Catalog, Config as PickerConfig, ToolDescriptor, ToolId as PickerToolId, ToolPicker,
};
use promptforge_webfetch::WebFetch;

/// Runs one prompt file once against a ready local gateway and returns the
/// final result string.
///
/// `base_url`, `api_key`, and `model_alias` identify the caller's guarded
/// `promptforge-gateway`, exactly as the scenario runner consumes them. The
/// separate credentials for `web_search` come from the process environment
/// (`PROMPTFORGE_GATEWAY_KEY` and `PROMPTFORGE_GATEWAY_URL`) when that tool should hit
/// a different gateway instance. Trace records print to stderr; nothing prints
/// to stdout. Whether the run succeeds or fails, its store is dumped to
/// `<prompt-stem>.store` next to the prompt file, one announcement line per
/// file on stderr.
///
/// # Errors
///
/// Returns an error when an argument is empty, the file cannot be read, the
/// file declares no `promptforge:` version, the prompt fails to parse or
/// bind, or execution fails. The error is returned rather than printed so the
/// caller can render it beside the server diagnostics it owns.
pub(crate) async fn run_once(
    prompt_path: &Path,
    input: &str,
    base_url: &str,
    api_key: &str,
    model_alias: &str,
) -> Result<String> {
    let observer = VerboseObserver::new(std::io::stderr());
    let gateway = GatewayConfig::from_process_env();
    run_once_with(
        prompt_path,
        input,
        base_url,
        api_key,
        model_alias,
        &gateway,
        &observer,
    )
    .await
}

/// [`run_once`] with the gateway configuration and observer injected, which is
/// the seam offline tests and the later watch loop use.
async fn run_once_with(
    prompt_path: &Path,
    input: &str,
    base_url: &str,
    api_key: &str,
    model_alias: &str,
    gateway: &GatewayConfig,
    observer: &dyn Observer,
) -> Result<String> {
    ensure!(!base_url.is_empty(), "dev run requires a server base URL");
    ensure!(!api_key.is_empty(), "dev run requires a server API key");
    ensure!(
        !model_alias.is_empty(),
        "dev run requires a server model alias"
    );

    let source = std::fs::read_to_string(prompt_path)
        .with_context(|| format!("read {}", prompt_path.display()))?;
    if promptforge_core::promptforge_version(&source).is_none() {
        bail!(
            "{} is not a promptforge prompt: its frontmatter declares no `promptforge:` version. promptforge runs only promptforge prompts.",
            prompt_path.display()
        );
    }

    let execution = format!("dev-{:016x}", fastrand::u64(..));
    let prompt = Prompt::parse(&source, &execution, observer)
        .with_context(|| format!("parse {}", prompt_path.display()))?;

    let available = available_tools(gateway);
    let picker = ToolPicker::build(available.catalog().clone(), PickerConfig::default())
        .context("build the live tool picker")?;
    let registry = available.registry();
    let models = pinned_qwen_dev_catalog(model_alias);
    let bound = bind_prompt(prompt, &picker, &registry, &models, &execution, observer)
        .with_context(|| format!("bind {}", prompt_path.display()))?;

    let store = StoreRef::memory();
    let client = GatewayClient::new(base_url, api_key);
    let capture = crate::dump::TraceCapture::new(prompt_path);

    // Clear the previous run's dump before starting so stale trace files
    // are never visible while a new run is in flight.
    let dump_dir = crate::dump::dump_directory(prompt_path);
    if dump_dir.is_dir() {
        let _ = std::fs::remove_dir_all(&dump_dir);
    }

    let options = RunOptions {
        execution: &execution,
        observer,
        client: Some(client),
        debug: Some(&capture),
    };
    let result = run(&bound, input, registry.tools(), &store, options)
        .await
        .with_context(|| format!("run {}", prompt_path.display()));
    // The dump runs on success and failure alike: a failed run's partial
    // store is exactly what a debugging author needs. Dev-mode status lines
    // go to stderr, keeping stdout for the result alone, and a dump failure
    // is reported there rather than displacing the run's own outcome.
    if let Err(error) = crate::dump::dump_store(&store, prompt_path, &mut std::io::stderr()) {
        eprintln!("store dump failed: {error:#}");
    }
    // Trace files are flushed after the store dump so clearing `<stem>.store`
    // cannot erase the turn JSON from the run that just finished.
    capture.flush(&mut std::io::stderr());
    result
}

/// Where `web_search` finds its gateway: the CLI's exact environment
/// semantics, factored so tests inject values instead of mutating the
/// process environment.
#[derive(Debug)]
struct GatewayConfig {
    /// The gateway API root for `web_search`; absent when unset in the environment.
    base_url: Option<String>,
    /// The bearer credential; `web_search` joins the registry only when this
    /// and `base_url` are both present.
    key: Option<String>,
}

impl GatewayConfig {
    /// Reads `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY` from the process
    /// environment.
    fn from_process_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Builds the configuration from an injected variable lookup.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            base_url: lookup("PROMPTFORGE_GATEWAY_URL"),
            key: lookup("PROMPTFORGE_GATEWAY_KEY"),
        }
    }
}

/// The complete set of concrete tools available to one dev run.
///
/// The picker catalog is built directly from `live`, so no descriptor can be
/// offered without a callable tool carrying the same stable identity.
struct AvailableTools {
    live: Vec<Box<dyn Tool>>,
    catalog: Catalog,
}

impl std::fmt::Debug for AvailableTools {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AvailableTools")
            .field(
                "ids",
                &self.live.iter().map(|tool| tool.id()).collect::<Vec<_>>(),
            )
            .field("catalog", &self.catalog)
            .finish()
    }
}

impl AvailableTools {
    /// Returns a registry borrowing every available concrete tool.
    fn registry(&self) -> ToolRegistry<'_> {
        ToolRegistry::new(self.live.iter().map(AsRef::as_ref))
    }

    /// Returns the matching abstract picker catalog.
    fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

/// Builds every concrete tool currently available to a dev run.
///
/// `web_fetch` is unconditional. `web_search` is included only when the
/// gateway configuration carries both a URL and a key, because those are the
/// credentials needed to invoke the gateway; a prompt needing search then
/// fails loudly as `Absent` at bind when either is missing.
fn available_tools(gateway: &GatewayConfig) -> AvailableTools {
    let mut live: Vec<Box<dyn Tool>> = vec![Box::new(WebFetch::new())];
    if let (Some(base_url), Some(key)) = (&gateway.base_url, &gateway.key) {
        live.push(Box::new(WebSearch::new(base_url, key.as_str())));
    }

    let catalog = Catalog::new(live.iter().map(|tool| descriptor(tool.as_ref())).collect());
    AvailableTools { live, catalog }
}

/// Derives one abstract picker descriptor from a live tool instance.
fn descriptor(tool: &dyn Tool) -> ToolDescriptor {
    let id = tool.id();
    ToolDescriptor::new(
        PickerToolId::new(id.server(), id.name()),
        tool.description(),
        tool.parameters_schema(),
    )
}

/// An observer that writes every record as one line to its sink.
///
/// [`run_once`] wires it to stderr, keeping stdout for the result alone.
struct VerboseObserver<W> {
    sink: Mutex<W>,
}

impl<W> std::fmt::Debug for VerboseObserver<W> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("VerboseObserver").finish()
    }
}

impl<W: Write + Send> VerboseObserver<W> {
    /// Wraps `sink` so each record appends one formatted line.
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::{Arc, Mutex, PoisonError};

    use promptforge_core::Error;
    use promptforge_core::bind::bind_prompt;
    use promptforge_core::observe::{NullObserver, Observer, detail};
    use promptforge_core::parser::Prompt;
    use promptforge_tool_picker::{Config, ToolPicker};

    use super::{
        GatewayConfig, VerboseObserver, available_tools, format_record, run_once, run_once_with,
    };

    /// A cloneable in-memory `Write` sink, so a test can keep reading what the
    /// observer wrote after handing the observer its writer.
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

    /// An observer that records every `(execution, section, detail)` tuple,
    /// so a test can assert execution-ID reuse across the whole lifecycle.
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

    #[test]
    fn record_formats_as_one_bracketed_trace_line() {
        assert_eq!(
            format_record("dev-00000000deadbeef", "Research", "Lua: checkpoint"),
            "[dev-00000000deadbeef] Research: Lua: checkpoint"
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
    fn gateway_config_omits_url_and_key_when_unset() {
        let gateway = GatewayConfig::from_lookup(lookup_from(&[]));
        assert!(gateway.base_url.is_none());
        assert!(gateway.key.is_none());
    }

    #[test]
    fn gateway_config_reads_injected_values() {
        let gateway = GatewayConfig::from_lookup(lookup_from(&[
            ("PROMPTFORGE_GATEWAY_URL", "http://10.0.0.7:9999/v1"),
            ("PROMPTFORGE_GATEWAY_KEY", "dev-secret"),
        ]));
        assert_eq!(gateway.base_url.as_deref(), Some("http://10.0.0.7:9999/v1"));
        assert_eq!(gateway.key.as_deref(), Some("dev-secret"));
    }

    #[test]
    fn key_alone_omits_web_search_without_url() {
        let gateway =
            GatewayConfig::from_lookup(lookup_from(&[("PROMPTFORGE_GATEWAY_KEY", "dev-secret")]));
        assert!(gateway.base_url.is_none());
        let available = available_tools(&gateway);
        assert!(
            available
                .registry()
                .tools()
                .iter()
                .all(|tool| tool.id().name() != "web_search")
        );
    }

    #[test]
    fn no_key_leaves_web_fetch_alone_and_search_solo_binds_web_fetch() {
        let gateway = GatewayConfig::from_lookup(lookup_from(&[]));
        let available = available_tools(&gateway);
        let registry = available.registry();
        assert_eq!(registry.tools().len(), 1);
        assert!(
            registry
                .tools()
                .iter()
                .any(|tool| tool.id().name() == "web_fetch")
        );

        let picker = ToolPicker::build(available.catalog().clone(), Config::default())
            .expect("fixture picker should build");
        let prompt = parse_prompt(
            r#"
tools.need("search", "Search the web and return a list of results (title, url, description).")
"#,
        );
        let bound = bind_prompt(
            prompt,
            &picker,
            &registry,
            &promptforge_core::model::ModelCatalog::empty(),
            "dev-test",
            &NullObserver,
        )
        .expect("solo-candidate rule should bind web_fetch as the only match");
        assert_eq!(
            bound.alias_to_id().get("search"),
            Some(&promptforge_core::tools::ToolId::new(
                "promptforge",
                "web_fetch"
            ))
        );
    }

    #[test]
    fn live_registry_and_picker_catalog_have_identical_ids() {
        for key in [&[][..], &[("PROMPTFORGE_GATEWAY_KEY", "dev-secret")][..]] {
            let gateway = GatewayConfig::from_lookup(lookup_from(key));
            let available = available_tools(&gateway);
            let live_ids = available
                .registry()
                .tools()
                .iter()
                .map(|tool| {
                    let id = tool.id();
                    (id.server().to_owned(), id.name().to_owned())
                })
                .collect::<Vec<_>>();
            let picker_ids = available
                .catalog()
                .tools()
                .iter()
                .map(|tool| (tool.server().to_owned(), tool.name().to_owned()))
                .collect::<Vec<_>>();
            assert_eq!(live_ids, picker_ids);
        }
    }

    #[tokio::test]
    async fn run_reuses_one_generated_execution_id_for_parse_bind_and_execution() {
        let directory = tempfile::tempdir().expect("create lifecycle fixture directory");
        let path = directory.path().join("lifecycle.md");
        std::fs::write(
            &path,
            "---\nname: lifecycle\ndescription: dev lifecycle fixture\npromptforge: 1\n---\n\n\
             # Lifecycle\n\n## Run\n\n```lua\nreturn 'done'\n```\n",
        )
        .expect("write the dev lifecycle fixture");
        let recorder = Recorder::default();
        // The preamble returns a scalar, so no model turn happens and the
        // unreachable server address below is never contacted.
        let gateway = GatewayConfig::from_lookup(lookup_from(&[]));

        let result = run_once_with(
            &path,
            "",
            "http://127.0.0.1:1/v1",
            "key",
            "alias",
            &gateway,
            &recorder,
        )
        .await
        .expect("the model-free lifecycle fixture must run offline");

        assert_eq!(result, "done");
        let records = recorder.0.lock().unwrap_or_else(PoisonError::into_inner);
        let execution = records
            .first()
            .map(|(execution, _, _)| execution.as_str())
            .expect("the dev run must emit observations");
        let nonce = execution
            .strip_prefix("dev-")
            .expect("the dev runner must generate its documented execution id prefix");
        assert!(
            nonce.len() == 16 && nonce.chars().all(|c| c.is_ascii_hexdigit()),
            "the dev execution id must carry a 16-hex-digit nonce: {execution}"
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
                "the dev lifecycle must include {expected:?}: {records:#?}"
            );
        }
    }

    /// Writes a model-free fixture whose preamble runs `lua` and returns a
    /// scalar, so no server is ever contacted, then runs it once.
    async fn run_fixture(directory: &tempfile::TempDir, lua: &str) -> anyhow::Result<String> {
        let path = directory.path().join("fixture.md");
        std::fs::write(
            &path,
            format!(
                "---\nname: fixture\ndescription: dev dump fixture\npromptforge: 1\n---\n\n\
                 # Fixture\n\n## Run\n\n```lua\n{lua}\n```\n"
            ),
        )
        .expect("write the dump fixture");
        let gateway = GatewayConfig::from_lookup(lookup_from(&[]));
        run_once_with(
            &path,
            "",
            "http://127.0.0.1:1/v1",
            "key",
            "alias",
            &gateway,
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

        let error = run_once(&path, "", "http://127.0.0.1:1/v1", "key", "alias")
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

        let error = run_once(&path, "", "http://127.0.0.1:1/v1", "key", "alias")
            .await
            .expect_err("a missing prompt file must fail");

        assert!(
            format!("{error:#}").contains("read") && format!("{error:#}").contains("absent.md"),
            "unexpected read error: {error:#}"
        );
    }

    #[tokio::test]
    async fn empty_server_arguments_are_rejected_before_any_work() {
        let directory = tempfile::tempdir().expect("create argument fixture directory");
        let path = directory.path().join("unused.md");

        for (base_url, api_key, model_alias, expected) in [
            ("", "key", "alias", "base URL"),
            ("http://127.0.0.1:1/v1", "", "alias", "API key"),
            ("http://127.0.0.1:1/v1", "key", "", "model alias"),
        ] {
            let error = run_once(&path, "", base_url, api_key, model_alias)
                .await
                .expect_err("an empty server argument must be rejected");
            assert!(
                format!("{error:#}").contains(expected),
                "expected a {expected} rejection, got: {error:#}"
            );
        }
    }

    fn parse_prompt(declarations: &str) -> Prompt {
        Prompt::parse(
            &format!(
                "---\nname: fixture\ndescription: dev registry fixture\npromptforge: 1\n---\n# Fixture\n\n```lua\n{declarations}```\n\n## Run\n\n```lua\nreturn \"done\"\n```\n"
            ),
            "dev-test",
            &NullObserver,
        )
        .expect("fixture prompt should parse")
    }
}
