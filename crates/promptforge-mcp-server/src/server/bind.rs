//! The MCP host's prepared semantic picker and complete live tool catalog.
//!
//! Both artifacts are derived from the same concrete instances once, before
//! the async runtime starts. The server then shares this immutable environment
//! across every run. A picker identity therefore cannot exist without a
//! callable catalog entry carrying the same stable identity.

use std::sync::Arc;

use promptforge_core::client::GatewayClient;
use promptforge_core::model::{
    CompletionError, CompletionErrorKind, ModelCatalog, fetch_model_catalog,
};
use promptforge_tool_picker::{
    Catalog, Config as PickerConfig, ToolDescriptor, ToolId as PickerToolId, ToolPicker,
};
use promptforge_tools::{Tool, ToolCatalog};
use promptforge_web_search::WebSearch;
use promptforge_webfetch::WebFetch;

use crate::config::{Config, GatewayConfig, ToolsConfig};
use crate::error::PreparedToolsError;

/// The immutable picker, live tools, and model catalog shared by every server run.
#[non_exhaustive]
pub struct PreparedTools {
    tools: ToolCatalog,
    picker: ToolPicker,
    models: ModelCatalog,
}

/// The prepared environment is shared immutably across every run on every
/// handler clone, so it must cross threads and outlive any one request. A
/// regression that made it otherwise would surface here rather than at a distant
/// `spawn`.
const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<PreparedTools>();
};

impl std::fmt::Debug for PreparedTools {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedTools")
            .field("tools", &self.tools)
            .field("picker", &self.picker)
            .field("models", &self.models)
            .finish()
    }
}

impl PreparedTools {
    /// Builds the complete MCP live tool catalog, picker, and gateway model
    /// catalog.
    ///
    /// A `GET /v1/models` failure is classified before it is acted on. A
    /// *transient* failure - a connection or timeout, or a 5xx the gateway may
    /// recover from - falls back to an empty catalog: prompts without
    /// `models.bind` keep working, and one that declares models fails at live H1
    /// with a model-absent error. A *fatal* misconfiguration - a bad endpoint or
    /// key, a non-5xx backend status such as a 401, or a malformed response - is
    /// propagated instead, so a wrong key or URL refuses to boot rather than
    /// silently serving an empty catalog that fails every `models.bind` prompt.
    ///
    /// # Examples
    /// ```no_run
    /// # use promptforge_mcp_server::{Config, PreparedTools};
    /// # async fn demo(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    /// // A reachable gateway yields a populated catalog; an unreachable one
    /// // still loads, serving without model resolution rather than failing.
    /// let prepared = PreparedTools::load(config).await?;
    /// # let _ = prepared;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`PreparedToolsError`] when the live tool catalog cannot be
    /// assembled, the tool picker index cannot be built, or the gateway model
    /// catalog fails *fatally* (a misconfiguration a retry cannot fix), with the
    /// underlying failure preserved as the error's source. A transient catalog
    /// failure is not an error here: it is logged and the catalog is left empty,
    /// so prompts without `models.bind` keep working.
    pub async fn load(config: &Config) -> Result<Self, PreparedToolsError> {
        let gateway = &config.gateway;
        let models = match fetch_model_catalog(gateway.url.as_str(), gateway.api_key.expose()).await
        {
            Ok(catalog) => catalog,
            Err(error) if is_transient(&error) => {
                // A momentary outage: warn and serve on with an empty catalog,
                // since the gateway may simply not be up yet and every prompt
                // without `models.bind` is unaffected.
                let kind = error.kind();
                tracing::warn!(
                    ?kind,
                    %error,
                    "gateway model catalog unavailable; serving without it"
                );
                ModelCatalog::empty()
            }
            Err(error) => {
                // A fatal misconfiguration: booting with an empty catalog would
                // hide a wrong key or URL behind a `models.bind` prompt that
                // fails at runtime, so it is surfaced at boot with its cause
                // preserved rather than swallowed as a momentary outage.
                let kind = error.kind();
                tracing::error!(
                    ?kind,
                    %error,
                    "gateway model catalog could not be loaded; refusing to serve an empty catalog"
                );
                return Err(PreparedToolsError::tools(error));
            }
        };
        Self::new(gateway, &config.tools, models)
    }

    /// Builds the live catalog and picker over an already-fetched model catalog.
    ///
    /// # Errors
    /// Returns [`PreparedToolsError`] when the live catalog cannot be assembled
    /// or the picker index cannot be built.
    pub(crate) fn new(
        gateway: &GatewayConfig,
        tools_config: &ToolsConfig,
        models: ModelCatalog,
    ) -> Result<Self, PreparedToolsError> {
        let live = live_tools(gateway, tools_config).map_err(PreparedToolsError::tools)?;
        let tools = ToolCatalog::new(&live).map_err(PreparedToolsError::tools)?;
        let picker = ToolPicker::build(catalog(tools.tools()), PickerConfig::default())
            .map_err(PreparedToolsError::picker)?;
        Ok(Self {
            tools,
            picker,
            models,
        })
    }

    /// Rebuilds the environment for another test gateway while reusing this
    /// environment's already-loaded embedding model.
    ///
    /// # Errors
    /// Returns [`PreparedToolsError`] when the new live catalog cannot be
    /// assembled or reindexed.
    #[cfg(test)]
    pub(crate) fn rebuild(
        &self,
        gateway: &GatewayConfig,
        tools_config: &ToolsConfig,
    ) -> Result<Self, PreparedToolsError> {
        let live = live_tools(gateway, tools_config).map_err(PreparedToolsError::tools)?;
        let tools = ToolCatalog::new(&live).map_err(PreparedToolsError::tools)?;
        let picker = self
            .picker
            .rebuild(catalog(tools.tools()))
            .map_err(PreparedToolsError::index)?;
        Ok(Self {
            tools,
            picker,
            models: self.models.clone(),
        })
    }

    /// Returns the validated tool catalog handed to
    /// [`ResolutionContext`](promptforge_core::execute::ResolutionContext).
    #[must_use]
    pub(crate) fn tools(&self) -> &ToolCatalog {
        &self.tools
    }

    /// Returns the process-lifetime prepared semantic picker.
    #[must_use]
    pub(crate) fn picker(&self) -> &ToolPicker {
        &self.picker
    }

    /// Returns the gateway model catalog used for live `models.bind` resolution.
    #[must_use]
    pub(crate) fn models(&self) -> &ModelCatalog {
        &self.models
    }
}

/// Whether a gateway model-catalog fetch failure is transient rather than a
/// fatal misconfiguration.
///
/// Transient means a retry may clear it: a transport connection or timeout, or
/// a 5xx the backend may recover from. Everything else - a bad configuration, a
/// non-5xx backend status such as a 401, or a malformed response - is fatal,
/// since serving an empty catalog would hide it behind a runtime failure of
/// every `models.bind` prompt.
fn is_transient(error: &CompletionError) -> bool {
    match error.kind() {
        CompletionErrorKind::Transport => true,
        CompletionErrorKind::Backend => error.status().is_some_and(|status| status >= 500),
        // Config, MalformedResponse, EmptyReply, Disabled, and any future class
        // are fatal: an unrecognized failure fails closed rather than serving an
        // empty catalog (`CompletionErrorKind` is non-exhaustive).
        _ => false,
    }
}
fn live_tools(
    gateway: &GatewayConfig,
    tools_config: &ToolsConfig,
) -> Result<Vec<Arc<dyn Tool>>, promptforge_tools::ToolError> {
    let mut live: Vec<Arc<dyn Tool>> = Vec::new();
    if tools_config.web_fetch {
        live.push(Arc::new(WebFetch::new()));
    }
    if tools_config.web_search {
        live.push(Arc::new(WebSearch::new(
            gateway.url.as_str(),
            gateway.api_key.expose(),
        )?));
    }
    Ok(live)
}
fn catalog(live: &[Arc<dyn Tool>]) -> Catalog {
    Catalog::new(live.iter().map(|tool| descriptor(tool.as_ref())).collect())
}
/// The client a run's model calls go through, built from the configuration
/// rather than the environment: setting an environment variable is `unsafe`
/// under edition 2024 and this workspace forbids unsafe, so a configured server
/// hands the executor a client instead of arranging for one to be found.
pub(super) fn gateway_client(
    gateway: &GatewayConfig,
) -> Result<GatewayClient, promptforge_core::model::CompletionError> {
    let endpoint = promptforge_core::client::GatewayEndpoint::new(gateway.url.as_str())?;
    let key = promptforge_core::client::SecretString::new(gateway.api_key.expose())?;
    Ok(GatewayClient::new(endpoint, key))
}
/// Derives one abstract descriptor from its callable live instance.
fn descriptor(tool: &dyn Tool) -> ToolDescriptor {
    let id = tool.id();
    ToolDescriptor::new(
        PickerToolId::new(id.server(), id.name()),
        tool.description(),
        tool.parameters_schema(),
    )
}
#[cfg(test)]
mod tests {
    use std::fs;
    use std::num::NonZeroU32;
    use std::path::Path;

    use promptforge_core::execute::{self, ResolutionContext, RunConfig};
    use promptforge_core::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
    use promptforge_core::observe::NullObserver;
    use promptforge_core::parser::Prompt;
    use promptforge_core::store::StoreRef;
    use promptforge_tool_picker::Outcome;

    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;

    use super::{PreparedTools, gateway_client};
    use crate::config::Config;

    fn gateway(extra: &str) -> Config {
        Config::from_toml_str(&format!(
            "[server]\napi_key = \"t\"\n\n[gateway]\nurl = \"http://127.0.0.1:8081/v1/\"\napi_key = \"gw\"\n\n[tools]\nweb_fetch = true\nweb_search = true\n{extra}"
        ))
        .expect("the fixture configuration parses")
    }

    fn collect_markdown(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(directory).expect("read repository prompt directory") {
            let path = entry.expect("read repository prompt entry").path();
            if path.is_dir() {
                collect_markdown(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                files.push(path);
            }
        }
    }

    #[test]
    fn complete_live_catalog_contains_both_canonical_tools() {
        let config = gateway("");
        let tools = PreparedTools::new(
            &config.gateway,
            &config.tools,
            promptforge_core::model::ModelCatalog::empty(),
        )
        .expect("prepare fixture tools");
        let catalog_ids = tools
            .tools()
            .tools()
            .iter()
            .map(|tool| {
                let id = tool.id();
                (id.server().to_owned(), id.name().to_owned())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            catalog_ids,
            [
                ("promptforge".to_owned(), "web_fetch".to_owned()),
                ("promptforge".to_owned(), "web_search".to_owned()),
            ]
        );
    }

    #[test]
    fn a_capability_binds_to_the_matching_live_tool() {
        let config = gateway("");
        let tools = PreparedTools::new(
            &config.gateway,
            &config.tools,
            promptforge_core::model::ModelCatalog::empty(),
        )
        .expect("prepare fixture tools");
        let outcome = tools
            .picker()
            .resolve("Fetch a web page and return its main content as markdown.")
            .expect("resolve available capability");
        assert!(matches!(outcome, Outcome::Bind(tool) if tool.name() == "web_fetch"));
    }

    #[tokio::test]
    async fn every_repository_prompt_parses_and_resolves_live_h1() {
        let config = gateway("");
        let models = ModelCatalog::new([ModelDescriptor::new(
            ModelId::gateway("claude-sonnet-4-6").expect("the test model alias is valid"),
            "A model suited for careful analysis, coding, and general assistance",
            NonZeroU32::new(200_000).expect("200000 is non-zero"),
            ThinkingMode::Never,
        )])
        .expect("the test catalog has a single unique model");
        let tools = PreparedTools::new(&config.gateway, &config.tools, models)
            .expect("prepare repository tools");
        let prompts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts");
        let mut files = Vec::new();
        collect_markdown(&prompts, &mut files);
        files.sort();
        assert_eq!(files.len(), 5, "every shipped markdown prompt is covered");

        for path in files {
            let source = fs::read_to_string(&path).expect("read repository prompt");
            assert!(
                !source.contains("web_search") && !source.contains("web_fetch"),
                "{} must not depend on concrete tool names",
                path.display()
            );
            let first_section = source
                .find(
                    "
## ",
                )
                .unwrap_or_else(|| panic!("{} must have a section", path.display()));
            let mut probe = source[..first_section].to_owned();
            probe.push_str(
                "

## Resolution Probe

```lua
return 'resolved'
```
",
            );
            let mut prompt = Prompt::parse(&probe, "test-run", &NullObserver::default())
                .unwrap_or_else(|error| {
                    panic!("{} must parse: {error}", path.display());
                });
            prompt.strip_h1_prose();
            let result = execute::run(
                &prompt,
                "",
                ResolutionContext::new(tools.picker(), tools.models(), tools.tools()),
                &StoreRef::memory(),
                RunConfig::new("test-run"),
            )
            .await
            .unwrap_or_else(|error| panic!("{} must resolve live H1: {error}", path.display()));
            assert_eq!(result, "resolved");
        }
    }

    /// A configuration whose gateway points at `addr`, for a test stub gateway.
    fn config_for(addr: &str) -> Config {
        Config::from_toml_str(&format!(
            "[server]\napi_key = \"t\"\n\n[gateway]\nurl = \"http://{addr}/v1/\"\napi_key = \"gw\"\n\n[tools]\nweb_fetch = true\nweb_search = true\n"
        ))
        .expect("the fixture configuration parses")
    }

    /// Serves `router` on an ephemeral loopback port, returning its address, the
    /// stop handle, and the join handle to await its clean exit.
    async fn spawn_gateway(
        router: axum::Router,
    ) -> (
        String,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<std::io::Result<()>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener
            .local_addr()
            .expect("read the bound address")
            .to_string();
        let (stop, shutdown) = tokio::sync::oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown.await;
                })
                .await
        });
        (addr, stop, serving)
    }

    async fn stop_gateway(
        stop: tokio::sync::oneshot::Sender<()>,
        serving: tokio::task::JoinHandle<std::io::Result<()>>,
    ) {
        let _ = stop.send(());
        serving
            .await
            .expect("the gateway task joins")
            .expect("the gateway served without error");
    }

    #[tokio::test]
    async fn load_populates_the_model_catalog_from_a_reachable_gateway() {
        // A local gateway answering `GET /v1/models` with one model exercises
        // the successful fetch path end to end.
        async fn models() -> axum::Json<serde_json::Value> {
            axum::Json(serde_json::json!({
                "data": [{
                    "id": "claude-sonnet-4-6",
                    "description": "A model suited for careful analysis, coding, and general assistance",
                    "context": 200_000,
                    "thinking": "never"
                }]
            }))
        }

        let router = Router::new().route("/v1/models", get(models));
        let (addr, stop, serving) = spawn_gateway(router).await;
        let prepared = PreparedTools::load(&config_for(&addr))
            .await
            .expect("a reachable gateway loads");

        assert!(
            !prepared.models().is_empty(),
            "the successful fetch path populates the model catalog"
        );
        assert_eq!(
            prepared.models().models().len(),
            1,
            "the one fetched model is present in the catalog"
        );

        stop_gateway(stop, serving).await;
    }

    #[tokio::test]
    async fn a_transient_gateway_failure_falls_back_to_an_empty_catalog() {
        // A 5xx is a server-side outage a retry may clear, so `load` serves on.
        let router = Router::new().route(
            "/v1/models",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
        let (addr, stop, serving) = spawn_gateway(router).await;
        let prepared = PreparedTools::load(&config_for(&addr))
            .await
            .expect("a transient gateway failure still loads");
        assert!(
            prepared.models().is_empty(),
            "a transient failure leaves the catalog empty rather than refusing to boot"
        );

        stop_gateway(stop, serving).await;
    }

    #[tokio::test]
    async fn a_fatal_gateway_failure_does_not_silently_fall_back() {
        // A 401 authentication failure: booting with an empty catalog would hide
        // the bad key, so `load` propagates it instead of falling back.
        let router = Router::new().route("/v1/models", get(|| async { StatusCode::UNAUTHORIZED }));
        let (addr, stop, serving) = spawn_gateway(router).await;
        let error = PreparedTools::load(&config_for(&addr))
            .await
            .expect_err("a fatal gateway failure refuses to serve an empty catalog");
        assert!(
            std::error::Error::source(&error).is_some(),
            "the gateway failure is preserved as the error's source"
        );

        stop_gateway(stop, serving).await;
    }

    #[test]
    fn gateway_client_is_built_from_url_and_key_without_leaking_the_key() {
        let config = gateway("");
        let client = gateway_client(&config.gateway).expect("the fixture gateway URL is valid");
        let rendered = format!("{client:?}");
        assert!(
            !rendered.contains("gw"),
            "the bearer key must never appear in Debug output, got: {rendered}"
        );
        assert!(
            rendered.contains("http://127.0.0.1:8081/v1") && rendered.contains("<redacted>"),
            "the client Debug must keep the base URL and redact the key, got: {rendered}"
        );
    }
}
