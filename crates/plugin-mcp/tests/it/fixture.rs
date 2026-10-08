//! A hand-written Streamable HTTP MCP server for the integration tests.
//!
//! It answers JSON-RPC requests in JSON, issues a session id, answers
//! notifications with `202 Accepted`, answers `GET` with `405` as GitHub's
//! remote server does, and records what it receives.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// One request the fixture received: its method and its headers, with
/// header names in lowercase.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub(crate) method: String,
    pub(crate) headers: HashMap<String, String>,
}

/// Everything the fixture recorded.
#[derive(Debug, Default)]
pub(crate) struct Recorded {
    /// The `params` of each `initialize` request.
    pub(crate) initializes: Vec<Value>,
    /// Every JSON-RPC message, requests and notifications, in order.
    pub(crate) seen: Vec<Seen>,
    /// The `params` of each `notifications/cancelled`.
    pub(crate) cancelled: Vec<Value>,
    /// The JSON-RPC id of the `hang` call.
    pub(crate) hang_id: Option<Value>,
    /// How many `DELETE` requests ended a session.
    pub(crate) deletes: usize,
}

pub(crate) struct Shared {
    pub(crate) recorded: Mutex<Recorded>,
    /// Signalled when the `hang` tool's call arrives.
    pub(crate) hang_started: Notify,
    /// Signalled when a `notifications/cancelled` arrives.
    pub(crate) cancel_arrived: Notify,
    /// Answers every `POST` with `401` when set.
    reject: bool,
}

pub(crate) struct Fixture {
    pub(crate) url: String,
    pub(crate) shared: Arc<Shared>,
    server: JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    /// Starts a server that accepts connections.
    pub(crate) async fn start() -> Fixture {
        Fixture::spawn(false).await
    }

    /// Starts a server that answers every `POST` with `401`.
    pub(crate) async fn rejecting() -> Fixture {
        Fixture::spawn(true).await
    }

    async fn spawn(reject: bool) -> Fixture {
        let shared = Arc::new(Shared {
            recorded: Mutex::new(Recorded::default()),
            hang_started: Notify::new(),
            cancel_arrived: Notify::new(),
            reject,
        });
        let app = Router::new()
            .route("/mcp", post(post_mcp).get(get_mcp).delete(delete_mcp))
            .with_state(Arc::clone(&shared));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let url = format!(
            "http://{}/mcp",
            listener.local_addr().expect("has an address")
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serves");
        });
        Fixture {
            url,
            shared,
            server,
        }
    }

    /// The methods received, in order.
    pub(crate) fn methods(&self) -> Vec<String> {
        let recorded = self.shared.recorded.lock().expect("lock");
        recorded.seen.iter().map(|s| s.method.clone()).collect()
    }
}

async fn get_mcp() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn delete_mcp(State(shared): State<Arc<Shared>>) -> StatusCode {
    shared.recorded.lock().expect("lock").deletes += 1;
    StatusCode::OK
}

fn lowercase_headers(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect()
}

fn rpc(id: &Value, result: Value) -> Value {
    let mut message = json!({ "jsonrpc": "2.0", "id": id });
    message["result"] = result;
    message
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn json_response(body: &Value) -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// The tools the fixture lists, in two pages.
fn tool(name: &str, description: &str) -> Value {
    json!({ "name": name, "description": description, "inputSchema": { "type": "object" } })
}

async fn post_mcp(State(shared): State<Arc<Shared>>, headers: HeaderMap, body: Bytes) -> Response {
    if shared.reject {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let message: Value = serde_json::from_slice(&body).expect("a JSON-RPC message");
    let method = message["method"].as_str().unwrap_or("").to_owned();
    {
        let mut recorded = shared.recorded.lock().expect("lock");
        recorded.seen.push(Seen {
            method: method.clone(),
            headers: lowercase_headers(&headers),
        });
    }
    let Some(id) = message.get("id").filter(|_| !method.is_empty()) else {
        // A notification, or a response to a server request: accepted, no body.
        if method == "notifications/cancelled" {
            shared
                .recorded
                .lock()
                .expect("lock")
                .cancelled
                .push(message["params"].clone());
            shared.cancel_arrived.notify_one();
        }
        return StatusCode::ACCEPTED.into_response();
    };
    match method.as_str() {
        "initialize" => {
            shared
                .recorded
                .lock()
                .expect("lock")
                .initializes
                .push(message["params"].clone());
            let mut response = json_response(&rpc(
                id,
                json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "fixture", "version": "1" }
                }),
            ));
            response.headers_mut().insert(
                "mcp-session-id",
                HeaderValue::from_static("fixture-session"),
            );
            response
        }
        "tools/list" => {
            let page = match message["params"]["cursor"].as_str() {
                None => json!({
                    "tools": [tool("Echo", "Echoes its arguments."), tool("fail", "Always fails.")],
                    "nextCursor": "page-two"
                }),
                Some(_) => json!({ "tools": [tool("hang", "Never answers.")] }),
            };
            json_response(&rpc(id, page))
        }
        "tools/call" => call(&shared, id, &message["params"]).await,
        "ping" => json_response(&rpc(id, json!({}))),
        other => json_response(&rpc_error(
            id,
            -32601,
            &format!("{other} is not implemented"),
        )),
    }
}

async fn call(shared: &Shared, id: &Value, params: &Value) -> Response {
    let text = |text: String| json!({ "content": [{ "type": "text", "text": text }] });
    match params["name"].as_str() {
        Some("Echo") => json_response(&rpc(id, text(format!("echo: {}", params["arguments"])))),
        Some("fail") => json_response(&rpc(
            id,
            json!({ "isError": true, "content": [{ "type": "text", "text": "it failed" }] }),
        )),
        Some("hang") => {
            shared.recorded.lock().expect("lock").hang_id = Some(id.clone());
            shared.hang_started.notify_one();
            std::future::pending::<()>().await;
            unreachable!("a hang call never answers")
        }
        _ => json_response(&rpc_error(id, -32602, "unknown tool")),
    }
}
