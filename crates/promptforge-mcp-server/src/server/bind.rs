//! The MCP host's prepared semantic picker and complete live tool registry.
//!
//! Both artifacts are derived from the same concrete instances once, before
//! the async runtime starts. The server then shares this immutable environment
//! across every run. A picker identity therefore cannot exist without a
//! callable registry entry carrying the same stable identity.

use promptforge_core::client::GatewayClient;
use promptforge_core::model::{ModelCatalog, fetch_model_catalog};
use promptforge_core::tools::{Tool, WebSearch};
use promptforge_tool_picker::{
    Catalog, Config as PickerConfig, ToolDescriptor, ToolId as PickerToolId, ToolPicker,
};
use promptforge_webfetch::WebFetch;

use crate::config::GatewayConfig;

/// The immutable picker, live tools, and model catalog shared by every server run.
pub struct PreparedTools {
    live: Vec<std::sync::Arc<dyn Tool>>,
    picker: ToolPicker,
    models: ModelCatalog,
}

impl std::fmt::Debug for PreparedTools {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedTools")
            .field(
                "ids",
                &self.live.iter().map(|tool| tool.id()).collect::<Vec<_>>(),
            )
            .field("picker", &self.picker)
            .field("models", &self.models)
            .finish()
    }
}

impl PreparedTools {
    /// Builds the complete MCP live registry, picker, and gateway model catalog.
    ///
    /// When `GET /v1/models` is unreachable the catalog is empty: prompts without
    /// `models.need` keep working, and a prompt that declares models fails during
    /// live H1 execution with [`promptforge_core::Error::ModelAbsent`].
    ///
    /// # Errors
    /// Returns a boxed error when the tool picker cannot load.
    pub async fn load(gateway: &GatewayConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let models = match fetch_model_catalog(&gateway.url, gateway.key.expose()).await {
            Ok(catalog) => catalog,
            Err(error) => {
                tracing::warn!("gateway model catalog unavailable: {error}");
                ModelCatalog::empty()
            }
        };
        Ok(Self::new(gateway, models)?)
    }

    /// Builds the live registry and picker over an already-fetched model catalog.
    ///
    /// # Errors
    /// Returns the picker error when its model cannot load or the live catalog
    /// cannot be indexed.
    pub fn new(
        gateway: &GatewayConfig,
        models: ModelCatalog,
    ) -> Result<Self, promptforge_tool_picker::BuildError> {
        let live = live_tools(gateway);
        let catalog = catalog(&live);
        let picker = ToolPicker::build(catalog, PickerConfig::default())?;
        Ok(Self {
            live,
            picker,
            models,
        })
    }

    /// Rebuilds the environment for another test gateway while reusing this
    /// environment's already-loaded embedding model.
    ///
    /// # Errors
    /// Returns the picker error when the new live catalog cannot be indexed.
    #[cfg(test)]
    pub(crate) fn rebuild(
        &self,
        gateway: &GatewayConfig,
    ) -> Result<Self, promptforge_tool_picker::IndexError> {
        let live = live_tools(gateway);
        let picker = self.picker.rebuild(catalog(&live))?;
        Ok(Self {
            live,
            picker,
            models: self.models.clone(),
        })
    }

    /// Returns the shared tool arcs for [`promptforge_core::execute::run`].
    #[must_use]
    pub(crate) fn tools(&self) -> &[std::sync::Arc<dyn Tool>] {
        &self.live
    }

    /// Returns the process-lifetime prepared semantic picker.
    #[must_use]
    pub(crate) fn picker(&self) -> &ToolPicker {
        &self.picker
    }

    /// Returns the gateway model catalog used for live `models.need` resolution.
    #[must_use]
    pub(crate) fn models(&self) -> &ModelCatalog {
        &self.models
    }
}

fn live_tools(gateway: &GatewayConfig) -> Vec<std::sync::Arc<dyn Tool>> {
    vec![
        std::sync::Arc::new(WebFetch::new()),
        std::sync::Arc::new(WebSearch::new(&gateway.url, gateway.key.expose())),
    ]
}

fn catalog(live: &[std::sync::Arc<dyn Tool>]) -> Catalog {
    Catalog::new(live.iter().map(|tool| descriptor(tool.as_ref())).collect())
}

/// The client a run's model calls go through, built from the configuration
/// rather than the environment: setting an environment variable is `unsafe`
/// under edition 2024 and this workspace forbids unsafe, so a configured server
/// hands the executor a client instead of arranging for one to be found.
pub(super) fn gateway_client(gateway: &GatewayConfig) -> GatewayClient {
    GatewayClient::new(&gateway.url, gateway.key.expose())
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
    use std::path::Path;

    use promptforge_core::execute::{self, ResolutionContext, RunConfig};
    use promptforge_core::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
    use promptforge_core::observe::NullObserver;
    use promptforge_core::parser::Prompt;
    use promptforge_core::store::StoreRef;
    use promptforge_tool_picker::Outcome;

    use super::{PreparedTools, gateway_client};
    use crate::config::Config;

    fn gateway(extra: &str) -> Config {
        Config::from_toml_str(&format!(
            "[server]\ntoken = \"t\"\n\n[gateway]\nurl = \"http://127.0.0.1:8081/v1/\"\nkey = \"gw\"\n{extra}"
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
    fn complete_live_registry_contains_both_canonical_tools() {
        let config = gateway("");
        let tools = PreparedTools::new(
            &config.gateway,
            promptforge_core::model::ModelCatalog::empty(),
        )
        .expect("prepare fixture tools");
        let registry_ids = tools
            .tools()
            .iter()
            .map(|tool| {
                let id = tool.id();
                (id.server().to_owned(), id.name().to_owned())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            registry_ids,
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
            ModelId::gateway("claude-sonnet-4-6"),
            "A model suited for careful analysis, coding, and general assistance",
            200_000,
            ThinkingMode::Never,
        )]);
        let tools = PreparedTools::new(&config.gateway, models).expect("prepare repository tools");
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
            let mut prompt =
                Prompt::parse(&probe, "test-run", &NullObserver).unwrap_or_else(|error| {
                    panic!("{} must parse: {error}", path.display());
                });
            prompt.strip_h1_prose();
            let result = execute::run(
                &prompt,
                "",
                ResolutionContext::new(tools.picker(), tools.models()),
                tools.tools(),
                &StoreRef::memory(),
                RunConfig::new("test-run"),
            )
            .await
            .unwrap_or_else(|error| panic!("{} must resolve live H1: {error}", path.display()));
            assert_eq!(result, "resolved");
        }
    }

    #[test]
    fn gateway_client_is_built_from_url_and_key_without_leaking_the_key() {
        let config = gateway("");
        let rendered = format!("{:?}", gateway_client(&config.gateway));
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
