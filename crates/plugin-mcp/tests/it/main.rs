//! Integration tests for `plugin-mcp`, through `PACKAGE` and `TestCall`
//! only, against a hand-written Streamable HTTP server.
#![expect(
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod fixture;

use std::sync::Arc;
use std::time::Duration;

use fixture::Fixture;
use promptforge_plugin::testing::TestCall;
use promptforge_plugin::{
    HostServices, OutputTrust, Plugin, PluginId, ServiceKey, TOKIO_RUNTIME, ToolError,
    ToolErrorKind, ToolId,
};
use serde_json::{Value, json};
use tokio::runtime::Handle;

const RUNTIME: ServiceKey<Handle> = ServiceKey::new(TOKIO_RUNTIME);
const SECRET: &str = "ghp_secret_token_value_0042";

fn runtime_services() -> HostServices {
    let mut services = HostServices::new();
    services
        .provide(&RUNTIME, Arc::new(Handle::current()))
        .expect("an empty map takes the runtime");
    services
}

fn construct(config: Value, services: &HostServices) -> Result<Arc<dyn Plugin>, ToolError> {
    let name = PluginId::parse("remote").expect("a Plugin name parses");
    (plugin_mcp::PACKAGE.construct)(&name, config, services)
}

/// Installs the Plugin against `fixture` with these extra headers.
fn install(fixture: &Fixture, headers: &Value) -> Arc<dyn Plugin> {
    construct(
        json!({ "url": fixture.url, "headers": headers }),
        &runtime_services(),
    )
    .expect("a remote entry installs")
}

async fn ready(plugin: &Arc<dyn Plugin>) {
    tokio::time::timeout(Duration::from_secs(30), plugin.ready())
        .await
        .expect("the server settles")
        .expect("the server is ready");
}

async fn call(plugin: &Arc<dyn Plugin>, tool: &str, args: Value) -> Result<String, ToolError> {
    let test = TestCall::new(ToolId::parse(tool).expect("a tool id parses"));
    let output = plugin.call(test.context(), args).await?;
    assert_eq!(
        output.trust(),
        OutputTrust::Untrusted,
        "every output is untrusted"
    );
    Ok(output.text().to_owned())
}

#[test]
fn the_package_is_promptforge_mcp_with_no_prelude_and_no_needs() {
    assert_eq!(plugin_mcp::PACKAGE.name, "promptforge/mcp");
    assert!(plugin_mcp::PACKAGE.prelude.is_none());
    assert!(plugin_mcp::PACKAGE.needs.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_initialize_request_names_promptforge_and_declares_no_capabilities() {
    let fixture = Fixture::start().await;
    let plugin = install(&fixture, &json!({}));
    ready(&plugin).await;

    let initializes = fixture
        .shared
        .recorded
        .lock()
        .expect("lock")
        .initializes
        .clone();
    assert_eq!(initializes.len(), 1);
    assert_eq!(initializes[0]["clientInfo"]["name"], "PromptForge");
    assert_eq!(initializes[0]["capabilities"], json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_resolves_with_the_tools_from_both_pages_named_under_the_plugin() {
    let fixture = Fixture::start().await;
    let plugin = install(&fixture, &json!({}));
    ready(&plugin).await;

    let ids: Vec<String> = plugin.tools().iter().map(|t| t.id.to_string()).collect();
    assert_eq!(ids, ["remote/echo", "remote/fail", "remote/hang"]);
    ready(&plugin).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_configured_headers_arrive_on_every_request() {
    let fixture = Fixture::start().await;
    let plugin = install(
        &fixture,
        &json!({ "Authorization": format!("Bearer {SECRET}"), "X-MCP-Tools": "echo" }),
    );
    ready(&plugin).await;
    call(&plugin, "remote/echo", json!({}))
        .await
        .expect("the call succeeds");

    let seen = fixture.shared.recorded.lock().expect("lock").seen.clone();
    let methods: Vec<&str> = seen.iter().map(|s| s.method.as_str()).collect();
    assert!(methods.contains(&"initialize"));
    assert!(methods.contains(&"notifications/initialized"));
    assert!(methods.contains(&"tools/list"));
    assert!(methods.contains(&"tools/call"));
    for request in &seen {
        assert_eq!(
            request.headers["authorization"],
            format!("Bearer {SECRET}"),
            "{}",
            request.method
        );
        assert_eq!(request.headers["x-mcp-tools"], "echo", "{}", request.method);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_sends_the_original_tool_name_and_returns_untrusted_text() {
    let fixture = Fixture::start().await;
    let plugin = install(&fixture, &json!({}));
    ready(&plugin).await;

    let text = call(&plugin, "remote/echo", json!({ "q": "hello" }))
        .await
        .expect("succeeds");
    assert_eq!(text, r#"echo: {"q":"hello"}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_is_error_result_becomes_a_backend_error() {
    let fixture = Fixture::start().await;
    let plugin = install(&fixture, &json!({}));
    ready(&plugin).await;

    let error = call(&plugin, "remote/fail", json!({}))
        .await
        .expect_err("isError fails");
    assert_eq!(error.kind(), ToolErrorKind::Backend);
    assert_eq!(error.to_string(), "it failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn arguments_that_are_not_an_object_are_refused_before_anything_is_sent() {
    let fixture = Fixture::start().await;
    let plugin = install(&fixture, &json!({}));
    ready(&plugin).await;

    let error = call(&plugin, "remote/echo", json!([1]))
        .await
        .expect_err("refused");
    assert_eq!(error.kind(), ToolErrorKind::InvalidArguments);
    assert!(!fixture.methods().contains(&"tools/call".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_call_makes_the_server_receive_a_cancel_for_its_request_id() {
    let fixture = Fixture::start().await;
    let plugin = install(&fixture, &json!({}));
    ready(&plugin).await;

    let test = TestCall::new(ToolId::parse("remote/hang").expect("parses"));
    {
        let hanging = plugin.call(test.context(), json!({}));
        tokio::select! {
            _ = hanging => panic!("the hang tool never answers"),
            () = async {
                tokio::time::timeout(Duration::from_secs(30), fixture.shared.hang_started.notified())
                    .await
                    .expect("the call reaches the server");
            } => {}
        }
    }
    tokio::time::timeout(
        Duration::from_secs(30),
        fixture.shared.cancel_arrived.notified(),
    )
    .await
    .expect("dropping the call tells the server");

    let recorded = fixture.shared.recorded.lock().expect("lock");
    assert_eq!(recorded.cancelled.len(), 1);
    assert_eq!(
        Some(recorded.cancelled[0]["requestId"].clone()),
        recorded.hang_id
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_entry_fails_construct_as_not_supported_yet() {
    let error = construct(
        json!({ "command": "npx", "args": ["-y", "server"], "env": { "KEY": SECRET } }),
        &runtime_services(),
    )
    .err()
    .expect("a local entry is refused");
    assert_eq!(
        error.to_string(),
        "local MCP servers (command) are not supported yet"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_runtime_service_fails_construct_naming_it() {
    let error = construct(
        json!({ "url": "http://127.0.0.1:1/mcp" }),
        &HostServices::new(),
    )
    .err()
    .expect("no runtime is a failure");
    assert!(
        error.to_string().contains("promptforge/tokio-runtime"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejecting_server_fails_ready_with_a_reason_that_holds_no_header_value() {
    let fixture = Fixture::rejecting().await;
    let plugin = install(
        &fixture,
        &json!({ "Authorization": format!("Bearer {SECRET}") }),
    );

    let reason = tokio::time::timeout(Duration::from_secs(30), plugin.ready())
        .await
        .expect("a rejection settles")
        .expect_err("a 401 fails the start");
    assert!(!reason.to_string().contains(SECRET), "{reason}");
    assert!(plugin.tools().is_empty());
    let refused = call(&plugin, "remote/echo", json!({}))
        .await
        .expect_err("no call goes out");
    assert!(!refused.to_string().contains(SECRET));
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_plugin_ends_the_session_and_stops_the_connection_task() {
    let fixture = Fixture::start().await;
    let plugin = install(&fixture, &json!({}));
    ready(&plugin).await;
    drop(plugin);

    tokio::time::timeout(Duration::from_secs(30), async {
        while fixture.shared.recorded.lock().expect("lock").deletes == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("rmcp's worker ends the session when the service drops");
}
