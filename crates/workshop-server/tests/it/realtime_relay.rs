//! Additive Workshop relay for Gateway Realtime transcription.

#![expect(
    clippy::expect_used,
    reason = "integration-test helpers panic with the failed wire invariant"
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::{Error as SocketError, Message as ClientMessage};

use crate::common::{RECV_TIMEOUT, TestServer, spawn_gateway};

#[derive(Clone, Debug, PartialEq, Eq)]
struct UpstreamRequest {
    path: String,
    query: String,
    has_origin: bool,
    has_subprotocol: bool,
}

#[derive(Clone, Default)]
struct UpstreamProbe {
    request: Arc<Mutex<Option<UpstreamRequest>>>,
    pings: Arc<Mutex<Vec<Vec<u8>>>>,
    pongs: Arc<Mutex<Vec<Vec<u8>>>>,
    browser_close: Arc<Mutex<Option<(u16, String)>>>,
    control_seen: Arc<Notify>,
    close_seen: Arc<Notify>,
    disconnected: Arc<Notify>,
}

#[derive(Clone, Default)]
struct FixtureUpstream {
    frames: Arc<Vec<String>>,
    gateway_bearer_seen: Arc<AtomicBool>,
    browser_bearer_seen: Arc<AtomicBool>,
}

fn canonical_server_frames() -> Vec<String> {
    let fixtures: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../gateway-stt/tests/fixtures/realtime/valid-sequences.json"
    ))
    .expect("canonical Realtime sequences parse");
    [
        "first_event_readiness",
        "hypothesis_negotiation",
        "overlapping_items_reverse_completion",
        "clear_retires_only_uncommitted_input",
        "saturated_commit_retry",
        "engine_replacement",
    ]
    .into_iter()
    .flat_map(|name| {
        fixtures[name]["events"]
            .as_array()
            .expect("canonical sequence has events")
            .iter()
            .filter(|entry| entry["direction"] == "server")
            .map(|entry| {
                serde_json::to_string(&entry["message"]).expect("canonical event serializes")
            })
            .collect::<Vec<_>>()
    })
    .collect()
}

impl UpstreamProbe {
    fn request(&self) -> UpstreamRequest {
        self.request
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("the upstream handshake was recorded")
    }

    fn pings(&self) -> Vec<Vec<u8>> {
        self.pings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn pongs(&self) -> Vec<Vec<u8>> {
        self.pongs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn browser_close(&self) -> Option<(u16, String)> {
        self.browser_close
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

async fn fixture_upstream(
    State(fixture): State<FixtureUpstream>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    fixture
        .gateway_bearer_seen
        .store(authorization == "Bearer test-key", Ordering::Release);
    fixture
        .browser_bearer_seen
        .store(authorization.contains("browser-secret"), Ordering::Release);
    if authorization != "Bearer test-key" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |mut socket| async move {
        for frame in fixture.frames.iter() {
            if socket
                .send(Message::Text(frame.clone().into()))
                .await
                .is_err()
            {
                return;
            }
        }
        while let Some(Ok(message)) = socket.recv().await {
            match message {
                Message::Text(text) => {
                    if socket.send(Message::Text(text)).await.is_err() {
                        return;
                    }
                }
                Message::Binary(bytes) => {
                    if socket.send(Message::Binary(bytes)).await.is_err() {
                        return;
                    }
                }
                Message::Close(_) => return,
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
    })
}

async fn upstream(
    State(probe): State<UpstreamProbe>,
    headers: HeaderMap,
    uri: Uri,
    ws: WebSocketUpgrade,
) -> Response {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let has_origin = headers.contains_key(header::ORIGIN);
    let has_subprotocol = headers.contains_key("sec-websocket-protocol");
    *probe
        .request
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(UpstreamRequest {
        path: uri.path().to_owned(),
        query: uri.query().unwrap_or_default().to_owned(),
        has_origin,
        has_subprotocol,
    });
    if authorization != "Bearer test-key" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |mut socket| async move {
        while let Some(Ok(message)) = socket.recv().await {
            match message {
                Message::Text(text) => {
                    if socket.send(Message::Text(text)).await.is_err() {
                        return;
                    }
                }
                Message::Binary(bytes) => {
                    if socket.send(Message::Binary(bytes)).await.is_err()
                        || socket
                            .send(Message::Ping(vec![9, 8, 7].into()))
                            .await
                            .is_err()
                        || socket
                            .send(Message::Pong(vec![6, 5, 4].into()))
                            .await
                            .is_err()
                    {
                        return;
                    }
                }
                Message::Pong(bytes) => {
                    probe
                        .pongs
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(bytes.to_vec());
                    probe.control_seen.notify_one();
                }
                Message::Close(frame) => {
                    if let Some(frame) = frame {
                        *probe
                            .browser_close
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some((frame.code, frame.reason.to_string()));
                    }
                    probe.close_seen.notify_one();
                    return;
                }
                Message::Ping(bytes) => {
                    probe
                        .pings
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(bytes.to_vec());
                    probe.control_seen.notify_one();
                }
            }
        }
        probe.disconnected.notify_one();
    })
}

async fn spawn_probe() -> (String, UpstreamProbe) {
    let probe = UpstreamProbe::default();
    let app = Router::new()
        .route("/v1/realtime", get(upstream))
        .with_state(probe.clone());
    (spawn_gateway(app).await, probe)
}

async fn recv(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> ClientMessage {
    tokio::time::timeout(RECV_TIMEOUT, socket.next())
        .await
        .expect("a frame arrives before the deadline")
        .expect("the relay socket stays open")
        .expect("the relayed frame is valid")
}

async fn assert_no_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), socket.next())
            .await
            .is_err(),
        "the terminated control frame produces no duplicate or forwarded frame"
    );
}

async fn upstream_close(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|mut socket| async move {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 4101,
                reason: "upstream finished".into(),
            })))
            .await;
    })
}

#[derive(Clone, Default)]
struct StalledPeerProbe {
    frame_sent: Arc<AtomicBool>,
    frame_sent_event: Arc<Notify>,
}

impl StalledPeerProbe {
    async fn wait_for_frame(&self) {
        let notified = self.frame_sent_event.notified();
        if self.frame_sent.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

async fn send_large_frame_then_disconnect(
    State(probe): State<StalledPeerProbe>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        if socket
            .send(Message::Binary(vec![0x5a; 32 * 1024 * 1024].into()))
            .await
            .is_ok()
        {
            probe.frame_sent.store(true, Ordering::Release);
            probe.frame_sent_event.notify_one();
        }
    })
}

async fn recovered_upstream(headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer replacement-key");
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(|mut socket| async move {
        for event in [
            r#"{"type":"session.created","session":{"id":"replacement"}}"#,
            r#"{"type":"session.updated","session":{"include":["item.input_audio_transcription.hypothesis"]}}"#,
        ] {
            if socket.send(Message::Text(event.into())).await.is_err() {
                return;
            }
        }
    })
}

fn request_with(
    url: &str,
    origin: Option<&str>,
    subprotocol: Option<&str>,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = url
        .into_client_request()
        .expect("the WebSocket request builds");
    if let Some(origin) = origin {
        request.headers_mut().insert(
            header::ORIGIN,
            origin.parse().expect("the test Origin is valid"),
        );
    }
    if let Some(subprotocol) = subprotocol {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            subprotocol.parse().expect("the test subprotocol is valid"),
        );
    }
    request
}

async fn rejected_status(request: tokio_tungstenite::tungstenite::http::Request<()>) -> StatusCode {
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("the WebSocket handshake is rejected");
    let SocketError::Http(response) = error else {
        panic!("the rejection is an HTTP response, got {error:?}");
    };
    StatusCode::from_u16(response.status().as_u16()).expect("the status is standard")
}

include!("realtime_relay/authentication.rs");
include!("realtime_relay/protocol.rs");
include!("realtime_relay/lifecycle.rs");
include!("realtime_relay/recovery.rs");
include!("realtime_relay/overload.rs");
include!("realtime_relay/canonical_sequence.rs");
