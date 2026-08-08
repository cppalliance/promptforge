//! The MCP host's prepared semantic picker and complete live tool registry.
//!
//! Both artifacts are derived from the same concrete instances once, before
//! the async runtime starts. The server then shares this immutable environment
//! across every run. A picker identity therefore cannot exist without a
//! callable registry entry carrying the same stable identity.

use promptforge_core::client::{DEFAULT_MODEL, GatewayClient};
use promptforge_core::model::{ModelCatalog, fetch_model_catalog};
use promptforge_core::tools::{Tool, ToolRegistry, WebSearch};
use promptforge_tool_picker::{
    Catalog, Config as PickerConfig, ToolDescriptor, ToolId as PickerToolId, ToolPicker,
};
use promptforge_webfetch::WebFetch;

use crate::config::GatewayConfig;

/// The immutable picker, live tools, and model catalog shared by every server run.
pub struct PreparedTools {
    live: Vec<Box<dyn Tool>>,
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
    /// `models.need` keep working, and a prompt that declares models fails at
    /// bind with [`promptforge_core::Error::ModelAbsent`].
    ///
    /// # Errors
    /// Returns a boxed error when the tool picker cannot load.
    pub async fn load(gateway: &GatewayConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let models = match fetch_model_catalog(&gateway.url, gateway.token.expose()).await {
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
    ) -> Result<Self, promptforge_tool_picker::Error> {
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
    ) -> Result<Self, promptforge_tool_picker::Error> {
        let live = live_tools(gateway);
        let picker = self.picker.rebuild(catalog(&live))?;
        Ok(Self {
            live,
            picker,
            models: ModelCatalog::empty(),
        })
    }

    /// Returns a registry borrowing every concrete live tool.
    #[must_use]
    pub(crate) fn registry(&self) -> ToolRegistry<'_> {
        ToolRegistry::new(self.live.iter().map(AsRef::as_ref))
    }

    /// Returns the process-lifetime prepared semantic picker.
    #[must_use]
    pub(crate) fn picker(&self) -> &ToolPicker {
        &self.picker
    }

    /// Returns the gateway model catalog used for `models.need` binding.
    #[must_use]
    pub(crate) fn models(&self) -> &ModelCatalog {
        &self.models
    }
}

fn live_tools(gateway: &GatewayConfig) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(WebFetch::new()),
        Box::new(WebSearch::new(&gateway.url, gateway.token.expose())),
    ]
}

fn catalog(live: &[Box<dyn Tool>]) -> Catalog {
    Catalog::new(live.iter().map(|tool| descriptor(tool.as_ref())).collect())
}

/// The client a run's model calls go through, built from the configuration
/// rather than the environment: setting an environment variable is `unsafe`
/// under edition 2024 and this workspace forbids unsafe, so a configured server
/// hands the executor a client instead of arranging for one to be found.
pub(super) fn gateway_client(gateway: &GatewayConfig) -> GatewayClient {
    let model = gateway.model.as_deref().unwrap_or(DEFAULT_MODEL);
    GatewayClient::new(&gateway.url, gateway.token.expose(), model)
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

    use promptforge_core::bind::bind_prompt;
    use promptforge_core::observe::NullObserver;
    use promptforge_core::parser::Prompt;

    use super::{PreparedTools, gateway_client};
    use crate::config::Config;

    fn gateway(model: &str) -> Config {
        Config::from_toml_str(&format!(
            "[server]\ntoken = \"t\"\n\n[gateway]\nurl = \"http://127.0.0.1:8081/v1/\"\ntoken = \"gw\"\n{model}"
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
            .registry()
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
        let prompt = Prompt::parse(
            "---\nname: fixture\ndescription: Binding fixture\npromptforge: 1\n---\n# Fixture\n\n```lua\ntools.need(\"fetch\", \"Fetch a web page and return its main content as markdown.\")\n```\n\n## Run\n\n```lua\nreturn \"done\"\n```\n",
            "test-run",
            &NullObserver,
        )
        .expect("parse fixture prompt");
        let registry = tools.registry();

        let bound = bind_prompt(
            prompt,
            tools.picker(),
            &registry,
            &promptforge_core::model::ModelCatalog::empty(),
            "test-run",
            &NullObserver,
        )
        .expect("bind available capability");

        assert_eq!(
            bound.alias_to_id()["fetch"],
            promptforge_core::tools::ToolId::new("promptforge", "web_fetch")
        );
    }

    #[test]
    fn every_repository_prompt_parses_and_binds() {
        let config = gateway("");
        let tools = PreparedTools::new(
            &config.gateway,
            promptforge_core::model::ModelCatalog::empty(),
        )
        .expect("prepare repository tools");
        let registry = tools.registry();
        let prompts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts");
        let mut files = Vec::new();
        collect_markdown(&prompts, &mut files);
        files.sort();
        assert_eq!(files.len(), 4, "every shipped markdown prompt is covered");

        for path in files {
            let source = fs::read_to_string(&path).expect("read repository prompt");
            assert!(
                !source.contains("web_search") && !source.contains("web_fetch"),
                "{} must not depend on concrete tool names",
                path.display()
            );
            let prompt =
                Prompt::parse(&source, "test-run", &NullObserver).unwrap_or_else(|error| {
                    panic!("{} must parse: {error}", path.display());
                });
            let name = prompt.frontmatter.name.clone();
            let bound = bind_prompt(
                prompt,
                tools.picker(),
                &registry,
                &promptforge_core::model::ModelCatalog::empty(),
                "test-run",
                &NullObserver,
            )
            .unwrap_or_else(|error| panic!("{} must bind: {error}", path.display()));

            if name == "research_person" {
                assert_eq!(
                    bound.alias_to_id().get("search"),
                    Some(&promptforge_core::tools::ToolId::new(
                        "promptforge",
                        "web_search"
                    ))
                );
                assert_eq!(
                    bound.alias_to_id().get("fetch"),
                    Some(&promptforge_core::tools::ToolId::new(
                        "promptforge",
                        "web_fetch"
                    ))
                );
            } else {
                assert!(
                    bound.alias_to_id().is_empty(),
                    "{name} declares no capabilities"
                );
            }
        }
    }

    #[test]
    fn a_configured_model_wins_over_the_default() {
        let config = gateway("model = \"some-other-model\"\n");
        assert_eq!(gateway_client(&config.gateway).model(), "some-other-model");
    }

    #[test]
    fn no_configured_model_falls_back_to_the_core_default() {
        let config = gateway("");
        assert_eq!(
            gateway_client(&config.gateway).model(),
            promptforge_core::client::DEFAULT_MODEL
        );
    }
}
