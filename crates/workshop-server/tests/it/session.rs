//! Characterization tests for the `/ws` workshop socket: the boot
//! snapshots (status, catalog, workbench), the unsolicited status frames
//! riding the socket, and the Model-menu events, pinned end to end.
//!
//! The root holds the shared harness - mock gateways, the server fixture,
//! frame readers - and each child module pins one behavior area of the
//! socket.

// clippy.toml's allow-expect-in-tests covers #[test] functions only, not
// the helpers they share; failing a test by panicking with the invariant
// named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

mod menu;
mod status;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt as _;
use tokio_tungstenite::tungstenite;

use workshop_server::{
    AgentsConfig, AppState, Config, GatewayConfig, ServerConfig, router,
};

const CATALOG: &str =
    r#"{"object":"list","data":[{"id":"test-model","object":"model","owned_by":"promptforge"}]}"#;

/// A mock `/health` whose answer flips under test control.
async fn flippable_health(State(healthy): State<Arc<AtomicBool>>) -> Response {
    if healthy.load(Ordering::Relaxed) {
        StatusCode::OK.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

/// A static mock catalog for the reconnect push test.
async fn mock_models() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], CATALOG).into_response()
}

/// Binds the workshop router against the gateway at `base_url` on a
/// free loopback port and returns the `/ws` URL, the tempdir keeping
/// the state directory alive, and a handle on the shared state (for
/// poking the status and catalog buses directly).
async fn spawn_session_server(base_url: &str) -> (String, tempfile::TempDir, AppState) {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let config = Config {
        gateway: GatewayConfig {
            base_url: base_url.to_string(),
            api_key: "test-key".to_string(),
        },
        server: ServerConfig {
            state_dir: state_dir.path().to_path_buf(),
            ..ServerConfig::default()
        },
        agents: AgentsConfig::default(),
    };
    let state = AppState::new(&config).expect("state builds in tests");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the session test server");
    let addr = listener.local_addr().expect("session test server address");
    let served = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, router(served))
            .await
            .expect("session test server serves");
    });
    (format!("ws://{addr}/ws"), state_dir, state)
}

/// Reads one text frame from the client socket and parses it as JSON.
async fn read_frame<S>(socket: &mut S) -> serde_json::Value
where
    S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    let message = socket
        .next()
        .await
        .expect("a frame follows")
        .expect("the frame is not a socket error");
    let text = message.into_text().expect("the frame is text");
    serde_json::from_str(&text).expect("the frame is JSON")
}

/// Reads frames until one arrives that is not a status update. Status
/// frames are unsolicited - the snapshot on connect, then bus pushes
/// that may interleave with replies at any point - so reply assertions
/// skip them.
async fn read_non_status_frame<S>(socket: &mut S) -> serde_json::Value
where
    S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    loop {
        let frame = read_frame(socket).await;
        if frame["type"] != "status" {
            return frame;
        }
    }
}

/// Reads frames until `accept` holds, within a generous deadline,
/// returning every frame read - the accepted one last.
async fn frames_until<S>(
    socket: &mut S,
    accept: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value>
where
    S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut frames = Vec::new();
        loop {
            let frame = read_frame(socket).await;
            let found = accept(&frame);
            frames.push(frame);
            if found {
                return frames;
            }
        }
    })
    .await
    .expect("the expected frame arrives within the deadline")
}
