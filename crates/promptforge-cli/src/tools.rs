//! Build the CLI's live tool registry and matching semantic-picker catalog.
//!
//! `web_fetch` runs locally and is always available. `web_search` proxies
//! through the gateway, so it is omitted when no bearer credential is
//! installed. The abstract picker descriptors are derived from the same live
//! instances placed in the registry, keeping identity, description, and schema
//! agreement structural rather than conventional.

use std::sync::Arc;

use promptforge_core::tools::{Tool, ToolRegistry, WebSearch};
use promptforge_tool_picker::{Catalog, ToolDescriptor, ToolId as PickerToolId};
use promptforge_webfetch::WebFetch;

/// The complete set of concrete tools available to one CLI run.
///
/// The picker catalog is built directly from `live`, so no descriptor can be
/// offered without a callable tool carrying the same stable identity.
pub(crate) struct AvailableTools {
    live: Vec<Arc<dyn Tool>>,
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
    pub(crate) fn registry(&self) -> ToolRegistry<'_> {
        ToolRegistry::new(self.live.iter().map(AsRef::as_ref))
    }

    /// Returns the shared tool arcs for [`promptforge_core::execute::run`].
    #[must_use]
    pub(crate) fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.live
    }

    /// Returns the matching abstract picker catalog.
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

/// Builds every concrete tool currently available to the CLI.
///
/// `web_fetch` is unconditional. `web_search` is included only when `key` and a
/// non-empty gateway URL are both present, because that bearer and base URL are
/// the credentials needed to invoke the gateway.
pub(crate) fn available_tools(base_url: &str, key: Option<&str>) -> AvailableTools {
    let mut live: Vec<Arc<dyn Tool>> = vec![Arc::new(WebFetch::new())];
    if let Some(key) = key.filter(|_| !base_url.is_empty()) {
        live.push(Arc::new(WebSearch::new(base_url, key)));
    }

    let catalog = Catalog::new(live.iter().map(|tool| descriptor(tool.as_ref())).collect());
    AvailableTools { live, catalog }
}

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
    use promptforge_core::bind::bind_prompt;
    use promptforge_core::observe::NullObserver;
    use promptforge_core::parser::Prompt;
    use promptforge_tool_picker::{Config, ToolPicker};

    use super::available_tools;

    const BASE_URL: &str = "http://127.0.0.1:8081/v1";

    #[test]
    fn available_capability_binds_to_live_tool() {
        let available = available_tools(BASE_URL, None);
        let picker = ToolPicker::build(available.catalog().clone(), Config::default())
            .expect("fixture picker should build");
        let registry = available.registry();
        let prompt = parse_prompt(
            r#"
tools.need("fetch", "Fetch a web page and return its main content as markdown.")
"#,
        );

        let bound = bind_prompt(
            prompt,
            &picker,
            &registry,
            &promptforge_core::model::ModelCatalog::empty(),
            "test-run",
            &NullObserver,
        )
        .expect("available fetch capability should bind");

        assert_eq!(
            bound.alias_to_id().get("fetch"),
            Some(&promptforge_core::tools::ToolId::new(
                "promptforge",
                "web_fetch"
            ))
        );
    }

    #[test]
    fn key_without_url_excludes_web_search_and_solo_candidate_binds_web_fetch() {
        let available = available_tools("", Some("test-token"));
        let registry = available.registry();
        assert!(
            registry
                .tools()
                .iter()
                .all(|tool| tool.id().name() != "web_search")
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
            "test-run",
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
    fn token_includes_web_search_and_need_can_bind() {
        let available = available_tools(BASE_URL, Some("test-token"));
        let registry = available.registry();
        assert!(
            registry
                .tools()
                .iter()
                .any(|tool| tool.id().name() == "web_search")
        );
        assert!(
            available
                .catalog()
                .tools()
                .iter()
                .any(|tool| tool.name() == "web_search")
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
            "test-run",
            &NullObserver,
        )
        .expect("available search capability should bind");

        assert_eq!(
            bound.alias_to_id().get("search"),
            Some(&promptforge_core::tools::ToolId::new(
                "promptforge",
                "web_search"
            ))
        );
    }

    #[test]
    fn no_token_excludes_web_search_and_solo_candidate_binds_web_fetch() {
        let available = available_tools(BASE_URL, None);
        let registry = available.registry();
        assert!(
            registry
                .tools()
                .iter()
                .all(|tool| tool.id().name() != "web_search")
        );
        assert!(
            available
                .catalog()
                .tools()
                .iter()
                .all(|tool| tool.name() != "web_search")
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
            "test-run",
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
        for token in [None, Some("test-token")] {
            let available = available_tools(BASE_URL, token);
            let registry = available.registry();
            let live_ids = registry
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

    fn parse_prompt(declarations: &str) -> Prompt {
        Prompt::parse(
            &format!(
                "---\nname: fixture\ndescription: CLI registry fixture\npromptforge: 1\n---\n# Fixture\n\n```lua shared\n{declarations}```\n\n## Run\n\n```lua\nreturn \"done\"\n```\n"
            ),
            "test-run",
            &NullObserver,
        )
        .expect("fixture prompt should parse")
    }
}
