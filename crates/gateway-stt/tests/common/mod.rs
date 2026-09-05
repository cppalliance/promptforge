//! Shared live-server helpers for STT integration tests.

#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gateway_stt::{SttRuntime, SttState};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub(crate) const RECV_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct TestServer {
    url: String,
    task: tokio::task::JoinHandle<()>,
    _runtime: Option<SttRuntime>,
}

impl TestServer {
    pub(crate) fn spawn() -> Self {
        Self::spawn_with(SttState::default(), None)
    }

    pub(crate) fn spawn_with(state: SttState, runtime: Option<SttRuntime>) -> Self {
        let std_listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("gateway listener binds");
        std_listener
            .set_nonblocking(true)
            .expect("gateway listener becomes nonblocking");
        let address = std_listener
            .local_addr()
            .expect("gateway listener has an address");
        let listener =
            tokio::net::TcpListener::from_std(std_listener).expect("tokio adopts the listener");
        let app = gateway_stt::gateway_routes(state);
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("gateway STT fixture serves");
        });
        Self {
            url: format!("http://{address}"),
            task,
            _runtime: runtime,
        }
    }

    pub(crate) fn ws_url(&self, path: &str) -> String {
        format!(
            "ws{}{}",
            self.url.strip_prefix("http").expect("server URL is http"),
            path
        )
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
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
