//! Build the runner's live tool catalog and matching semantic-picker catalog.
//!
//! Both tools are always live: `web_fetch` runs locally and `web_search`
//! proxies through the gateway. Unlike the CLI, the dev runner has no
//! offline mode: it always has a validated gateway URL and bearer credential
//! (see [`crate::config::GatewayEnv`]), so both concrete tools are constructed
//! unconditionally. The assembled live set is validated against the callable
//! [`ToolCatalog`] contract before the picker catalog is derived, so an
//! invalid set fails here rather than after unnecessary picker work, and the
//! picker catalog can never advertise a descriptor with no matching callable tool.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use promptforge_core::tools::{Tool, ToolCatalog, WebSearch};
use promptforge_tool_picker::{Catalog, ToolDescriptor, ToolId as PickerToolId};
use promptforge_webfetch::WebFetch;

/// The complete set of concrete tools available to one run.
///
/// The picker catalog is built directly from the validated tool catalog, so
/// no descriptor can be offered without a callable tool carrying the same
/// stable identity.
pub(crate) struct AvailableTools {
    live: ToolCatalog,
    catalog: Catalog,
}

impl std::fmt::Debug for AvailableTools {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AvailableTools")
            .field("live", &self.live)
            .field("catalog", &self.catalog)
            .finish()
    }
}

impl AvailableTools {
    /// Returns the validated tool catalog handed to
    /// [`ResolutionContext`](promptforge_core::execute::ResolutionContext).
    #[must_use]
    pub(crate) fn tools(&self) -> &ToolCatalog {
        &self.live
    }

    /// Returns the matching abstract picker catalog.
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

/// Builds every concrete tool available to the dev runner.
///
/// Both `web_fetch` and `web_search` are always constructed: the dev runner
/// only runs with a validated gateway URL and bearer credential, so the
/// offline case the CLI models is unreachable here.
///
/// # Errors
/// Returns an error if `web_search` construction fails (for example, a
/// malformed gateway URL or a blank credential), or if the assembled live set
/// violates the catalog contract (duplicate identity or invalid wire name).
pub(crate) fn available_tools(base_url: &str, key: &str) -> Result<AvailableTools> {
    let web_search = WebSearch::new(base_url, key).context("construct the web_search tool")?;
    let live: Vec<Arc<dyn Tool>> = vec![Arc::new(WebFetch::new()), Arc::new(web_search)];
    assemble(&live)
}

/// Validates the complete live tool set and derives its picker catalog.
///
/// Validation runs through [`ToolCatalog::new`], the existing boundary that
/// rejects a duplicate stable identity or a non-transport-legal wire name,
/// before any picker work.
fn assemble(tools: &[Arc<dyn Tool>]) -> Result<AvailableTools> {
    let live = ToolCatalog::new(tools).context("validate the live tool catalog")?;
    let catalog = Catalog::new(
        live.tools()
            .iter()
            .map(|tool| descriptor(tool.as_ref()))
            .collect(),
    );
    Ok(AvailableTools { live, catalog })
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
    use std::sync::Arc;

    use promptforge_core::tools::{Tool, ToolError, ToolId, ToolOutput};
    use promptforge_tool_picker::{Config, Outcome, ToolPicker};

    use super::{assemble, available_tools};

    const BASE_URL: &str = "http://127.0.0.1:8081/v1";

    /// A minimal tool used only to exercise catalog validation with
    /// caller-chosen identity and wire name.
    struct FakeTool {
        id: ToolId,
        wire: &'static str,
    }

    #[async_trait::async_trait]
    impl Tool for FakeTool {
        fn id(&self) -> ToolId {
            self.id.clone()
        }
        fn wire_name(&self) -> &str {
            self.wire
        }
        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait ties the returned &str to &self"
        )]
        fn description(&self) -> &str {
            "fake tool for validation tests"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        async fn call(&self, _args: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::trusted("ok".to_owned()))
        }
    }

    fn fake(server: &str, name: &str, wire: &'static str) -> Arc<dyn Tool> {
        Arc::new(FakeTool {
            id: ToolId::new(server, name).expect("valid tool id"),
            wire,
        })
    }

    fn selected_name(picker: &ToolPicker, capability: &str) -> String {
        match picker.resolve(capability).expect("picker should resolve") {
            Outcome::Bind(tool) => tool.name().to_owned(),
            other => panic!("expected one selected tool, got {other:?}"),
        }
    }

    #[test]
    fn both_tools_are_always_constructed_and_bind() {
        let available = available_tools(BASE_URL, "test-token").expect("available tools build");
        assert!(
            available
                .tools()
                .tools()
                .iter()
                .any(|tool| tool.id().name() == "web_search"),
            "web_search must always be present"
        );
        assert!(
            available
                .tools()
                .tools()
                .iter()
                .any(|tool| tool.id().name() == "web_fetch"),
            "web_fetch must always be present"
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
        assert_eq!(
            selected_name(
                &picker,
                "Fetch a web page and return its main content as markdown."
            ),
            "web_fetch"
        );
    }

    #[test]
    fn a_malformed_gateway_url_is_rejected() {
        assert!(
            available_tools("not-a-url", "test-token").is_err(),
            "a malformed gateway URL must fail tool assembly"
        );
    }

    #[test]
    fn a_blank_credential_is_rejected() {
        assert!(
            available_tools(BASE_URL, "").is_err(),
            "an empty bearer credential must fail tool assembly"
        );
    }

    #[test]
    fn live_catalog_and_picker_catalog_have_identical_ids() {
        let available = available_tools(BASE_URL, "test-token").expect("available tools build");
        let live_ids = available
            .tools()
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

    #[test]
    fn assembly_rejects_a_duplicate_identity() {
        let live = vec![
            fake("promptforge", "dup", "dup_a"),
            fake("promptforge", "dup", "dup_b"),
        ];
        let error = assemble(&live).expect_err("a duplicate identity must fail validation");
        assert!(
            format!("{error:#}").contains("duplicate tool identity"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn assembly_rejects_an_invalid_wire_name() {
        let live = vec![fake("promptforge", "bad", "not/legal")];
        let error = assemble(&live).expect_err("an invalid wire name must fail validation");
        assert!(
            format!("{error:#}").contains("wire name"),
            "unexpected error: {error:#}"
        );
    }
}
