//! The `web` Plugin's label, [`PACKAGE`], the `construct` that builds its
//! one shared object, and the keys of the two Host-wide services that
//! `construct` reads.
//!
//! The fetch client is built once, at install, with the default fetch
//! policy and bound to the Host's runtime handle; the search tool is
//! built over the Host's search provider. The prompt never sees either.

use std::sync::Arc;

use promptforge_plugin::{
    HostServices, Package, Plugin, PluginFuture, PluginId, ServiceKey, ToolContext, ToolDescriptor,
    ToolError, ToolId, ToolOutput,
};
use serde_json::{Value, json};
use tokio::runtime::Handle;

use crate::fetch::{FetchClient, WebFetch};
use crate::provider::SearchProvider;
use crate::search::WebSearch;

/// The Plugin's label, which a Host passes to its install.
///
/// Its name is `promptforge/web`, so a Host that picks no name installs it
/// as `web`. It has no prelude and needs no per-run service; its
/// `construct` reads [`SEARCH_PROVIDER`] and [`TOKIO_RUNTIME`] from the
/// Host-wide services and takes no configuration but `null` or `{}`.
pub const PACKAGE: Package = Package::new("promptforge/web", construct);

/// The service key for the Host's search provider.
///
/// The key's id is `promptforge/search-provider`. A Host provides it among
/// its Host-wide services, and every search runs through it.
pub const SEARCH_PROVIDER: ServiceKey<dyn SearchProvider> =
    ServiceKey::new("promptforge/search-provider");

/// The service key for the Host's tokio runtime handle.
///
/// The key's id is `promptforge/tokio-runtime`. A Host provides it among
/// its Host-wide services, and every fetch is spawned onto it.
pub const TOKIO_RUNTIME: ServiceKey<Handle> = ServiceKey::new("promptforge/tokio-runtime");

/// The one object every run shares: its two-tool list and the two tools.
#[derive(Debug)]
struct Web {
    tools: Vec<ToolDescriptor>,
    fetch: WebFetch,
    search: WebSearch,
}

/// Builds the Plugin under `name`: refuses any configuration but `null` or
/// `{}`, reads the two Host-wide services or fails naming the missing one,
/// and names the tools `<name>/fetch` and `<name>/search`.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the Package construct signature fixes the argument types"
)]
fn construct(
    name: &PluginId,
    config: Value,
    services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    if !(config.is_null() || config == json!({})) {
        return Err(ToolError::message("web takes no configuration"));
    }
    let provider = services.get(&SEARCH_PROVIDER).ok_or_else(|| {
        ToolError::message("web needs promptforge/search-provider, and this host provides none")
    })?;
    let runtime = services.get(&TOKIO_RUNTIME).ok_or_else(|| {
        ToolError::message("web needs promptforge/tokio-runtime, and this host provides none")
    })?;
    let web = Web::new(
        name,
        FetchClient::new().tool(Handle::clone(&runtime)),
        WebSearch::new(provider),
    )?;
    Ok(Arc::new(web))
}

impl Web {
    /// The Plugin over `fetch` and `search`, its tools named under `name`.
    fn new(name: &PluginId, fetch: WebFetch, search: WebSearch) -> Result<Web, ToolError> {
        let id = |tool: &str| {
            ToolId::parse(&format!("{name}/{tool}"))
                .map_err(|e| ToolError::with_source("web could not name its tools", e))
        };
        let tools = vec![
            ToolDescriptor::new(id("fetch")?, fetch.description(), fetch.parameters_schema()),
            ToolDescriptor::new(
                id("search")?,
                search.description(),
                search.parameters_schema(),
            ),
        ];
        Ok(Web {
            tools,
            fetch,
            search,
        })
    }

    /// Replaces the default fetch policy with a validated custom one,
    /// rebuilding the fetch tool's descriptor to match.
    #[cfg(test)]
    fn with_fetch_config(
        self,
        name: &PluginId,
        runtime: Handle,
        config: crate::config::FetchConfig,
    ) -> Result<Web, crate::config::ConfigError> {
        let fetch = FetchClient::try_with_config(config)?.tool(runtime);
        Ok(Web::new(name, fetch, self.search).expect("the tool ids parsed once already"))
    }
}

impl Plugin for Web {
    fn tools(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            match cx.tool().name() {
                "fetch" => self.fetch.call(args).await,
                "search" => self.search.call(args).await,
                other => Err(ToolError::message(format!("web has no tool named {other}"))),
            }
        })
    }
}

#[cfg(test)]
#[path = "web-tests.rs"]
mod tests;
