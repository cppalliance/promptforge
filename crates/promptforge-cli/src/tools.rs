//! Build the CLI's live tool catalog and matching semantic-picker catalog.
//!
//! `web_fetch` runs locally and is always available. `web_search` proxies
//! through the gateway, so it is offered only when a validated [`Gateway::Remote`]
//! configuration supplies both an endpoint and a bearer token. The abstract
//! picker descriptors are derived from the same live instances placed in the
//! catalog, keeping identity, description, and schema agreement structural
//! rather than conventional.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use promptforge_tool_picker::{Catalog, ToolDescriptor, ToolId as PickerToolId};
use promptforge_tools::{Tool, ToolCatalog};
use promptforge_web_search::WebSearch;
use promptforge_webfetch::WebFetch;

/// A validated gateway configuration produced by argument/environment parsing.
///
/// The remote case carries a [`Remote`] whose fields are private and can only be
/// produced by [`Remote::new`], which rejects a blank endpoint or token. There is
/// therefore no way for crate code to construct a "token without endpoint" (or
/// any other blank-field) remote state: the invalid combination is
/// unrepresentable in the type, not merely rejected by one runtime path.
pub(crate) enum Gateway {
    /// No gateway credentials: only local tools, no remote model catalog.
    LocalOnly,
    /// A reachable gateway with a validated endpoint and bearer token.
    Remote(Remote),
    /// Test seam: local tools with an explicitly disabled gateway client.
    #[cfg(test)]
    Disabled,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Gateway::LocalOnly => formatter.write_str("Gateway::LocalOnly"),
            Gateway::Remote(remote) => write!(formatter, "Gateway::Remote({remote:?})"),
            #[cfg(test)]
            Gateway::Disabled => formatter.write_str("Gateway::Disabled"),
        }
    }
}

/// A validated remote gateway: a non-blank endpoint paired with a non-blank
/// bearer token.
///
/// The fields are private to this module and there is no public field or literal
/// constructor, so the only way to obtain a `Remote` is [`Remote::new`], which
/// enforces the invariant. This makes an invalid remote configuration
/// unrepresentable rather than merely unlikely.
pub(crate) struct Remote {
    endpoint: String,
    token: String,
}

impl Remote {
    /// Validates `endpoint` and `token` into a [`Remote`].
    ///
    /// Both values are trimmed; a value that is empty or whitespace-only is
    /// rejected. URL syntax is validated later, at the gateway boundary in
    /// [`WebSearch::new`], so a syntactically malformed but non-blank endpoint
    /// surfaces there rather than being silently accepted as local-only.
    ///
    /// # Errors
    /// Returns an error if the trimmed endpoint or token is empty.
    pub(crate) fn new(endpoint: &str, token: &str) -> Result<Remote> {
        let endpoint = endpoint.trim();
        let token = token.trim();
        if endpoint.is_empty() {
            bail!("gateway endpoint must not be blank");
        }
        if token.is_empty() {
            bail!("gateway token must not be blank");
        }
        Ok(Remote {
            endpoint: endpoint.to_owned(),
            token: token.to_owned(),
        })
    }

    /// Returns the validated gateway endpoint.
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Returns the validated bearer token.
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

impl std::fmt::Debug for Remote {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The bearer token is a secret; never render it through `Debug`.
        formatter
            .debug_struct("Remote")
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// The complete set of concrete tools available to one CLI run.
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

/// Builds every concrete tool available to the CLI for the given `gateway`.
///
/// `web_fetch` is unconditional. `web_search` is included only for
/// [`Gateway::Remote`], using its validated endpoint and bearer token. The
/// completed live set is validated against the callable [`ToolCatalog`] contract
/// before the picker catalog is derived, so an invalid set fails here rather than
/// after unnecessary picker work.
///
/// # Errors
/// Returns an error if `web_search` construction fails (for example, a malformed
/// gateway URL), or if the assembled live set violates the catalog contract
/// (duplicate identity or invalid wire name).
pub(crate) fn available_tools(gateway: &Gateway) -> Result<AvailableTools> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![Arc::new(WebFetch::new())];
    if let Gateway::Remote(remote) = gateway {
        let search = WebSearch::new(remote.endpoint(), remote.token())
            .context("construct the web_search tool")?;
        tools.push(Arc::new(search));
    }

    let live = ToolCatalog::new(&tools).context("validate the live tool catalog")?;

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
    use promptforge_tool_picker::{Config, Outcome, ToolPicker};

    use super::{Gateway, Remote, available_tools};

    const ENDPOINT: &str = "http://127.0.0.1:8081/v1";
    const SEARCH_CAPABILITY: &str =
        "Search the web and return a list of results (title, url, description).";

    fn remote() -> Gateway {
        Gateway::Remote(Remote::new(ENDPOINT, "test-token").expect("valid remote"))
    }

    fn selected_name(picker: &ToolPicker, capability: &str) -> String {
        match picker.resolve(capability).expect("picker should resolve") {
            Outcome::Bind(tool) => tool.name().to_owned(),
            other => panic!("expected one selected tool, got {other:?}"),
        }
    }

    #[test]
    fn remote_constructor_rejects_blank_endpoint_or_token() {
        assert!(Remote::new("", "token").is_err(), "blank endpoint");
        assert!(Remote::new("   ", "token").is_err(), "whitespace endpoint");
        assert!(Remote::new(ENDPOINT, "").is_err(), "blank token");
        assert!(Remote::new(ENDPOINT, "  ").is_err(), "whitespace token");
        let remote = Remote::new("  http://gw/v1  ", "  tok  ").expect("trimmed valid remote");
        assert_eq!(remote.endpoint(), "http://gw/v1");
        assert_eq!(remote.token(), "tok");
    }

    #[test]
    fn remote_debug_never_reveals_the_bearer_token() {
        let secret = "super-secret-bearer-token";
        let gateway = Gateway::Remote(Remote::new(ENDPOINT, secret).expect("valid remote"));
        let rendered = format!("{gateway:?}");
        assert!(
            !rendered.contains(secret),
            "the token must never appear in Debug output: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "Debug output must mark the token as redacted: {rendered}"
        );
    }

    #[test]
    fn malformed_gateway_url_is_rejected_at_tool_assembly() {
        // A non-blank but syntactically invalid endpoint passes `Remote::new`
        // and is rejected at the gateway boundary in `WebSearch::new`.
        let gateway = Gateway::Remote(Remote::new("not a url", "test-token").expect("non-blank"));
        assert!(
            available_tools(&gateway).is_err(),
            "a malformed gateway URL must fail tool assembly"
        );
    }

    #[test]
    fn web_search_availability_and_binding_track_the_gateway_mode() {
        // One table over the two representable gateway modes: local-only omits
        // web_search and the solo web_fetch absorbs the search capability;
        // remote installs web_search and the search capability binds to it.
        for (gateway, expect_search, expected_search_binding) in [
            (Gateway::LocalOnly, false, "web_fetch"),
            (remote(), true, "web_search"),
        ] {
            let available = available_tools(&gateway).expect("available tools build");
            let has_search = available
                .tools()
                .tools()
                .iter()
                .any(|tool| tool.id().name() == "web_search");
            assert_eq!(has_search, expect_search, "gateway {gateway:?}");
            assert_eq!(
                available
                    .catalog()
                    .iter()
                    .any(|tool| tool.name() == "web_search"),
                expect_search,
                "catalog must mirror the live set for {gateway:?}",
            );

            let picker = ToolPicker::build(available.catalog().clone(), Config::default())
                .expect("fixture picker should build");
            assert_eq!(
                selected_name(&picker, SEARCH_CAPABILITY),
                expected_search_binding,
                "search capability binding for {gateway:?}",
            );
            assert_eq!(
                selected_name(
                    &picker,
                    "Fetch a web page and return its main content as markdown."
                ),
                "web_fetch",
            );
        }
    }

    #[test]
    fn live_catalog_and_picker_catalog_have_identical_ids() {
        for gateway in [Gateway::LocalOnly, remote()] {
            let available = available_tools(&gateway).expect("available tools build");
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

            assert_eq!(live_ids, picker_ids, "for {gateway:?}");
        }
    }
}
