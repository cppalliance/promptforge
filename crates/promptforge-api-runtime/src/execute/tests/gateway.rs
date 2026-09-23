//! The scripted mock gateway and the reply builders the suites script it
//! with.

use super::*;

/// The single configurable mock gateway every execution test uses
/// (EXEC-TESTS-005). It serves a fixed script of chat-completions responses in
/// order, repeating the last entry once the script is exhausted, records every
/// request body it receives, and counts calls. Scripts stay in the buffered
/// chat-completion shape; each is converted to the SSE chunk stream the
/// always-streaming client consumes at serve time (see [`sse_events`]).
///
/// The server is OWNED (EXEC-TESTS-003): the guard holds the bound address, a
/// graceful-shutdown sender, and the serving task's `JoinHandle`. Dropping the
/// guard (at test end) signals shutdown and aborts the task, so no detached
/// server survives the test to `.unwrap()`-panic during runtime teardown. The
/// listener is bound inside [`ScriptedGateway::start`], so a bind failure
/// surfaces in the owning test, not in a detached task.
pub(super) struct ScriptedGateway {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Value>>>,
    pub(super) calls: Arc<AtomicUsize>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: tokio::task::JoinHandle<()>,
}

/// One scripted reply: either a JSON completion body (HTTP 200) or a
/// status-coded error body, so one harness covers success and backend-failure
/// tests alike.
#[derive(Clone)]
pub(super) enum GatewayReply {
    Json(Value),
    Status(u16, String),
    DelayedJson(std::time::Duration, Value),
}

#[derive(Clone)]
pub(super) struct ScriptState {
    responses: Arc<Vec<GatewayReply>>,
    requests: Arc<Mutex<Vec<Value>>>,
    pub(super) calls: Arc<AtomicUsize>,
}

/// Splits `text` at its char midpoint, so a scripted string streams as two
/// fragments and the client's accumulation is actually exercised.
pub(super) fn split_for_stream(text: &str) -> (&str, &str) {
    let mid = text.chars().count() / 2;
    let at = text
        .char_indices()
        .nth(mid)
        .map_or(text.len(), |(index, _)| index);
    text.split_at(at)
}

/// Converts one buffered chat-completion body into the SSE event text a
/// streaming backend would emit for it: reasoning deltas, content split
/// across fragments, tool calls as split argument fragments, the
/// finish-reason chunk, a trailing empty-choices summary chunk when the
/// body includes `usage`/`timings`/`metrics`, and the `[DONE]` sentinel.
pub(super) fn sse_events(body: &Value) -> String {
    let model = body.get("model").cloned();
    let choice = body["choices"].get(0).cloned().unwrap_or_default();
    let message = choice.get("message").cloned().unwrap_or_default();
    let chunk = |delta: Value, finish: Option<&Value>| -> Value {
        let mut chunk_choice = json!({ "index": 0, "delta": delta });
        if let Some(finish) = finish {
            chunk_choice["finish_reason"] = finish.clone();
        }
        let mut event = json!({ "object": "chat.completion.chunk", "choices": [chunk_choice] });
        if let Some(model) = &model {
            event["model"] = model.clone();
        }
        event
    };
    let mut events: Vec<Value> = Vec::new();
    if let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) {
        events.push(chunk(json!({ "reasoning_content": reasoning }), None));
    }
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        let (first, second) = split_for_stream(content);
        for part in [first, second] {
            if !part.is_empty() {
                events.push(chunk(json!({ "content": part }), None));
            }
        }
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (index, call) in calls.iter().enumerate() {
            let arguments = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (first, second) = split_for_stream(arguments);
            let mut opener = json!({
                "index": index,
                "type": "function",
                "function": {
                    "name": call.pointer("/function/name").cloned().unwrap_or(Value::Null),
                    "arguments": first,
                },
            });
            if let Some(id) = call.get("id") {
                opener["id"] = id.clone();
            }
            events.push(chunk(json!({ "tool_calls": [opener] }), None));
            if !second.is_empty() {
                events.push(chunk(
                    json!({ "tool_calls": [{
                        "index": index,
                        "function": { "arguments": second },
                    }] }),
                    None,
                ));
            }
        }
    }
    let finish = choice.get("finish_reason").cloned().unwrap_or(Value::Null);
    events.push(chunk(json!({}), Some(&finish)));
    let mut summary = serde_json::Map::new();
    for key in ["usage", "timings", "metrics"] {
        if let Some(section) = body.get(key).filter(|section| !section.is_null()) {
            summary.insert(key.to_owned(), section.clone());
        }
    }
    if !summary.is_empty() {
        let mut event = json!({ "object": "chat.completion.chunk", "choices": [] });
        if let Some(model) = &model {
            event["model"] = model.clone();
        }
        for (key, value) in summary {
            event[key] = value;
        }
        events.push(event);
    }
    let mut out = String::new();
    for event in &events {
        out.push_str("data: ");
        out.push_str(&event.to_string());
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

/// Renders a scripted body as the SSE response the streaming client expects.
pub(super) fn sse_response(body: &Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        sse_events(body),
    )
        .into_response()
}

/// Validates every replayed `messages[].tool_calls[]` entry against the
/// OpenAI function-call schema, mirroring `parse_openai_tool_calls` (the
/// engine's own inbound parser, `pub(crate)` to `model-client` and so
/// unreachable from here).
///
/// The mock gateway owes the suites a strict endpoint: without this check a
/// neutral-shape replay would pass the mock and fail a real OpenAI or vLLM
/// endpoint, which is exactly the regression this pins. A violation is a
/// diagnostic naming the offending path, never a silent pass.
pub(super) fn assert_openai_tool_calls(body: &Value) -> std::result::Result<(), String> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "request body had no messages array".to_owned())?;
    for (message_index, message) in messages.iter().enumerate() {
        let Some(calls) = message.get("tool_calls").filter(|calls| !calls.is_null()) else {
            continue;
        };
        let calls = calls.as_array().ok_or_else(|| {
            format!("messages[{message_index}].tool_calls was present but not an array")
        })?;
        for (call_index, call) in calls.iter().enumerate() {
            let path = format!("messages[{message_index}].tool_calls[{call_index}]");
            if !call.is_object() {
                return Err(format!("{path} was not an object"));
            }
            match call.get("type") {
                Some(Value::String(kind)) if kind == "function" => {}
                _ => {
                    return Err(format!("{path}.type must be the string \"function\""));
                }
            }
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{path} had no string id"))?;
            if id.trim().is_empty() {
                return Err(format!("{path}.id was blank"));
            }
            let function = call
                .get("function")
                .ok_or_else(|| format!("{path} had no function"))?;
            if !function.is_object() {
                return Err(format!("{path}.function was not an object"));
            }
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{path}.function had no string name"))?;
            if name.trim().is_empty() {
                return Err(format!("{path}.function.name was blank"));
            }
            match function.get("arguments") {
                Some(Value::String(raw)) => {
                    let decoded = serde_json::from_str::<Value>(raw).map_err(|error| {
                        format!("{path}.function.arguments was not valid JSON: {error}")
                    })?;
                    if !decoded.is_object() {
                        return Err(format!(
                            "{path}.function.arguments did not decode to a JSON object"
                        ));
                    }
                }
                None | Some(Value::Null) => {
                    return Err(format!("{path}.function.arguments was missing"));
                }
                Some(_) => {
                    return Err(format!(
                        "{path}.function.arguments was not a JSON-encoded string"
                    ));
                }
            }
        }
    }
    Ok(())
}

impl ScriptedGateway {
    /// Starts a gateway serving `responses` in order (repeating the last).
    pub(super) async fn start(responses: Vec<GatewayReply>) -> ScriptedGateway {
        async fn completions(
            State(state): State<ScriptState>,
            Json(body): Json<Value>,
        ) -> axum::response::Response {
            use axum::response::IntoResponse;
            let n = state.calls.fetch_add(1, Ordering::SeqCst);
            state
                .requests
                .lock()
                .expect("scripted gateway request log must not be poisoned")
                .push(body.clone());
            // A real OpenAI-protocol endpoint rejects a neutral-shape
            // replay with 400; the mock owes the suites the same gate.
            if let Err(diagnostic) = assert_openai_tool_calls(&body) {
                return (StatusCode::BAD_REQUEST, diagnostic).into_response();
            }
            let index = n.min(state.responses.len() - 1);
            match &state.responses[index] {
                GatewayReply::Json(value) => sse_response(value),
                GatewayReply::Status(code, body) => (
                    StatusCode::from_u16(*code).expect("valid test status code"),
                    body.clone(),
                )
                    .into_response(),
                GatewayReply::DelayedJson(delay, value) => {
                    tokio::time::sleep(*delay).await;
                    sse_response(value)
                }
            }
        }

        assert!(
            !responses.is_empty(),
            "a scripted gateway needs at least one response"
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let state = ScriptState {
            responses: Arc::new(responses),
            requests: Arc::clone(&requests),
            calls: Arc::clone(&calls),
        };
        let router = Router::new()
            .route("/v1/chat/completions", post(completions))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("scripted gateway must bind a local port");
        let addr = listener
            .local_addr()
            .expect("scripted gateway must report its local address");
        let (shutdown, rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            // No `.unwrap()`: the serve outcome is swallowed so a torn-down test
            // runtime can never trigger a detached-task panic (EXEC-TESTS-003).
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        ScriptedGateway {
            addr,
            requests,
            calls,
            shutdown: Some(shutdown),
            server,
        }
    }

    /// The bound local address (`127.0.0.1:<port>`).
    pub(super) fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The number of completion requests served so far.
    pub(super) fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// A snapshot of every recorded request body, in arrival order.
    pub(super) fn requests(&self) -> Vec<Value> {
        self.requests
            .lock()
            .expect("scripted gateway request log must not be poisoned")
            .clone()
    }

    /// The most recently recorded request body, if any.
    pub(super) fn last_request(&self) -> Option<Value> {
        self.requests
            .lock()
            .expect("scripted gateway request log must not be poisoned")
            .last()
            .cloned()
    }
}

impl Drop for ScriptedGateway {
    fn drop(&mut self) {
        // Signal graceful shutdown, then abort to guarantee the task ends with
        // the guard rather than outliving the test as a detached server.
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.server.abort();
    }
}

/// A response asking the model to call one tool (OpenAI `tool_calls` shape).
pub(super) fn resp_tool_call(id: &str, name: &str, arguments: &str) -> GatewayReply {
    GatewayReply::Json(json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": arguments }
                }]
            }
        }]
    }))
}

/// A response asking the model to call one tool twice in a single turn.
pub(super) fn resp_two_tool_calls(
    name: &str,
    first: (&str, &str),
    second: (&str, &str),
) -> GatewayReply {
    GatewayReply::Json(json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": first.0,
                        "type": "function",
                        "function": { "name": name, "arguments": first.1 }
                    },
                    {
                        "id": second.0,
                        "type": "function",
                        "function": { "name": name, "arguments": second.1 }
                    }
                ]
            }
        }]
    }))
}

/// A final assistant text reply.
pub(super) fn resp_text(content: &str) -> GatewayReply {
    GatewayReply::Json(json!({
        "choices": [{
            "message": { "role": "assistant", "content": content }
        }]
    }))
}

/// A delayed final assistant text reply for in-flight cancellation tests.
pub(super) fn resp_delayed_text(content: &str, delay: std::time::Duration) -> GatewayReply {
    GatewayReply::DelayedJson(
        delay,
        json!({
            "choices": [{
                "message": { "role": "assistant", "content": content }
            }]
        }),
    )
}

/// A final assistant text reply with an explicit `finish_reason`.
pub(super) fn resp_text_finish(content: &str, finish_reason: &str) -> GatewayReply {
    GatewayReply::Json(json!({
        "choices": [{
            "finish_reason": finish_reason,
            "message": { "role": "assistant", "content": content }
        }]
    }))
}

/// A status-coded error response with a raw body (for backend-failure tests).
pub(super) fn resp_status(code: u16, body: &str) -> GatewayReply {
    GatewayReply::Status(code, body.to_owned())
}

/// The two-round `echo` tool-call-then-text script most loop tests use.
pub(super) fn echo_then_text_script() -> Vec<GatewayReply> {
    vec![
        resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ]
}

/// A tool-call under `alias` on the first round, then a final text reply.
pub(super) fn aliased_tool_script(alias: &str) -> Vec<GatewayReply> {
    vec![
        resp_tool_call("aliased_call", alias, "{\"value\":\"payload\"}"),
        resp_text("aliased final"),
    ]
}
