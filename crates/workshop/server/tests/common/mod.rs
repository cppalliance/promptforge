//! Shared helpers for the workshop server integration tests: an in-process
//! spawn fixture over [`workshop_server::spawn`], a router-level fixture
//! over composed state, the SSE echo and typed catalog mock gateways
//! serve, a typed JSON WebSocket client over tokio-tungstenite, and the
//! `/agents/ws` connect-and-launch pair.

// clippy.toml's allow-expect-in-tests covers #[test] functions and
// #[cfg(test)] modules only, not integration-test helpers; failing a test
// by panicking with the invariant named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::path::Path;
use std::time::Duration;

use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use workshop_server::fixtures::state_with_gateway;
use workshop_server::{
    AgentsConfig, AppState, Config, GatewayConfig, ResolvedGateway, ServerConfig, ServerHandle,
    router,
};

/// How long one frame read may take before the test fails: generous enough
/// for a slow CI runner, far below any test's own deadline.
pub(crate) const RECV_TIMEOUT: Duration = Duration::from_secs(10);

/// A workshop server spawned in-process for one test, anchoring its state
/// in a tempdir that lives as long as the fixture.
///
/// Dropping the fixture shuts the server down and waits for its thread, so
/// every state file is closed before the tempdir is deleted.
pub(crate) struct TestServer {
    handle: Option<ServerHandle>,
    /// Held as an `Option` only so [`TestServer::shutdown_keeping_state_dir`]
    /// can move it out past the `Drop` impl; it is `Some` until then.
    state_dir: Option<tempfile::TempDir>,
}

impl TestServer {
    /// Spawns the server against the gateway at `gateway_base_url` over a
    /// fresh state directory. Discovery is bypassed: a test never
    /// consults the real run directory.
    pub(crate) fn spawn(gateway_base_url: &str) -> Self {
        Self::spawn_in(gateway_base_url, tempfile::TempDir::new().expect("tempdir"))
    }

    /// Spawns the server against the gateway at `gateway_base_url` over
    /// `state_dir`, which the fixture then owns: the relaunch seam, fed
    /// by [`TestServer::shutdown_keeping_state_dir`] of a previous
    /// server.
    pub(crate) fn spawn_in(gateway_base_url: &str, state_dir: tempfile::TempDir) -> Self {
        let config = Config {
            gateway: GatewayConfig {
                base_url: gateway_base_url.to_string(),
                api_key: "test-key".to_string(),
            },
            server: ServerConfig {
                bind: "127.0.0.1:0".to_string(),
                state_dir: state_dir.path().to_path_buf(),
            },
            agents: AgentsConfig::default(),
        };
        let handle = workshop_server::fixtures::spawn(config).expect("the workshop server spawns");
        Self {
            handle: Some(handle),
            state_dir: Some(state_dir),
        }
    }

    /// The state directory this server persists into.
    pub(crate) fn state_dir(&self) -> &std::path::Path {
        self.state_dir
            .as_ref()
            .expect("the state dir is held until shutdown")
            .path()
    }

    /// Shuts the server down, waits for its thread, and hands back the
    /// state directory intact so a second server can boot over it.
    pub(crate) fn shutdown_keeping_state_dir(mut self) -> tempfile::TempDir {
        if let Some(handle) = self.handle.take() {
            handle.shutdown().expect("the server shuts down cleanly");
        }
        self.state_dir
            .take()
            .expect("the state dir is held until shutdown")
    }

    /// The `ws://` URL of `path` on this server, for example `/ws` or
    /// `/v1/realtime`.
    pub(crate) fn ws_url(&self, path: &str) -> String {
        let url = self
            .handle
            .as_ref()
            .expect("the handle is held until drop")
            .url();
        let rest = url
            .strip_prefix("http")
            .expect("the server URL scheme is http");
        format!("ws{rest}{path}")
    }

    /// The `http://` URL of `path` on this server.
    pub(crate) fn http_url(&self, path: &str) -> String {
        format!(
            "{}{path}",
            self.handle
                .as_ref()
                .expect("the handle is held until drop")
                .url()
        )
    }

    /// Atomically replaces the local sidecar endpoint and bearer used by
    /// every gateway-dependent Workshop path.
    pub(crate) fn replace_gateway(&self, gateway_base_url: &str, api_key: &str) {
        let updater = self
            .handle
            .as_ref()
            .expect("the handle is held until drop")
            .gateway_updater();
        workshop_server::fixtures::replace_gateway(&updater, gateway_base_url, api_key)
            .expect("the replacement endpoint publishes");
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // A failed shutdown must not double-panic a failing test.
            let _ = handle.shutdown();
        }
    }
}

// The crate's own fixture, shared here instead of duplicated: binds a mock
// gateway on a free loopback port and returns its base URL.
pub(crate) use workshop_server::fixtures::spawn_gateway;

/// The configuration a router-level test composes state from: the
/// gateway at `gateway_base_url` under the fixture bearer, state in
/// `state_dir`, and the default agents directory.
pub(crate) fn test_config(gateway_base_url: &str, state_dir: &Path) -> Config {
    Config {
        gateway: GatewayConfig {
            base_url: gateway_base_url.to_string(),
            api_key: "test-key".to_string(),
        },
        server: ServerConfig {
            state_dir: state_dir.to_path_buf(),
            ..ServerConfig::default()
        },
        agents: AgentsConfig::default(),
    }
}

/// Composes state from `config` and binds the full workshop router over
/// it, returning the state and the server's `ws://` base URL. Discovery
/// is bypassed: a test never consults the real run directory. The
/// router is bound without the serving loop, so no registered task runs
/// unless the test spawns it.
pub(crate) async fn spawn_router(config: &Config) -> (AppState, String) {
    let gateway = ResolvedGateway::from_config(&config.gateway);
    let state = state_with_gateway(config, &gateway).expect("state builds in tests");
    let base = serve_router(&state).await;
    (state, base)
}

/// Binds the full workshop router over `state` on a free loopback port
/// and returns the server's `ws://` base URL.
pub(crate) async fn serve_router(state: &AppState) -> String {
    let (addr, _handle) = workshop_support::fixtures::serve(router(state.clone())).await;
    format!("ws://{addr}")
}

/// One SSE data line holding `event`.
pub(crate) fn sse_line(event: &serde_json::Value) -> String {
    format!("data: {event}\n\n")
}

/// One streaming chunk in OpenAI's format, attributed to `model`.
pub(crate) fn sse_chunk(
    model: &str,
    delta: &serde_json::Value,
    finish: &serde_json::Value,
) -> serde_json::Value {
    json!({
        "model": model,
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
    })
}

/// Streams `echo:<text>` as an SSE completion attributed to `model`: a
/// reasoning chunk, the content split across two chunks, the finish
/// chunk, and the `[DONE]` sentinel - so a turn provably yields multiple
/// live deltas.
pub(crate) fn echo_stream(model: &str, text: &str) -> Response {
    let null = serde_json::Value::Null;
    let reply = format!("echo:{text}");
    let (first, second) = reply.split_at(reply.len() / 2);
    let mut sse = String::new();
    for event in [
        sse_chunk(model, &json!({ "role": "assistant" }), &null),
        sse_chunk(model, &json!({ "reasoning_content": "mm" }), &null),
        sse_chunk(model, &json!({ "content": first }), &null),
        sse_chunk(model, &json!({ "content": second }), &null),
        sse_chunk(model, &json!({}), &json!("stop")),
    ] {
        sse.push_str(&sse_line(&event));
    }
    sse.push_str("data: [DONE]\n\n");
    ([(header::CONTENT_TYPE, "text/event-stream")], sse).into_response()
}

/// The typed `/v1/models` route a launch resolves the menu selection
/// through, listing `ids` in order. Every entry is a chat model whose
/// window clears the built-in chat's declared minimum, so a test pins
/// the host wiring rather than a refused binding.
pub(crate) fn typed_catalog(ids: &'static [&'static str]) -> MethodRouter {
    get(move || async move {
        let data: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| {
                json!({
                    "id": id, "object": "model", "kind": "chat", "description": id,
                    "context": 200_000, "thinking": "never",
                })
            })
            .collect();
        axum::Json(json!({ "object": "list", "data": data }))
    })
}

/// A typed JSON WebSocket client: JSON and control frames out, JSON frames
/// in, every receive bounded by a timeout.
pub(crate) struct JsonSocket {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl JsonSocket {
    /// Connects to a `ws://` URL.
    pub(crate) async fn connect(url: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("the WebSocket connects");
        Self { socket }
    }

    /// Sends one JSON value as a text frame.
    pub(crate) async fn send_json(&mut self, value: &serde_json::Value) {
        self.send_text(&value.to_string()).await;
    }

    /// Sends one raw text frame, for control messages and malformed input.
    pub(crate) async fn send_text(&mut self, text: &str) {
        self.socket
            .send(Message::Text(text.to_string().into()))
            .await
            .expect("the text frame is sent");
    }

    /// Receives the next text frame and parses it as JSON, failing after
    /// [`RECV_TIMEOUT`].
    pub(crate) async fn recv_json(&mut self) -> serde_json::Value {
        self.recv_json_within(RECV_TIMEOUT).await
    }

    /// Receives the next text frame and parses it as JSON, failing after
    /// `deadline`.
    pub(crate) async fn recv_json_within(&mut self, deadline: Duration) -> serde_json::Value {
        let message = tokio::time::timeout(deadline, self.socket.next())
            .await
            .expect("a frame arrives within the deadline")
            .expect("the socket is open")
            .expect("the frame is not a socket error");
        let text = message.into_text().expect("the frame is text");
        serde_json::from_str(&text).expect("the frame is JSON")
    }

    /// Receives frames until `keep` accepts one, failing after `deadline`.
    pub(crate) async fn recv_until(
        &mut self,
        deadline: Duration,
        keep: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        tokio::time::timeout(deadline, async {
            loop {
                let frame = self.recv_json_within(deadline).await;
                if keep(&frame) {
                    break frame;
                }
            }
        })
        .await
        .expect("a matching frame arrives within the deadline")
    }

    /// Closes the socket with the normal close handshake.
    pub(crate) async fn close(mut self) {
        self.socket.close(None).await.expect("the socket closes");
    }
}

/// Connects to `/agents/ws` at `base`, asserting the connect-time push
/// lists exactly `agents`.
pub(crate) async fn connect_agents(base: &str, agents: &[&str]) -> JsonSocket {
    let mut socket = JsonSocket::connect(&format!("{base}/agents/ws")).await;
    assert_eq!(
        socket.recv_json().await,
        json!({ "type": "agents", "agents": agents }),
        "the connect-time push lists the offered agents"
    );
    socket
}

/// Launches `agent` on `socket` and returns the session id from the
/// acknowledgment frame.
pub(crate) async fn launch(socket: &mut JsonSocket, agent: &str) -> String {
    socket
        .send_json(&json!({ "type": "launch", "agent": agent }))
        .await;
    let frame = socket.recv_json().await;
    assert_eq!(
        frame["type"], "agent_session",
        "launch acknowledged: {frame}"
    );
    assert_eq!(frame["agent"], agent);
    frame["session"]
        .as_str()
        .expect("the acknowledgment includes the session id")
        .to_owned()
}
