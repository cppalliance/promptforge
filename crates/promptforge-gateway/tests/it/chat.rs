//! Chat-completions, models-catalog, and health routes through the real client.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::StreamExt as _;
use promptforge_core::client::{GatewayClient, GatewayEndpoint, SecretString};
use promptforge_core::model::CompletionOptions;
use promptforge_gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::support::{
    PHASE_TIMEOUT, ReleaseTx, TestServer, canned_reply, chat_body, fake_backend, gateway_for,
    gateway_with_queue, join_within, json_within, next_arrival, recording_backend, send_within,
    spawn_backend, spawn_chat,
};

#[tokio::test]
async fn happy_path_through_the_real_client() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{}/v1", gateway.addr)).expect("valid test endpoint"),
        SecretString::new("test-token").expect("non-empty test key"),
    );
    let options = CompletionOptions::new("test-model");
    let result = tokio::time::timeout(
        PHASE_TIMEOUT,
        client.complete(
            &[promptforge_core::client::Message::user("ping")],
            None,
            &options,
        ),
    )
    .await
    .expect("client completion exceeded the phase timeout")
    .unwrap();
    match result.result() {
        promptforge_core::client::CompletionResult::Text(reply) => assert_eq!(reply, "pong"),
        other => panic!("expected text reply, got {other:?}"),
    }
    gateway.shutdown().await;
}

/// IT-005/006: the fake backend records the request, so we can assert exactly
/// what the gateway forwarded: method, path, the rewritten upstream model, the
/// intact messages, and that the client's bearer is not leaked upstream.
#[tokio::test]
async fn forwards_method_path_model_and_messages_to_backend() {
    let (backend, recorder) = recording_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&chat_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    let request = &seen[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/chat/completions");
    // The public model name is rewritten to the endpoint's upstream alias.
    assert_eq!(
        request.body.get("model").and_then(Value::as_str),
        Some("backend-model")
    );
    let messages = request
        .body
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages forwarded");
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].get("role").and_then(Value::as_str),
        Some("user")
    );
    assert_eq!(
        messages[0].get("content").and_then(Value::as_str),
        Some("ping")
    );
    // The gateway must not pass the caller's own bearer through to the upstream.
    assert_ne!(
        request.authorization.as_deref(),
        Some("Bearer test-token"),
        "caller bearer must not leak to the upstream"
    );
    gateway.shutdown().await;
}

/// A 200 upstream response that fails shape validation is a protocol error
/// (UP-004): the gateway must not fabricate an upstream status that never
/// happened.
#[tokio::test]
async fn invalid_shape_200_is_upstream_protocol_not_upstream_error() {
    async fn completions() -> Json<Value> {
        Json(serde_json::json!({
            "id": "cmpl-test",
            "object": "chat.completion",
            "model": "backend-model",
            "choices": [{ "index": 0 }]
        }))
    }
    let backend = spawn_backend(Router::new().route("/chat/completions", post(completions))).await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&chat_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 502);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("upstream_protocol")
    );
    gateway.shutdown().await;
}

/// A model configured for a non-chat kind is rejected on the chat route with
/// 400 and `kind_mismatch` before any queue admission or upstream call.
#[tokio::test]
async fn non_chat_kinds_are_rejected_on_the_chat_route() {
    let backend = fake_backend().await;
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "embed-model"
kind = "embedding"
description = "an embedding model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "reranker"
kind = "classifier"
description = "a classifier model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    for model in ["embed-model", "reranker"] {
        let response = send_within(
            reqwest::Client::new()
                .post(format!("http://{}/v1/chat/completions", gateway.addr))
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": model,
                    "messages": [{ "role": "user", "content": "hi" }]
                })),
        )
        .await;
        assert_eq!(response.status().as_u16(), 400, "model {model}");
        let body = json_within(response).await;
        assert_eq!(
            body.pointer("/error/code").and_then(Value::as_str),
            Some("kind_mismatch"),
            "model {model}"
        );
        assert_eq!(
            body.pointer("/error/type").and_then(Value::as_str),
            Some("invalid_request_error"),
            "model {model}"
        );
    }
    gateway.shutdown().await;
}

#[tokio::test]
async fn unknown_model_is_404_with_model_not_found_code() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "nope",
                "messages": [{ "role": "user", "content": "hi" }]
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 404);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("model_not_found")
    );
    assert_eq!(
        body.pointer("/error/type").and_then(Value::as_str),
        Some("invalid_request_error")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn wrong_token_is_401_with_unauthorized_code() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("wrong-token")
            .json(&serde_json::json!({ "model": "test-model", "messages": [] })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 401);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("unauthorized")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn health_needs_no_auth() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response =
        send_within(reqwest::Client::new().get(format!("http://{}/health", gateway.addr))).await;
    assert_eq!(response.status().as_u16(), 200);
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_returns_configured_entries() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    assert_eq!(body.get("object").and_then(Value::as_str), Some("list"));
    let data = body.get("data").and_then(Value::as_array).unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(
        data[0].get("id").and_then(Value::as_str),
        Some("test-model")
    );
    assert_eq!(data[0].get("object").and_then(Value::as_str), Some("model"));
    assert_eq!(data[0].get("kind").and_then(Value::as_str), Some("chat"));
    assert_eq!(
        data[0].get("description").and_then(Value::as_str),
        Some("a test model for integration")
    );
    assert_eq!(data[0].get("context").and_then(Value::as_u64), Some(8192));
    assert_eq!(
        data[0].get("thinking").and_then(Value::as_str),
        Some("never")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_carries_model_kinds() {
    let backend = fake_backend().await;
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "chat-model"
description = "a chat model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "embed-model"
kind = "embedding"
description = "an embedding model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    let data = body.get("data").and_then(Value::as_array).unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(
        data[0].get("id").and_then(Value::as_str),
        Some("chat-model")
    );
    assert_eq!(data[0].get("kind").and_then(Value::as_str), Some("chat"));
    assert_eq!(
        data[1].get("id").and_then(Value::as_str),
        Some("embed-model")
    );
    assert_eq!(
        data[1].get("kind").and_then(Value::as_str),
        Some("embedding")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_includes_capabilities() {
    let backend = fake_backend().await;
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "chat-model"
description = "a chat model"
context = 8192
thinking = "switchable"
upstream = "backend-model"
endpoints = ["fake"]
max_output = 4096
default_temperature = 0.7
images = true
parallel_tool_calls = true
effort_levels = ["low", "high"]
default_effort = "low"
adaptive_thinking = true
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    let data = body.get("data").and_then(Value::as_array).unwrap();
    assert_eq!(data.len(), 1);
    let entry = &data[0];
    assert_eq!(entry.get("max_output").and_then(Value::as_u64), Some(4096));
    assert_eq!(
        entry.get("default_temperature").and_then(Value::as_f64),
        Some(0.7)
    );
    assert_eq!(entry.get("images").and_then(Value::as_bool), Some(true));
    assert_eq!(
        entry.get("parallel_tool_calls").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        entry.get("effort_levels").and_then(Value::as_array),
        Some(&vec![
            Value::String("low".to_owned()),
            Value::String("high".to_owned())
        ])
    );
    assert_eq!(
        entry.get("default_effort").and_then(Value::as_str),
        Some("low")
    );
    assert_eq!(
        entry.get("adaptive_thinking").and_then(Value::as_bool),
        Some(true)
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_omits_unset_optional_capabilities() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    let data = body.get("data").and_then(Value::as_array).unwrap();
    let entry = data[0].as_object().unwrap();
    // Absent options never serialize as null; the flags default to false.
    assert!(!entry.contains_key("max_output"));
    assert!(!entry.contains_key("default_temperature"));
    assert!(!entry.contains_key("default_effort"));
    assert_eq!(entry.get("images").and_then(Value::as_bool), Some(false));
    assert_eq!(
        entry.get("parallel_tool_calls").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        entry.get("adaptive_thinking").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        entry.get("effort_levels").and_then(Value::as_array),
        Some(&vec![])
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_wrong_token_is_401() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("wrong-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 401);
    gateway.shutdown().await;
}

#[test]
fn canned_reply_shapes_a_chat_completion() {
    let reply = canned_reply("m");
    assert_eq!(
        reply.get("object").and_then(Value::as_str),
        Some("chat.completion")
    );
    assert_eq!(reply.get("model").and_then(Value::as_str), Some("m"));
}

/// A gateway whose single chat model is configured for the emulated Gemma3
/// `tool_code` dialect, wired to a backend that records requests and returns
/// `reply` verbatim.
async fn gemma_gateway(reply: Value) -> (TestServer, crate::support::Recorder) {
    async fn completions(
        State((recorder, reply)): State<(crate::support::Recorder, Value)>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        recorder
            .lock()
            .unwrap()
            .push(crate::support::RecordedRequest {
                method: "POST".to_owned(),
                path: "/chat/completions".to_owned(),
                authorization: None,
                body,
            });
        Json(reply)
    }
    let recorder: crate::support::Recorder = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/chat/completions", post(completions))
        .with_state((recorder.clone(), reply));
    let backend = spawn_backend(router).await;
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "test-model"
description = "a test model for integration"
context = 8192
tool_dialect = "gemma3_tool_code"
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    (TestServer::start(gateway).await, recorder)
}

fn gemma_reply_with_content(content: &str) -> Value {
    serde_json::json!({
        "id": "cmpl-test",
        "object": "chat.completion",
        "model": "backend-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }]
    })
}

/// A request with tools to a `gemma3_tool_code` model reaches the backend with
/// the tool-code guide prepended as a system message and the tool surface
/// (`tools`, `tool_choice`) stripped.
#[tokio::test]
async fn gemma_request_with_tools_gets_guide_injected_and_tools_stripped() {
    let (gateway, recorder) = gemma_gateway(canned_reply("backend-model")).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{ "role": "user", "content": "ping" }],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "search",
                        "description": "search the web",
                        "parameters": {
                            "type": "object",
                            "properties": { "query": { "type": "string" } },
                            "required": ["query"]
                        }
                    }
                }],
                "tool_choice": "auto"
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    let body = &seen[0].body;
    assert!(
        body.get("tools").is_none(),
        "tool definitions are stripped: {body}"
    );
    assert!(
        body.get("tool_choice").is_none(),
        "tool_choice is stripped: {body}"
    );
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages forwarded");
    assert_eq!(messages.len(), 2, "guide prepended before the user message");
    assert_eq!(
        messages[0].get("role").and_then(Value::as_str),
        Some("system")
    );
    let guide = messages[0]
        .get("content")
        .and_then(Value::as_str)
        .expect("guide content");
    assert!(
        guide.contains("tool_code"),
        "guide teaches the fence: {guide}"
    );
    assert!(guide.contains("search(query=...)"), "guide: {guide}");
    assert_eq!(
        messages[1].get("content").and_then(Value::as_str),
        Some("ping")
    );
    gateway.shutdown().await;
}

/// A reply whose content is a `tool_code` fence is rewritten into OpenAI
/// `tool_calls` with a null content and a `tool_calls` finish reason.
#[tokio::test]
async fn gemma_response_tool_code_fence_becomes_tool_calls() {
    let reply = gemma_reply_with_content("```tool_code\nsearch(query=\"a\")\n```");
    let (gateway, _recorder) = gemma_gateway(reply).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&chat_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    let choice = &body["choices"][0];
    let message = &choice["message"];
    assert_eq!(message.get("content"), Some(&Value::Null));
    assert_eq!(
        choice.get("finish_reason").and_then(Value::as_str),
        Some("tool_calls")
    );
    let calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .expect("tool_calls present");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].pointer("/function/name").and_then(Value::as_str),
        Some("search")
    );
    assert_eq!(
        calls[0]
            .pointer("/function/arguments")
            .and_then(Value::as_str),
        Some("{\"query\":\"a\"}")
    );
    assert!(message.get("gateway_warning").is_none());
    gateway.shutdown().await;
}

/// A malformed `tool_code` fence never fails the turn and never masquerades
/// as final text: the content is emptied and a `gateway_warning` field carries
/// the reason.
#[tokio::test]
async fn gemma_malformed_fence_yields_empty_content_and_gateway_warning() {
    let reply = gemma_reply_with_content("```tool_code\nsearch(query=bareword)\n```");
    let (gateway, _recorder) = gemma_gateway(reply).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&chat_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    let message = &body["choices"][0]["message"];
    assert_eq!(message.get("content").and_then(Value::as_str), Some(""));
    assert!(
        message
            .get("gateway_warning")
            .and_then(Value::as_str)
            .is_some_and(|warning| !warning.is_empty()),
        "gateway_warning is always present on recovery: {message}"
    );
    assert!(message.get("tool_calls").is_none());
    gateway.shutdown().await;
}

/// One SSE `data:` line carrying an OpenAI streaming chunk.
fn sse_line(model: &str, content: &str) -> String {
    format!(
        "data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]}}\n\n"
    )
}

/// Read a response body to completion, bounded by the phase timeout.
async fn text_within(response: reqwest::Response) -> String {
    tokio::time::timeout(PHASE_TIMEOUT, response.text())
        .await
        .expect("HTTP body read exceeded the phase timeout")
        .expect("HTTP body read failed")
}

/// The mock upstream emits three chunks; the client receives three SSE data
/// lines (each a validated chunk carrying the caller's model name) plus the
/// terminal `data: [DONE]`.
#[tokio::test]
async fn stream_true_relays_typed_chunks_and_done() {
    let body = ["Hel", "lo", "!"]
        .into_iter()
        .map(|part| sse_line("backend-model", part))
        .collect::<String>()
        + "data: [DONE]\n\n";
    let backend = spawn_backend(Router::new().route(
        "/chat/completions",
        post(move || {
            let body = body.clone();
            async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    body,
                )
            }
        }),
    ))
    .await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{ "role": "user", "content": "ping" }],
                "stream": true
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let body = text_within(response).await;
    let data_lines: Vec<&str> = body
        .lines()
        .filter(|line| line.starts_with("data:"))
        .collect();
    assert_eq!(data_lines.len(), 4, "three chunks plus [DONE]: {body}");
    assert_eq!(data_lines[3], "data: [DONE]");
    let mut text = String::new();
    for line in &data_lines[..3] {
        let chunk: Value = serde_json::from_str(line.strip_prefix("data: ").unwrap()).unwrap();
        // The relay re-serializes each chunk with the caller's model name.
        assert_eq!(
            chunk.get("model").and_then(Value::as_str),
            Some("test-model")
        );
        text.push_str(
            chunk
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .unwrap(),
        );
    }
    assert_eq!(text, "Hello!");
    gateway.shutdown().await;
}

/// A backend that gates each request on a release handle: streaming requests
/// get one chunk, then block mid-stream until released; non-streaming
/// requests block, then return the canned reply.
async fn completions_gated(
    State(arrivals): State<UnboundedSender<ReleaseTx>>,
    Json(body): Json<Value>,
) -> Response {
    let (release, released) = oneshot::channel();
    let _ = arrivals.send(release);
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        let first = futures_util::stream::once({
            let model = model.clone();
            async move { Ok::<_, std::convert::Infallible>(sse_line(&model, "po")) }
        });
        let rest = futures_util::stream::once(async move {
            let _ = released.await;
            Ok(format!("{}data: [DONE]\n\n", sse_line(&model, "ng")))
        });
        let mut response = Response::new(axum::body::Body::from_stream(first.chain(rest)));
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/event-stream"),
        );
        response
    } else {
        let _ = released.await;
        Json(canned_reply(&model)).into_response()
    }
}

async fn gated_sse_backend() -> (std::net::SocketAddr, UnboundedReceiver<ReleaseTx>) {
    let (arrivals, receiver) = mpsc::unbounded_channel::<ReleaseTx>();
    let router = Router::new()
        .route("/chat/completions", post(completions_gated))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

/// Under concurrency=1, a streaming request holds the dominion queue permit
/// for the stream's whole lifetime: a second request is not admitted until
/// the first stream has ended.
#[tokio::test]
async fn stream_permit_is_held_until_the_stream_ends() {
    let (backend, mut arrivals) = gated_sse_backend().await;
    let gateway = gateway_with_queue(backend, 1, 10).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/chat/completions", gateway.addr);

    let first = {
        let client = client.clone();
        let url = url.clone();
        tokio::spawn(async move {
            client
                .post(url)
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": "test-model",
                    "messages": [{ "role": "user", "content": "ping" }],
                    "stream": true
                }))
                .send()
                .await
        })
    };
    let release_first = next_arrival(&mut arrivals).await;

    // The second request cannot be admitted while the first stream holds the
    // only concurrency slot.
    let second = spawn_chat(&client, &url);
    assert!(
        matches!(arrivals.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "second request must not reach the backend while the stream is open"
    );

    let first_response = join_within(first).await.unwrap();
    assert_eq!(first_response.status().as_u16(), 200);
    release_first.send(()).unwrap();
    // Drain the body so the relay finishes and releases the permit.
    let body = text_within(first_response).await;
    assert!(body.contains("data: [DONE]"), "stream completed: {body}");

    // After the stream ends, the second is admitted and reaches the backend.
    let release_second = next_arrival(&mut arrivals).await;
    release_second.send(()).unwrap();
    assert_eq!(join_within(second).await.unwrap().status().as_u16(), 200);
    gateway.shutdown().await;
}

/// A client disconnect mid-stream cancels the upstream stream: dropping the
/// response body drops the relay, which drops the gateway's upstream
/// connection, which the backend observes as its own response body being
/// dropped. Drop is the entire mechanism - there is no explicit cancel path.
#[tokio::test]
async fn client_disconnect_aborts_the_upstream_stream() {
    /// Signals once the backend's response body is dropped mid-stream.
    struct NotifyOnDrop(UnboundedSender<()>);
    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    let (dropped, mut observed) = mpsc::unbounded_channel::<()>();
    let backend = spawn_backend(Router::new().route(
        "/chat/completions",
        post(move || {
            let dropped = dropped.clone();
            async move {
                let first = futures_util::stream::once(async {
                    Ok::<_, std::convert::Infallible>(sse_line("backend-model", "po"))
                });
                let rest = futures_util::stream::once(async move {
                    let _notify = NotifyOnDrop(dropped);
                    futures_util::future::pending::<()>().await;
                    unreachable!("the stream never yields a second chunk")
                });
                let mut response = Response::new(axum::body::Body::from_stream(first.chain(rest)));
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    axum::http::HeaderValue::from_static("text/event-stream"),
                );
                response
            }
        }),
    ))
    .await;
    let gateway = gateway_for(backend).await;

    let mut response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{ "role": "user", "content": "ping" }],
                "stream": true
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    // Read the first chunk so the stream is genuinely mid-flight, then hang up.
    let first = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(first.is_some(), "first chunk arrived");
    drop(response);

    tokio::time::timeout(PHASE_TIMEOUT, observed.recv())
        .await
        .expect("backend did not observe the disconnect within the phase timeout")
        .expect("disconnect notification channel closed");
    gateway.shutdown().await;
}

/// An upstream 500 before the stream starts is consumed as a normal JSON
/// error, never an SSE stream that dies mid-flight.
#[tokio::test]
async fn stream_true_upstream_500_is_a_json_error_not_sse() {
    async fn completions() -> (StatusCode, Json<Value>) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "backend exploded" })),
        )
    }
    let backend = spawn_backend(Router::new().route("/chat/completions", post(completions))).await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{ "role": "user", "content": "ping" }],
                "stream": true
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 502);
    assert_ne!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
        "a pre-stream failure is never an SSE response"
    );
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("upstream_error")
    );
    gateway.shutdown().await;
}
