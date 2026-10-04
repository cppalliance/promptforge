//! Emulated Gemma3 `tool_code` dialect: guide injection, tool stripping, and
//! fence-to-tool-call rewriting, streamed or not.

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;

use super::text_within;
use crate::support::{
    TestServer, canned_reply, chat_body, json_within, send_within, spawn_backend,
};

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
config-version = 0

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

/// A malformed `tool_code` fence still completes the turn: the content is
/// emptied so the fence never reaches the client as final text, and a
/// `gateway_warning` field reports the reason.
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

/// A `stream: true` request to a `gemma3_tool_code` model still gets tool
/// calling: the gateway buffers one non-streaming upstream round trip
/// (stripping `stream` and `stream_options`, injecting the guide), rewrites
/// the fence, and re-emits the result as SSE chunks whose delta contains
/// the indexed tool calls, closed by `[DONE]`.
#[tokio::test]
async fn gemma_stream_true_emulates_tool_calls_over_sse() {
    let reply = gemma_reply_with_content("```tool_code\nsearch(query=\"a\")\n```");
    let (gateway, recorder) = gemma_gateway(reply).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{ "role": "user", "content": "ping" }],
                "stream": true,
                "stream_options": { "include_usage": true },
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
                }]
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

    // The upstream saw one buffered request: no stream flag, no
    // stream_options, no tools, and the guide prepended.
    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one buffered request");
    let upstream_body = &seen[0].body;
    assert!(
        upstream_body.get("stream").and_then(Value::as_bool) != Some(true),
        "the upstream call is buffered: {upstream_body}"
    );
    assert!(
        upstream_body.get("stream_options").is_none(),
        "streaming-only options are stripped: {upstream_body}"
    );
    assert!(upstream_body.get("tools").is_none());
    let guide = upstream_body
        .pointer("/messages/0/content")
        .and_then(Value::as_str)
        .expect("guide content");
    assert!(guide.contains("tool_code"), "guide injected: {guide}");

    let body = text_within(response).await;
    let data_lines: Vec<&str> = body
        .lines()
        .filter(|line| line.starts_with("data:"))
        .collect();
    assert_eq!(
        data_lines.last().copied(),
        Some("data: [DONE]"),
        "the synthetic stream ends with the sentinel: {body}"
    );
    let chunk: Value = serde_json::from_str(data_lines[0].strip_prefix("data: ").unwrap()).unwrap();
    assert_eq!(
        chunk.get("model").and_then(Value::as_str),
        Some("test-model")
    );
    let choice = &chunk["choices"][0];
    assert_eq!(
        choice.get("finish_reason").and_then(Value::as_str),
        Some("tool_calls")
    );
    let calls = choice
        .pointer("/delta/tool_calls")
        .and_then(Value::as_array)
        .expect("the delta includes tool calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].get("index").and_then(Value::as_u64),
        Some(0),
        "delta tool-call entries include the fragment index"
    );
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
    gateway.shutdown().await;
}
