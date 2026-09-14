//! The session run's environment and current model: the shared model-free
//! [`Environment`] every session run prepares against, and the launch-time
//! resolution of the dropdown's current model into the per-run context.

use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge_api::client::fetch_model_catalog;
use promptforge_api::{CapabilityRegistry, Environment, Web};
use shared_promptforge_api::models::{ModelDescriptor, ModelId, ThinkingMode};

use super::SessionHost;

/// The context window a selection resolved without catalog metadata
/// records: a conservative default keeps the compactor precheck safe,
/// mirroring the raw-id binding's fallback.
const FALLBACK_CONTEXT: NonZeroU32 = match NonZeroU32::new(8192) {
    Some(value) => value,
    None => unreachable!(),
};

/// Builds the sessions' shared environment for one gateway generation:
/// model-free (the gateway's model list feeds the dropdown UI and never
/// crosses this interface), carrying the first-party capabilities built
/// from the gateway's API root and bearer - today `promptforge/web`. One
/// environment is shared across the runs of one gateway generation and
/// rebuilt when the generation changes, so a replacement gateway's root
/// and key reach the contributed tools.
///
/// Returns `None` - reported like an unusable model client - when the
/// gateway root or key cannot build the capability.
#[must_use]
pub fn session_environment(base_url: &str, api_key: &str) -> Option<Environment> {
    let root = format!("{}/v1", base_url.trim_end_matches('/'));
    let web = match Web::new(&root, api_key) {
        Ok(web) => web,
        Err(error) => {
            tracing::warn!(%error, "agent sessions degraded: the gateway cannot build promptforge/web");
            return None;
        }
    };
    let mut registry = CapabilityRegistry::new();
    if registry.register(Arc::new(web)).is_err() {
        // A single registration cannot collide; the registry's error is
        // defensive on this path.
        return None;
    }
    Some(Environment::new().registry(registry))
}

/// Resolves the dropdown's current model for one run's context. The
/// selection is read at launch, so a selection change takes effect on the
/// next run. A launch with no selection yet - the boot window before the
/// menu's own auto-select settles - binds the retained catalog's first
/// chat-capable model, the same fallback the menu applies. The typed
/// descriptor comes from the gateway's model list through
/// [`fetch_model_catalog`]; when the fetch fails or the selection is
/// absent from it, a minimal descriptor under the fallback context window
/// keeps the run on the selected id, mirroring the raw-id binding's
/// fallback.
///
/// Returns `None` only when neither a selection nor a catalog model
/// exists, or the id is not representable; the prompt's declared roles
/// then stay unbound.
pub(crate) async fn current_model(
    host: &SessionHost,
    base_url: &str,
    api_key: &str,
) -> Option<ModelDescriptor> {
    let selected = host
        .menu()
        .latest()
        .and_then(|snapshot| snapshot.selected_model)
        .or_else(|| {
            host.catalog()
                .latest_chat()?
                .models
                .first()?
                .get("id")?
                .as_str()
                .map(str::to_owned)
        })?;
    let id = match ModelId::gateway(&selected) {
        Ok(id) => id,
        Err(error) => {
            tracing::warn!(%error, "the selected model id is invalid");
            return None;
        }
    };
    let root = format!("{}/v1", base_url.trim_end_matches('/'));
    let fetched = match fetch_model_catalog(&root, api_key).await {
        Ok(catalog) => catalog.get(&id).cloned(),
        Err(error) => {
            tracing::warn!(%error, "the model catalog fetch failed; the selection binds under the fallback descriptor");
            None
        }
    };
    Some(fetched.unwrap_or_else(|| {
        tracing::debug!(model = %selected, "binding the selection under the fallback descriptor");
        ModelDescriptor::new(id, "", FALLBACK_CONTEXT, ThinkingMode::Never)
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use workshop_gateway::GatewayBinding;
    use workshop_menu::{CatalogBus, MenuBus};
    use workshop_registry::Registry;
    use workshop_support::ReconnectBackoff;

    use super::super::AgentSessions;
    use super::*;

    /// A host whose menu and catalog hold one chat-capable model, with
    /// the selection applied only when `selected` is set.
    fn host_with_catalog(selected: bool) -> SessionHost {
        let catalog = CatalogBus::new();
        catalog.publish(vec![
            serde_json::json!({ "id": "test-model", "object": "model" }),
        ]);
        let menu = MenuBus::new(catalog.clone(), None);
        if selected {
            menu.set_selected("test-model")
                .expect("the id is in the catalog");
        }
        SessionHost::new(Registry::new(), ReconnectBackoff::new(), menu, catalog)
    }

    /// Serves the typed catalog entry the fetch resolves through.
    async fn spawn_models_gateway() -> String {
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "object": "list",
                    "data": [{ "id": "test-model", "description": "fetched", "context": 4096, "thinking": "switchable" }],
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the mock gateway binds");
        let addr = listener.local_addr().expect("the mock gateway address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("the mock serves");
        });
        format!("http://{addr}")
    }

    #[test]
    fn the_session_environment_builds_only_from_usable_gateway_settings() {
        assert!(
            session_environment("http://127.0.0.1:1", "k").is_some(),
            "a well-shaped root and key build the environment"
        );
        assert!(
            session_environment("http://127.0.0.1:1", "").is_none(),
            "an empty key cannot authenticate the search proxy"
        );
        assert!(session_environment("not a url", "k").is_none());
    }

    #[tokio::test]
    async fn the_selection_resolves_through_the_fetched_catalog() {
        let base_url = spawn_models_gateway().await;
        let host = host_with_catalog(true);
        let model = current_model(&host, &base_url, "k")
            .await
            .expect("the selection resolves");
        assert_eq!(model.id().name(), "test-model");
        assert_eq!(
            model.context(),
            NonZeroU32::new(4096).expect("4096 is non-zero"),
            "the fetched descriptor wins over the fallback"
        );
        assert_eq!(model.thinking(), ThinkingMode::Switchable);
    }

    #[tokio::test]
    async fn a_failed_catalog_fetch_binds_the_fallback_descriptor() {
        // Port 1 refuses the connection: the fetch fails fast.
        let host = host_with_catalog(true);
        let model = current_model(&host, "http://127.0.0.1:1", "k")
            .await
            .expect("the fallback keeps the selected id");
        assert_eq!(model.id().name(), "test-model");
        assert_eq!(model.context(), FALLBACK_CONTEXT);
        assert_eq!(model.thinking(), ThinkingMode::Never);
    }

    #[tokio::test]
    async fn a_launch_without_a_selection_binds_the_first_catalog_model() {
        let host = host_with_catalog(false);
        let model = current_model(&host, "http://127.0.0.1:1", "k")
            .await
            .expect("the catalog's first model stands in");
        assert_eq!(model.id().name(), "test-model");
    }

    #[tokio::test]
    async fn no_selection_and_no_catalog_means_no_model() {
        let catalog = CatalogBus::new();
        let menu = MenuBus::new(catalog.clone(), None);
        let host = SessionHost::new(Registry::new(), ReconnectBackoff::new(), menu, catalog);
        assert!(
            current_model(&host, "http://127.0.0.1:1", "k")
                .await
                .is_none()
        );
    }

    /// One SSE data line carrying `event`.
    fn sse_line(event: &serde_json::Value) -> String {
        format!("data: {event}\n\n")
    }

    /// The mock's first completion: the model calls the `search` slot.
    fn sse_search_call() -> String {
        let call = serde_json::json!({
            "object": "chat.completion.chunk",
            "model": "test-model",
            "choices": [{ "index": 0, "delta": { "tool_calls": [{
                "index": 0, "id": "call_1", "type": "function",
                "function": { "name": "search", "arguments": "{\"query\":\"promptforge\"}" }
            }] }, "finish_reason": null }],
        });
        let finish = serde_json::json!({
            "object": "chat.completion.chunk",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
        });
        sse_line(&call) + &sse_line(&finish) + "data: [DONE]\n\n"
    }

    /// The mock's later completions: a terminal text reply.
    fn sse_text_reply() -> String {
        let chunk = serde_json::json!({
            "object": "chat.completion.chunk",
            "model": "test-model",
            "choices": [{ "index": 0, "delta": { "content": "found it" }, "finish_reason": null }],
        });
        let finish = serde_json::json!({
            "object": "chat.completion.chunk",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
        });
        sse_line(&chunk) + &sse_line(&finish) + "data: [DONE]\n\n"
    }

    /// Polls until the session's run opens an input wait and returns its token.
    async fn next_wait(sessions: &AgentSessions, id: &str) -> String {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(token) = sessions
                    .unresolved_waits(id)
                    .and_then(|tokens| tokens.first().cloned())
                {
                    return token;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the run opens an input wait")
    }

    /// The mock gateway behind the end-to-end chat session: scripted
    /// completions (a `search` tool call, then text), the search endpoint the
    /// activated capability proxies to, and the typed model catalog the
    /// launch-time selection resolution fetches.
    struct ChatGateway {
        /// The mock's `http://` base URL.
        base_url: String,
        /// Every completion request body, in arrival order.
        completions: Arc<Mutex<Vec<serde_json::Value>>>,
        /// Every search request body plus its Authorization header.
        searches: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    /// Binds the mock gateway on a loopback ephemeral port.
    async fn spawn_chat_gateway() -> ChatGateway {
        use axum::response::IntoResponse;
        use axum::routing::{get, post};

        let completions = Arc::new(Mutex::new(Vec::new()));
        let searches = Arc::new(Mutex::new(Vec::new()));
        let completion_log = Arc::clone(&completions);
        let search_log = Arc::clone(&searches);
        let gateway = axum::Router::new()
            .route(
                "/v1/chat/completions",
                post(move |body: String| {
                    let log = Arc::clone(&completion_log);
                    async move {
                        let body: serde_json::Value =
                            serde_json::from_str(&body).expect("the request is JSON");
                        let call = {
                            let mut log = log.lock().expect("the capture lock is healthy");
                            log.push(body);
                            log.len()
                        };
                        let sse = if call == 1 { sse_search_call() } else { sse_text_reply() };
                        (
                            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                            sse,
                        )
                            .into_response()
                    }
                }),
            )
            .route(
                "/v1/tools/web_search",
                post(move |headers: axum::http::HeaderMap, body: String| {
                    let log = Arc::clone(&search_log);
                    async move {
                        let mut captured: serde_json::Value =
                            serde_json::from_str(&body).expect("the search request is JSON");
                        captured["authorization"] = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_owned()
                            .into();
                        log.lock().expect("the capture lock is healthy").push(captured);
                        axum::Json(serde_json::json!({
                            "results": [{ "url": "https://example.com", "title": "t", "description": "d" }]
                        }))
                        .into_response()
                    }
                }),
            )
            .route(
                "/v1/models",
                get(|| async {
                    axum::Json(serde_json::json!({
                        "object": "list",
                        "data": [{ "id": "test-model", "description": "d", "context": 200_000, "thinking": "never" }],
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the mock gateway binds");
        let addr = listener.local_addr().expect("the mock gateway address");
        tokio::spawn(async move {
            axum::serve(listener, gateway)
                .await
                .expect("the mock serves");
        });
        ChatGateway {
            base_url: format!("http://{addr}"),
            completions,
            searches,
        }
    }

    #[tokio::test]
    async fn a_chat_session_activates_the_web_capability_and_calls_search_end_to_end() {
        let gateway = spawn_chat_gateway().await;
        let catalog = CatalogBus::new();
        catalog.publish(vec![
            serde_json::json!({ "id": "test-model", "object": "model" }),
        ]);
        let menu = MenuBus::new(catalog.clone(), None);
        menu.set_selected("test-model")
            .expect("the id is in the catalog");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let sessions = AgentSessions::new(
            dir.path().join("missing-agents"),
            dir.path().join("sessions"),
            GatewayBinding::new(&gateway.base_url, "test-key").expect("the binding builds"),
            SessionHost::new(Registry::new(), ReconnectBackoff::new(), menu, catalog),
        );
        let session = sessions.launch("chat").expect("the built-in chat launches");

        let token = next_wait(&sessions, &session.id).await;
        session
            .waits
            .complete(&token, "search the web".to_owned())
            .expect("the wait completes");
        // The loop's return to input proves the whole turn settled: the model
        // round, the tool call through the activated capability, and the
        // terminal reply.
        let _settled = next_wait(&sessions, &session.id).await;

        let completions = gateway
            .completions
            .lock()
            .expect("the capture lock is healthy");
        assert_eq!(completions.len(), 2, "the turn is two model rounds");
        assert_eq!(completions[0]["model"], "test-model");
        let advertised: Vec<&str> = completions[0]["tools"]
            .as_array()
            .expect("the filled slots advertise on the wire")
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        assert!(
            advertised.contains(&"search") && advertised.contains(&"fetch"),
            "both slot aliases are advertised: {advertised:?}"
        );
        assert!(
            completions[1]["messages"]
                .as_array()
                .expect("the second round carries the history")
                .iter()
                .any(|message| message["role"] == "tool"),
            "the search result rode back as a tool message"
        );
        let searches = gateway
            .searches
            .lock()
            .expect("the capture lock is healthy");
        assert_eq!(searches.len(), 1, "the capability proxied one search");
        assert_eq!(searches[0]["query"], "promptforge");
        assert_eq!(
            searches[0]["authorization"], "Bearer test-key",
            "the search proxy authenticates with the session's gateway key"
        );

        assert!(sessions.close(&session.id), "the session ends");
    }
}
