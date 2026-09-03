//! Shared live-server helpers for STT integration tests.

#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gateway_stt::{SttRuntime, SttState};
use workshop_server::{
    AgentsConfig, Config, GatewayConfig, ServerConfig, ServerHandle,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub(crate) const RECV_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct TestServer {
    handle: Option<ServerHandle>,
    _runtime: Option<SttRuntime>,
    _state_dir: tempfile::TempDir,
}

impl TestServer {
    pub(crate) fn spawn() -> Self {
        Self::spawn_with(SttState::default(), None)
    }

    pub(crate) fn spawn_with(state: SttState, runtime: Option<SttRuntime>) -> Self {
        let state_dir = tempfile::TempDir::new().expect("tempdir");
        let config = Config {
            gateway: GatewayConfig {
                base_url: "http://127.0.0.1:1".to_owned(),
                api_key: "test-key".to_owned(),
            },
            server: ServerConfig {
                bind: "127.0.0.1:0".to_owned(),
                open_browser: false,
                state_dir: state_dir.path().to_path_buf(),
            },
            agents: AgentsConfig::default(),
        };
        let route_state = state;
        let handle = workshop_server::spawn_with_routes(config, move |app| {
            gateway_stt::stt_routes(route_state, app.push())
        })
        .expect("workshop server spawns with STT routes");
        Self {
            handle: Some(handle),
            _runtime: runtime,
            _state_dir: state_dir,
        }
    }

    pub(crate) fn ws_url(&self, path: &str) -> String {
        let url = self.handle.as_ref().expect("handle held until drop").url();
        format!(
            "ws{}{}",
            url.strip_prefix("http").expect("server URL is http"),
            path
        )
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown();
        }
    }
}

pub(crate) struct JsonSocket {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl JsonSocket {
    pub(crate) async fn connect(url: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("WebSocket connects");
        Self { socket }
    }

    pub(crate) async fn send_text(&mut self, text: &str) {
        self.socket
            .send(Message::Text(text.to_owned().into()))
            .await
            .expect("text frame sends");
    }

    pub(crate) async fn send_binary(&mut self, bytes: Vec<u8>) {
        self.socket
            .send(Message::Binary(bytes.into()))
            .await
            .expect("binary frame sends");
    }

    pub(crate) async fn recv_json(&mut self) -> serde_json::Value {
        let message = tokio::time::timeout(RECV_TIMEOUT, self.socket.next())
            .await
            .expect("frame arrives before timeout")
            .expect("socket open")
            .expect("frame has no socket error");
        let text = message.into_text().expect("frame is text");
        serde_json::from_str(&text).expect("frame is JSON")
    }

    pub(crate) async fn recv_until(
        &mut self,
        deadline: Duration,
        keep: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        tokio::time::timeout(deadline, async {
            loop {
                let frame = self.recv_json().await;
                if keep(&frame) {
                    break frame;
                }
            }
        })
        .await
        .expect("matching frame arrives before deadline")
    }

    pub(crate) async fn close(mut self) {
        self.socket.close(None).await.expect("socket closes");
    }
}
