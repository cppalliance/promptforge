//! Build the CLI's live tool registry and matching semantic-picker catalog.
//!
//! `web_fetch` runs locally and is always available. `web_search` proxies
//! through the gateway, so it is omitted when no bearer credential is
//! installed. The abstract picker descriptors are derived from the same live
//! instances placed in the registry, keeping identity, description, and schema
//! agreement structural rather than conventional.

use std::sync::Arc;

use promptforge_core::tools::{Tool, WebSearch};
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
    use promptforge_tool_picker::{Config, Outcome, ToolPicker};

    use super::available_tools;

    const BASE_URL: &str = "http://127.0.0.1:8081/v1";

    fn selected_name(picker: &ToolPicker, capability: &str) -> String {
        match picker.resolve(capability).expect("picker should resolve") {
            Outcome::Bind(tool) => tool.name().to_owned(),
            other => panic!("expected one selected tool, got {other:?}"),
        }
    }

    #[test]
    fn available_capability_binds_to_live_tool() {
        let available = available_tools(BASE_URL, None);
        let picker = ToolPicker::build(available.catalog().clone(), Config::default())
            .expect("fixture picker should build");
        assert_eq!(
            selected_name(
                &picker,
                "Fetch a web page and return its main content as markdown."
            ),
            "web_fetch"
        );
    }

    #[test]
    fn key_without_url_excludes_web_search_and_solo_candidate_binds_web_fetch() {
        let available = available_tools("", Some("test-token"));
        assert!(
            available
                .tools()
                .iter()
                .all(|tool| tool.id().name() != "web_search")
        );
        let picker = ToolPicker::build(available.catalog().clone(), Config::default())
            .expect("fixture picker should build");
        assert_eq!(
            selected_name(
                &picker,
                "Search the web and return a list of results (title, url, description)."
            ),
            "web_fetch"
        );
    }

    #[test]
    fn token_includes_web_search_and_need_can_bind() {
        let available = available_tools(BASE_URL, Some("test-token"));
        assert!(
            available
                .tools()
                .iter()
                .any(|tool| tool.id().name() == "web_search")
        );
        assert!(
            available
                .catalog()
                .iter()
                .any(|tool| tool.name() == "web_search")
        );
        let picker = ToolPicker::build(available.catalog().clone(), Config::default())
            .expect("fixture picker should build");
        assert_eq!(
            selected_name(
                &picker,
                "Search the web and return a list of results (title, url, description)."
            ),
            "web_search"
        );
    }

    #[test]
    fn no_token_excludes_web_search_and_solo_candidate_binds_web_fetch() {
        let available = available_tools(BASE_URL, None);
        assert!(
            available
                .tools()
                .iter()
                .all(|tool| tool.id().name() != "web_search")
        );
        assert!(
            available
                .catalog()
                .iter()
                .all(|tool| tool.name() != "web_search")
        );
        let picker = ToolPicker::build(available.catalog().clone(), Config::default())
            .expect("fixture picker should build");
        assert_eq!(
            selected_name(
                &picker,
                "Search the web and return a list of results (title, url, description)."
            ),
            "web_fetch"
        );
    }

    #[test]
    fn live_registry_and_picker_catalog_have_identical_ids() {
        for token in [None, Some("test-token")] {
            let available = available_tools(BASE_URL, token);
            let live_ids = available
                .tools()
                .iter()
                .map(|tool| {
                    let id = tool.id();
                    (id.server().to_owned(), id.name().to_owned())
                })
                .collect::<Vec<_>>();
            let picker_ids = available
                .catalog()
                .iter()
                .map(|tool| (tool.server().to_owned(), tool.name().to_owned()))
                .collect::<Vec<_>>();

            assert_eq!(live_ids, picker_ids);
        }
    }
}
