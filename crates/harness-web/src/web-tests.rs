//! Tests for the `promptforge/web` Plugin: what activation contributes
//! with both services, without either, and on a cancelled run; the custom
//! fetch policy; and the two service keys.

use std::sync::Arc;

use harness::plugin::{HostServices, Plugin, PluginErrorKind, PluginId, RunServices};
use promptforge::cancel::CancelHandle;
use promptforge::tools::ToolId;
use tokio::runtime::Handle;

use super::{SEARCH_PROVIDER, TOKIO_RUNTIME, Web};
use crate::config::FetchConfig;
use crate::provider::{SearchError, SearchProvider, SearchQuery, SearchResults};

/// A provider that finds nothing.
struct NoResults;

#[async_trait::async_trait]
impl SearchProvider for NoResults {
    async fn search(&self, query: SearchQuery) -> Result<SearchResults, SearchError> {
        Ok(SearchResults {
            query: query.query,
            results: Vec::new(),
        })
    }
}

/// Run services over `cancel`, holding the search provider and this
/// runtime's handle as each flag says.
fn services_with(provider: bool, runtime: bool, cancel: CancelHandle) -> RunServices {
    let mut host = HostServices::new();
    if provider {
        host.provide(&SEARCH_PROVIDER, Arc::new(NoResults))
            .expect("an empty map takes the provider");
    }
    if runtime {
        host.provide(&TOKIO_RUNTIME, Arc::new(Handle::current()))
            .expect("an empty map takes the runtime");
    }
    RunServices::with_host(cancel, host)
}

/// Run services holding both services, with a live cancel handle.
fn services() -> RunServices {
    services_with(true, true, CancelHandle::new())
}

#[tokio::test]
async fn activating_the_plugin_contributes_both_tools_under_its_full_id() {
    let plugin = Web::new();
    assert_eq!(
        plugin.id(),
        &PluginId::parse("promptforge/web").expect("valid Plugin id")
    );

    let contribution = plugin.create(&services()).expect("activation succeeds");

    let mut ids: Vec<ToolId> = contribution.tools.iter().map(|tool| tool.id()).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            ToolId::parse("promptforge/web/fetch").expect("valid tool id"),
            ToolId::parse("promptforge/web/search").expect("valid tool id"),
        ]
    );
    for tool in &contribution.tools {
        assert!(
            plugin.id().contains(&tool.id()),
            "every contributed tool lives under the Plugin's id: {}",
            tool.id()
        );
    }

    let mut wire_names: Vec<&str> = contribution
        .tools
        .iter()
        .map(|tool| tool.wire_name())
        .collect();
    wire_names.sort_unstable();
    assert_eq!(wire_names, ["web_fetch", "web_search"]);
}

#[test]
fn the_plugin_needs_the_search_provider_and_the_runtime() {
    let needs: Vec<String> = Web::new().needs().iter().map(ToString::to_string).collect();
    assert_eq!(
        needs,
        ["promptforge/search-provider", "promptforge/tokio-runtime"]
    );
}

#[tokio::test]
async fn a_run_missing_either_service_gets_no_web_tools() {
    let plugin = Web::new();
    for (provider, runtime) in [(false, true), (true, false), (false, false)] {
        let contribution = plugin
            .create(&services_with(provider, runtime, CancelHandle::new()))
            .expect("activation without a service still succeeds");
        assert!(
            contribution.tools.is_empty(),
            "provider {provider}, runtime {runtime}: no tools without both services"
        );
    }
}

#[tokio::test]
async fn activation_on_a_cancelled_run_fails_as_cancelled() {
    let plugin = Web::new();
    let cancel = CancelHandle::new();
    cancel.cancel();

    let err = plugin
        .create(&services_with(true, true, cancel))
        .expect_err("a cancelled run must not activate");
    assert_eq!(err.kind(), PluginErrorKind::Cancelled);
    assert!(err.is_cancelled());
}

#[tokio::test]
async fn a_custom_fetch_policy_is_accepted() {
    let policy = FetchConfig::builder()
        .max_chars(10_000)
        .build()
        .expect("valid policy");
    let plugin = Web::new()
        .with_fetch_config(policy)
        .expect("the custom policy builds a fetch client");

    let contribution = plugin.create(&services()).expect("activation succeeds");
    assert_eq!(contribution.tools.len(), 2);
    let fetch = contribution
        .tools
        .iter()
        .find(|tool| tool.wire_name() == "web_fetch")
        .expect("the fetch tool is contributed");
    assert_eq!(
        fetch.parameters_schema()["properties"]["max_chars"]["maximum"],
        10_000,
        "the fetch tool enforces the custom policy"
    );
}

#[tokio::test]
async fn each_service_key_literal_is_a_namespace_name_id() {
    let mut host = HostServices::new();
    host.provide(&SEARCH_PROVIDER, Arc::new(NoResults))
        .expect("provide accepts the search provider's literal");
    host.provide(&TOKIO_RUNTIME, Arc::new(Handle::current()))
        .expect("provide accepts the runtime's literal");
}
