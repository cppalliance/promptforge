//! Tests for the `web` Plugin's label and `construct`: the tools it names
//! under the installed name, the configuration it accepts, the Host-wide
//! services it reads, the custom fetch policy, and how a call dispatches:
//! to the tool its last segment names, or failing on a tool it lacks.

use std::sync::Arc;

use promptforge_plugin::testing::TestCall;
use promptforge_plugin::{HostServices, Plugin, PluginId, ToolError, ToolErrorKind, ToolId};
use serde_json::{Value, json};
use tokio::runtime::Handle;

use super::{PACKAGE, SEARCH_PROVIDER, TOKIO_RUNTIME, Web};
use crate::config::FetchConfig;
use crate::fetch::FetchClient;
use crate::provider::{SearchError, SearchProvider, SearchQuery, SearchResults};
use crate::search::WebSearch;

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

/// Host-wide services holding the search provider and this runtime's
/// handle as each flag says.
fn services_with(provider: bool, runtime: bool) -> HostServices {
    let mut host = HostServices::new();
    if provider {
        host.provide(&SEARCH_PROVIDER, Arc::new(NoResults))
            .expect("an empty map takes the provider");
    }
    if runtime {
        host.provide(&TOKIO_RUNTIME, Arc::new(Handle::current()))
            .expect("an empty map takes the runtime");
    }
    host
}

fn name(text: &str) -> PluginId {
    PluginId::parse(text).expect("a one-segment Plugin name parses")
}

/// The Plugin installed under `installed` with `config` and both services.
fn construct(installed: &str, config: Value) -> Result<Arc<dyn Plugin>, ToolError> {
    (PACKAGE.construct)(&name(installed), config, &services_with(true, true))
}

#[test]
fn the_package_is_promptforge_web_with_no_prelude_and_no_per_run_needs() {
    assert_eq!(PACKAGE.name, "promptforge/web");
    assert!(PACKAGE.prelude.is_none());
    assert!(PACKAGE.needs.is_empty());
}

#[tokio::test]
async fn construct_names_both_tools_under_the_installed_name() {
    let tools = construct("browse", Value::Null)
        .expect("construct succeeds with both services")
        .tools();
    let ids: Vec<String> = tools.iter().map(|tool| tool.id.to_string()).collect();
    assert_eq!(ids, ["browse/fetch", "browse/search"]);
    let wire_names: Vec<&str> = tools.iter().map(|tool| tool.wire_name.as_str()).collect();
    assert_eq!(wire_names, ["web_fetch", "web_search"]);
    assert!(tools.iter().all(|tool| !tool.survives_stop));
}

#[tokio::test]
async fn construct_accepts_null_or_an_empty_object_and_refuses_any_other_configuration() {
    assert!(construct("web", json!({})).is_ok());
    for config in [json!({ "max_chars": 10 }), json!([]), json!("web")] {
        let Err(error) = construct("web", config.clone()) else {
            panic!("{config} is refused");
        };
        assert_eq!(error.to_string(), "web takes no configuration");
    }
}

#[tokio::test]
async fn construct_fails_naming_whichever_host_wide_service_is_missing() {
    let missing = |provider, runtime| {
        let services = services_with(provider, runtime);
        match (PACKAGE.construct)(&name("web"), Value::Null, &services) {
            Ok(_) => panic!("construct fails without both services"),
            Err(error) => error.to_string(),
        }
    };
    assert_eq!(
        missing(false, true),
        "web needs promptforge/search-provider, and this host provides none"
    );
    assert_eq!(
        missing(true, false),
        "web needs promptforge/tokio-runtime, and this host provides none"
    );
}

#[tokio::test]
async fn a_custom_fetch_policy_reaches_the_fetch_tools_schema() {
    let installed = name("web");
    let web = Web::new(
        &installed,
        FetchClient::new().tool(Handle::current()),
        WebSearch::new(Arc::new(NoResults)),
    )
    .expect("the tool ids parse");
    let policy = FetchConfig::builder()
        .max_chars(10_000)
        .build()
        .expect("valid policy");
    let web = web
        .with_fetch_config(&installed, Handle::current(), policy)
        .expect("the custom policy builds a fetch client");
    let fetch = web
        .tools()
        .into_iter()
        .find(|tool| tool.wire_name == "web_fetch")
        .expect("the fetch tool is offered");
    assert_eq!(
        fetch.parameters_schema["properties"]["max_chars"]["maximum"], 10_000,
        "the fetch tool enforces the custom policy"
    );
}

#[tokio::test]
async fn a_call_to_a_tool_web_does_not_offer_fails_naming_it() {
    let plugin = construct("web", Value::Null).expect("construct succeeds");
    let call = TestCall::new(ToolId::parse("web/browse").expect("the id parses"));
    let error = plugin
        .call(call.context(), json!({}))
        .await
        .expect_err("web has no browse tool");
    assert_eq!(error.to_string(), "web has no tool named browse");
}

#[tokio::test]
async fn a_call_reaches_the_tool_its_last_segment_names() {
    let plugin = construct("browse", Value::Null).expect("construct succeeds");

    let search = TestCall::new(ToolId::parse("browse/search").expect("the id parses"));
    let output = plugin
        .call(search.context(), json!({ "query": "rust" }))
        .await
        .expect("the search tool answers through the provider");
    assert_eq!(output.text(), r#"{"query":"rust","results":[]}"#);

    let fetch = TestCall::new(ToolId::parse("browse/fetch").expect("the id parses"));
    let error = plugin
        .call(fetch.context(), json!({ "url": "https://1.2.3.4/" }))
        .await
        .expect_err("the fetch policy refuses an IP literal host");
    assert_eq!(error.kind(), ToolErrorKind::InvalidArguments);
    assert!(
        error.to_string().contains("ip literal host not allowed"),
        "the fetch tool's policy refused the call, got: {error}"
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
