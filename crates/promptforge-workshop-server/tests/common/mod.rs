//! Shared helpers for the workshop server integration tests: an in-process
//! spawn fixture over [`promptforge_workshop_server::spawn`] and a typed JSON
//! WebSocket client over tokio-tungstenite.

// clippy.toml's allow-expect-in-tests covers #[test] functions and
// #[cfg(test)] modules only, not integration-test helpers; failing a test
// by panicking with the invariant named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use promptforge_workshop_server::{Config, GatewayConfig, ServerConfig, ServerHandle, TapeConfig};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// How long one frame read may take before the test fails: generous enough
/// for a slow CI runner, far below any test's own deadline.
pub(crate) const RECV_TIMEOUT: Duration = Duration::from_secs(10);

/// A workshop server spawned in-process for one test, taping into a
/// tempdir that lives as long as the fixture.
///
/// Dropping the fixture shuts the server down and waits for its thread, so
/// the tape file is closed before the tempdir is deleted.
pub(crate) struct TestServer {
    handle: Option<ServerHandle>,
    tape_dir: tempfile::TempDir,
}

impl TestServer {
    /// Spawns the server against the gateway at `gateway_base_url`.
    pub(crate) fn spawn(gateway_base_url: &str) -> Self {
        let tape_dir = tempfile::TempDir::new().expect("tempdir");
        let config = Config {
            gateway: GatewayConfig {
                base_url: gateway_base_url.to_string(),
                api_key: "test-key".to_string(),
            },
            tape: TapeConfig {
                path: tape_dir.path().join("tape.jsonl"),
            },
            server: ServerConfig {
                bind: "127.0.0.1:0".to_string(),
                open_browser: false,
            },
        };
        let handle =
            promptforge_workshop_server::spawn(config).expect("the workshop server spawns");
        Self {
            handle: Some(handle),
            tape_dir,
        }
    }

    /// The `ws://` URL of `path` on this server, for example `/ws` or
    /// `/voice`.
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

    /// Every event on the server's tape, oldest first.
    pub(crate) fn tape_events(&self) -> Vec<serde_json::Value> {
        let raw = std::fs::read_to_string(self.tape_dir.path().join("tape.jsonl"))
            .expect("the tape exists once the server is spawned");
        raw.lines()
            .map(|line| serde_json::from_str(line).expect("the tape line is valid JSON"))
            .collect()
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
pub(crate) use promptforge_workshop_server::fixtures::spawn_gateway;

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

    /// Receives frames until one arrives whose `type` is neither `status`
    /// nor `workbench`. Both are unsolicited pushes - the heartbeat and
    /// the menu bus interleave them with replies at any point - so reply
    /// assertions skip them.
    pub(crate) async fn recv_non_status(&mut self) -> serde_json::Value {
        loop {
            let frame = self.recv_json().await;
            if frame["type"] != "status" && frame["type"] != "workbench" {
                return frame;
            }
        }
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
