//! Fake OpenAI and Brave backends: canned and streamed replies, a
//! request-recording backend, and a rendezvous-gated slow backend.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, Method, header::AUTHORIZATION};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

/// Spawns a plain axum backend on an ephemeral port and returns its address.
pub(crate) async fn spawn_backend(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    addr
}

/// A fake OpenAI backend that echoes the model and returns a canned reply,
/// speaking SSE when the request asks to stream and JSON otherwise.
pub(crate) async fn fake_backend() -> SocketAddr {
    async fn completions(Json(body): Json<Value>) -> axum::response::Response {
        use axum::response::IntoResponse;
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if body.get("stream").and_then(Value::as_bool) == Some(true) {
            return (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                canned_sse_reply(&model),
            )
                .into_response();
        }
        Json(canned_reply(&model)).into_response()
    }
    spawn_backend(Router::new().route("/chat/completions", post(completions))).await
}

pub(crate) fn canned_reply(model: &str) -> Value {
    serde_json::json!({
        "id": "cmpl-test",
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "pong" },
            "finish_reason": "stop"
        }]
    })
}

/// The streamed form of [`canned_reply`]: two content chunks, the finish
/// chunk, and the `[DONE]` sentinel.
pub(crate) fn canned_sse_reply(model: &str) -> String {
    let chunk = |delta: Value, finish: Value| {
        serde_json::json!({
            "id": "cmpl-test",
            "object": "chat.completion.chunk",
            "model": model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }]
        })
    };
    let events = [
        chunk(serde_json::json!({ "content": "po" }), Value::Null),
        chunk(serde_json::json!({ "content": "ng" }), Value::Null),
        chunk(serde_json::json!({}), Value::String("stop".to_owned())),
    ];
    let mut body = String::new();
    for event in &events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// One request as observed by the recording backend (IT-005/006).
#[derive(Clone, Debug)]
pub(crate) struct RecordedRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) authorization: Option<String>,
    pub(crate) body: Value,
}

/// Shared, thread-safe log of requests the backend received.
pub(crate) type Recorder = Arc<Mutex<Vec<RecordedRequest>>>;

/// A fake OpenAI backend that validates and records each request it receives,
/// then returns the canned reply. The recorder lets a test assert exactly what
/// the gateway forwarded (method, path, bearer, rewritten model, messages).
pub(crate) async fn recording_backend() -> (SocketAddr, Recorder) {
    async fn completions(
        State(recorder): State<Recorder>,
        method: Method,
        uri: axum::http::Uri,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let authorization = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        recorder.lock().unwrap().push(RecordedRequest {
            method: method.to_string(),
            path: uri.path().to_string(),
            authorization,
            body: body.clone(),
        });
        Json(canned_reply(&model))
    }

    let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/chat/completions", post(completions))
        .with_state(Arc::clone(&recorder));
    (spawn_backend(router).await, recorder)
}

/// A fake Brave Search backend returning five hits on two hosts.
#[cfg(feature = "web-search")]
pub(crate) async fn fake_brave() -> SocketAddr {
    async fn search() -> Json<Value> {
        Json(serde_json::json!({
            "web": {
                "results": [
                    { "title": "A1", "url": "https://a.com/1", "description": "first a", "age": "1 day ago", "extra_snippets": ["snippet a1"] },
                    { "title": "A2", "url": "https://a.com/2", "description": "second a", "extra_snippets": ["snippet a2"] },
                    { "title": "A3", "url": "https://a.com/3", "description": "third a" },
                    { "title": "B1", "url": "https://b.com/1", "description": "first b", "extra_snippets": ["snippet b1"] },
                    { "title": "B2", "url": "https://b.com/2", "description": "second b" }
                ]
            }
        }))
    }
    spawn_backend(Router::new().route("/web/search", axum::routing::get(search))).await
}

/// Release handle handed back by the slow backend when a request arrives.
pub(crate) type ReleaseTx = oneshot::Sender<()>;

/// A fake backend that, on each arrival, hands the test a release handle and
/// blocks until it is fired. No sleeps: arrival and release are rendezvous.
async fn completions_slow(
    State(arrivals): State<UnboundedSender<ReleaseTx>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let (release, released) = oneshot::channel();
    let _ = arrivals.send(release);
    let _ = released.await;
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Json(canned_reply(&model))
}

pub(crate) async fn slow_fake_backend() -> (SocketAddr, UnboundedReceiver<ReleaseTx>) {
    let (arrivals, receiver) = mpsc::unbounded_channel::<ReleaseTx>();
    let router = Router::new()
        .route("/chat/completions", post(completions_slow))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}
