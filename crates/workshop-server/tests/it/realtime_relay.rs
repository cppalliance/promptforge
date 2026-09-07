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

#[tokio::test]
async fn canonical_sequences_cross_the_fake_upstream_unchanged_without_browser_bearer() {
    let fixture = FixtureUpstream {
        frames: Arc::new(canonical_server_frames()),
        ..FixtureUpstream::default()
    };
    let gateway = spawn_gateway(
        Router::new()
            .route("/v1/realtime", get(fixture_upstream))
            .with_state(fixture.clone()),
    )
    .await;
    let server = TestServer::spawn(&gateway);
    let url = server.ws_url("/v1/realtime?browser=query");
    let mut request = request_with(&url, None, None);
    request.headers_mut().insert(
        header::AUTHORIZATION,
        "Bearer browser-secret"
            .parse()
            .expect("browser bearer is a header"),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("Workshop fixture relay upgrades");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    for expected in fixture.frames.iter() {
        let ClientMessage::Text(actual) = recv(&mut socket).await else {
            panic!("canonical fixture remains a text payload");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&actual).expect("relayed event parses"),
            serde_json::from_str::<serde_json::Value>(expected).expect("fixture event parses")
        );
    }
    let opaque = "opaque: not JSON, not speech state";
    socket
        .send(ClientMessage::Text(opaque.into()))
        .await
        .expect("opaque browser text sends");
    assert_eq!(recv(&mut socket).await, ClientMessage::Text(opaque.into()));

    assert!(fixture.gateway_bearer_seen.load(Ordering::Acquire));
    assert!(
        !fixture.browser_bearer_seen.load(Ordering::Acquire),
        "the browser bearer never reaches the fake Gateway"
    );
    socket.close(None).await.expect("fixture socket closes");
}

#[tokio::test]
async fn workshop_exposes_only_the_realtime_speech_route() {
    let (gateway, _probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let client = reqwest::Client::new();
    for path in ["/stt", "/stt/capability"] {
        let response = client
            .get(server.http_url(path))
            .send()
            .await
            .expect("the Workshop route answers");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "GET {path} is retired"
        );
    }
}

#[tokio::test]
async fn realtime_relay_is_authenticated_fixed_and_payload_opaque() {
    let (gateway, probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let url = server.ws_url("/v1/realtime?ignored=browser");
    let mut request = request_with(&url, None, None);
    request.headers_mut().insert(
        header::AUTHORIZATION,
        "Bearer browser-secret"
            .parse()
            .expect("the browser credential is a header"),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the Workshop Realtime socket upgrades");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    let opaque = "not JSON: \u{00e9}\u{65e5}\u{1f40d}";
    socket
        .send(ClientMessage::Text(opaque.into()))
        .await
        .expect("opaque text sends");
    assert_eq!(recv(&mut socket).await, ClientMessage::Text(opaque.into()));

    socket
        .send(ClientMessage::Ping(vec![2, 4, 6, 8].into()))
        .await
        .expect("browser ping sends");
    assert_eq!(
        recv(&mut socket).await,
        ClientMessage::Pong(vec![2, 4, 6, 8].into()),
        "the Workshop hop owns exactly one matching browser pong"
    );
    assert_no_frame(&mut socket).await;

    socket
        .send(ClientMessage::Pong(vec![1, 3, 5, 7].into()))
        .await
        .expect("caller-owned pong sends");

    let binary = vec![0, 255, 1, 128, 2];
    socket
        .send(ClientMessage::Binary(binary.clone().into()))
        .await
        .expect("opaque binary sends");
    assert_eq!(
        recv(&mut socket).await,
        ClientMessage::Binary(binary.into())
    );
    tokio::time::timeout(RECV_TIMEOUT, async {
        loop {
            let notified = probe.control_seen.notified();
            if !probe.pongs().is_empty() {
                break;
            }
            notified.await;
        }
    })
    .await
    .expect("the Gateway hop receives its automatic pong");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        probe.pongs(),
        vec![vec![9, 8, 7]],
        "the Gateway hop owns exactly one matching pong and receives no browser pong"
    );
    assert!(
        probe.pings().is_empty(),
        "the browser ping terminates at Workshop"
    );
    assert_no_frame(&mut socket).await;
    socket.close(None).await.expect("the browser socket closes");

    assert_eq!(
        probe.request(),
        UpstreamRequest {
            path: "/v1/realtime".to_owned(),
            query: "intent=transcription".to_owned(),
            has_origin: false,
            has_subprotocol: false,
        },
        "the connector fixes the upstream target and forwards no browser policy headers"
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

#[tokio::test]
async fn gateway_close_code_and_reason_reach_the_browser() {
    let gateway = spawn_gateway(Router::new().route("/v1/realtime", get(upstream_close))).await;
    let server = TestServer::spawn(&gateway);
    let (mut socket, _) =
        tokio_tungstenite::connect_async(server.ws_url("/v1/realtime?intent=transcription"))
            .await
            .expect("the Workshop Realtime socket upgrades");
    let ClientMessage::Close(Some(close)) = recv(&mut socket).await else {
        panic!("the upstream close frame is relayed");
    };
    assert_eq!(u16::from(close.code), 4101);
    assert_eq!(close.reason, "upstream finished");
}

#[tokio::test]
async fn stalled_browser_cleanup_is_bounded_after_gateway_disconnect() {
    let probe = StalledPeerProbe::default();
    let gateway = spawn_gateway(
        Router::new()
            .route("/v1/realtime", get(send_large_frame_then_disconnect))
            .with_state(probe.clone()),
    )
    .await;
    let server = TestServer::spawn(&gateway);
    let (mut socket, _) =
        tokio_tungstenite::connect_async(server.ws_url("/v1/realtime?intent=transcription"))
            .await
            .expect("the Workshop Realtime socket upgrades");
    tokio::time::timeout(RECV_TIMEOUT, probe.wait_for_frame())
        .await
        .expect("the Gateway fills the relay's browser send");

    tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    let first = tokio::time::timeout(RECV_TIMEOUT, socket.next())
        .await
        .expect("bounded relay cleanup releases the stalled browser");
    assert!(
        !matches!(first, Some(Ok(ClientMessage::Binary(_)))),
        "the stalled send is canceled before peer reads can release it"
    );
}

#[tokio::test]
async fn browser_close_code_and_reason_reach_the_gateway() {
    let (gateway, probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let (mut socket, _) =
        tokio_tungstenite::connect_async(server.ws_url("/v1/realtime?intent=transcription"))
            .await
            .expect("the Workshop Realtime socket upgrades");
    socket
        .send(ClientMessage::Close(Some(
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: 4201.into(),
                reason: "browser finished".into(),
            },
        )))
        .await
        .expect("the browser close sends");
    tokio::time::timeout(RECV_TIMEOUT, probe.close_seen.notified())
        .await
        .expect("the gateway receives the close");
    assert_eq!(
        probe.browser_close(),
        Some((4201, "browser finished".to_owned()))
    );
}

#[tokio::test]
async fn browser_disconnect_releases_the_gateway_peer() {
    let (gateway, probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let (mut socket, _) =
        tokio_tungstenite::connect_async(server.ws_url("/v1/realtime?intent=transcription"))
            .await
            .expect("the Workshop Realtime socket upgrades");
    let close_seen = probe.close_seen.notified();
    let disconnected = probe.disconnected.notified();
    let tokio_tungstenite::MaybeTlsStream::Plain(transport) = socket.get_mut() else {
        panic!("the loopback Workshop test uses a plain transport");
    };
    transport
        .shutdown()
        .await
        .expect("the browser transport disconnects");
    drop(socket);
    tokio::time::timeout(RECV_TIMEOUT, async {
        tokio::select! {
            () = close_seen => {}
            () = disconnected => {}
        }
    })
    .await
    .expect("an abrupt browser disconnect closes the Gateway hop");
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

#[tokio::test]
async fn realtime_relay_enforces_same_origin_authority_and_no_subprotocol() {
    let (gateway, _probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let url = server.ws_url("/v1/realtime?intent=transcription");
    let parsed = url::Url::parse(&url).expect("the Workshop URL parses");
    let authority = parsed
        .socket_addrs(|| None)
        .expect("the Workshop authority resolves")
        .into_iter()
        .next()
        .expect("the Workshop authority has an address");
    let same_origin = format!("http://{authority}");

    let (socket, response) =
        tokio_tungstenite::connect_async(request_with(&url, Some(&same_origin), None))
            .await
            .expect("the exact same origin upgrades");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    drop(socket);

    assert_eq!(
        rejected_status(request_with(&url, Some("http://localhost:9"), None)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        rejected_status(request_with(&url, Some(&same_origin), Some("realtime"))).await,
        StatusCode::BAD_REQUEST
    );
}
