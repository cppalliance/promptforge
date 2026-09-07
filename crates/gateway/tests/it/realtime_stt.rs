//! Mounted Realtime transcription route through the production Gateway wall.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt as _, StreamExt as _};
use gateway::{Config, Gateway, ProfilesContext};
use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::{
    ScriptedDecoder, ScriptedModelFactory, begin_scripted_replacement, scripted_service,
};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Error as SocketError, Message};

use crate::support::{PHASE_TIMEOUT, TestServer, send_within};

type Socket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

fn config(strict: bool) -> Config {
    Config::from_toml_str(&format!(
        "config-version = 2\n\
         [server]\n\
         bind = \"127.0.0.1:0\"\n\
         api_key = \"test-token\"\n\
         trust_loopback = {}\n",
        !strict
    ))
    .expect("Gateway test config parses")
}

fn speech(interim: &ScriptedDecoder, final_decoder: Option<&ScriptedDecoder>) -> SpeechService {
    let factory = final_decoder.map_or_else(
        || ScriptedModelFactory::new(interim.clone()),
        |final_decoder| {
            ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone())
        },
    );
    scripted_service(factory, 15, 500).expect("scripted speech starts")
}

async fn server(strict: bool, service: &SpeechService) -> TestServer {
    let gateway = Gateway::new(&config(strict), ProfilesContext::default())
        .with_speech_service(service.clone());
    TestServer::start(gateway).await
}

fn request(
    addr: SocketAddr,
    query: &str,
    bearer: Option<&str>,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = format!("ws://{addr}/v1/realtime?{query}")
        .into_client_request()
        .expect("WebSocket request builds");
    if let Some(bearer) = bearer {
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {bearer}")).expect("bearer is a header"),
        );
    }
    if let Some(cookie) = cookie {
        request.headers_mut().insert(
            "cookie",
            HeaderValue::from_str(cookie).expect("cookie is a header"),
        );
        request
            .headers_mut()
            .insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    }
    if let Some(origin) = origin {
        request.headers_mut().insert(
            "origin",
            HeaderValue::from_str(origin).expect("Origin is a header"),
        );
    }
    request
}

async fn connect(
    addr: SocketAddr,
    bearer: Option<&str>,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> Socket {
    let (socket, response) = tokio::time::timeout(
        PHASE_TIMEOUT,
        tokio_tungstenite::connect_async(request(
            addr,
            "intent=transcription",
            bearer,
            cookie,
            origin,
        )),
    )
    .await
    .expect("WebSocket upgrade answers before deadline")
    .expect("WebSocket upgrades");
    assert_eq!(response.status(), 101);
    socket
}

async fn rejected(
    addr: SocketAddr,
    query: &str,
    bearer: Option<&str>,
    origin: Option<&str>,
) -> u16 {
    rejected_request(request(addr, query, bearer, None, origin)).await
}

async fn rejected_request(request: tokio_tungstenite::tungstenite::http::Request<()>) -> u16 {
    match tokio::time::timeout(PHASE_TIMEOUT, tokio_tungstenite::connect_async(request))
        .await
        .expect("rejected upgrade answers before deadline")
    {
        Err(SocketError::Http(response)) => response.status().as_u16(),
        other => panic!("expected rejected upgrade, got {other:?}"),
    }
}

async fn receive(socket: &mut Socket) -> serde_json::Value {
    let message = tokio::time::timeout(PHASE_TIMEOUT, socket.next())
        .await
        .expect("server frame arrives before deadline")
        .expect("server keeps the socket open")
        .expect("server frame is valid");
    serde_json::from_str(
        message
            .to_text()
            .expect("Realtime server frames are JSON text"),
    )
    .expect("Realtime server frame is JSON")
}

async fn send(socket: &mut Socket, value: serde_json::Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("client event sends");
}

async fn append_audio(socket: &mut Socket, audio: String) {
    send(
        socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": audio
        }),
    )
    .await;
}

fn audio() -> String {
    let bytes = vec![0_u8; 24_000 * 2 / 10];
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn audio_samples(samples: &[i16]) -> String {
    let bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn closed_segment() -> String {
    let mut samples = vec![16_384; 24_000];
    samples.extend(vec![0; 72_000]);
    audio_samples(&samples)
}

fn canonical_sequences() -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("gateway-stt")
        .join("tests")
        .join("fixtures")
        .join("realtime")
        .join("valid-sequences.json");
    serde_json::from_slice(&std::fs::read(path).expect("canonical Realtime sequences read"))
        .expect("canonical Realtime sequences parse")
}

fn canonical_message(
    fixtures: &serde_json::Value,
    sequence: &str,
    direction: &str,
    event_type: &str,
    occurrence: usize,
) -> serde_json::Value {
    fixtures[sequence]["events"]
        .as_array()
        .expect("canonical sequence has events")
        .iter()
        .filter(|entry| entry["direction"] == direction && entry["message"]["type"] == event_type)
        .nth(occurrence)
        .unwrap_or_else(|| {
            panic!(
                "{sequence} has {direction} {event_type} occurrence {}",
                occurrence + 1
            )
        })["message"]
        .clone()
}

fn canonical_first_message(
    fixtures: &serde_json::Value,
    sequence: &str,
    direction: &str,
    event_type: &str,
) -> serde_json::Value {
    canonical_message(fixtures, sequence, direction, event_type, 0)
}

fn canonical_client(
    fixtures: &serde_json::Value,
    sequence: &str,
    event_type: &str,
) -> serde_json::Value {
    canonical_first_message(fixtures, sequence, "client", event_type)
}

fn canonical_server(
    fixtures: &serde_json::Value,
    sequence: &str,
    event_type: &str,
) -> serde_json::Value {
    canonical_first_message(fixtures, sequence, "server", event_type)
}

async fn expect_type(socket: &mut Socket, expected: &str) -> serde_json::Value {
    let event = receive(socket).await;
    assert_eq!(event["type"], expected, "{event}");
    event
}

async fn expect_error(
    socket: &mut Socket,
    kind: &str,
    code: &str,
    message: &str,
    param: serde_json::Value,
    client_event_id: &str,
) -> serde_json::Value {
    let event = expect_type(socket, "error").await;
    assert_eq!(event["error"]["type"], kind, "{event}");
    assert_eq!(event["error"]["code"], code, "{event}");
    assert_eq!(event["error"]["message"], message, "{event}");
    assert_eq!(event["error"]["param"], param, "{event}");
    assert_eq!(event["error"]["event_id"], client_event_id, "{event}");
    event
}

#[tokio::test]
async fn canonical_fixture_drives_hypothesis_completion_and_clear() {
    let fixtures = canonical_sequences();
    let interim = ScriptedDecoder::new();
    interim.push_text("Hello");
    interim.push_text("Hello!");
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("Hello");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;

    let created = expect_type(&mut socket, "session.created").await;
    assert_eq!(
        created["type"],
        canonical_server(&fixtures, "first_event_readiness", "session.created")["type"]
    );
    send(
        &mut socket,
        canonical_client(&fixtures, "hypothesis_negotiation", "session.update"),
    )
    .await;
    expect_type(&mut socket, "session.updated").await;

    for _ in 0..2 {
        let mut append = canonical_client(
            &fixtures,
            "hypothesis_negotiation",
            "input_audio_buffer.append",
        );
        append["audio"] = serde_json::json!(audio());
        send(&mut socket, append).await;
    }
    let first = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    let second = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    assert_eq!(first["revision"], 1);
    assert_eq!(first["transcript"], "Hello");
    assert_eq!(second["revision"], 2);
    assert_eq!(second["transcript"], "Hello!");

    send(
        &mut socket,
        canonical_client(
            &fixtures,
            "immediate_commit_and_provisional_promotion",
            "input_audio_buffer.commit",
        ),
    )
    .await;
    let committed = expect_type(&mut socket, "input_audio_buffer.committed").await;
    let item_id = committed["item_id"].clone();
    assert_eq!(
        expect_type(&mut socket, "conversation.item.created").await["item"]["id"],
        item_id
    );
    let completed = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;
    assert_eq!(completed["item_id"], item_id);
    assert_eq!(
        completed["transcript"],
        canonical_server(
            &fixtures,
            "hypothesis_negotiation",
            "conversation.item.input_audio_transcription.completed",
        )["transcript"]
    );

    let mut append = canonical_client(
        &fixtures,
        "clear_retires_only_uncommitted_input",
        "input_audio_buffer.append",
    );
    append["audio"] = serde_json::json!(audio());
    send(&mut socket, append).await;
    send(
        &mut socket,
        canonical_client(
            &fixtures,
            "clear_retires_only_uncommitted_input",
            "input_audio_buffer.clear",
        ),
    )
    .await;
    expect_type(&mut socket, "input_audio_buffer.cleared").await;

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}

#[tokio::test]
async fn producer_snapshots_partition_finalized_agreed_and_tentative_text() {
    let fixtures = canonical_sequences();
    let interim = ScriptedDecoder::new();
    for transcript in [
        "ask not your country",
        "ask not your country",
        "new tail first",
        "new tail second",
    ] {
        interim.push_text(transcript);
    }
    let final_decoder = ScriptedDecoder::new();
    final_decoder.park_next();
    final_decoder.push_text("ask not your kingdom");
    final_decoder.push_text("second final");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;

    expect_type(&mut socket, "session.created").await;
    send(
        &mut socket,
        canonical_client(&fixtures, "producer_hypothesis_ownership", "session.update"),
    )
    .await;
    expect_type(&mut socket, "session.updated").await;

    append_audio(&mut socket, closed_segment()).await;
    expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    append_audio(&mut socket, audio()).await;
    expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;

    assert!(final_decoder.wait_until_parked(PHASE_TIMEOUT));
    interim.park_next();
    final_decoder.release();
    let completed_final = final_decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || completed_final.wait_for_completed(1, PHASE_TIMEOUT))
            .await
            .expect("final completion observer joins")
    );
    final_decoder.park_next();
    append_audio(&mut socket, closed_segment()).await;
    let parked_interim = interim.clone();
    assert!(
        tokio::task::spawn_blocking(move || parked_interim.wait_until_parked(PHASE_TIMEOUT))
            .await
            .expect("interim park observer joins")
    );
    assert!(
        final_decoder.wait_for_requests(2, PHASE_TIMEOUT),
        "the first closed segment is finalized before snapshot assembly"
    );
    interim.release();

    let third = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    append_audio(&mut socket, audio()).await;
    let fourth = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;

    for (actual, occurrence) in [(third, 0), (fourth, 1)] {
        let expected = canonical_message(
            &fixtures,
            "producer_hypothesis_ownership",
            "server",
            "conversation.item.input_audio_transcription.hypothesis",
            occurrence,
        );
        for field in [
            "revision",
            "transcript",
            "finalized",
            "agreed",
            "tentative",
            "audio_start_ms",
            "audio_end_ms",
        ] {
            assert_eq!(actual[field], expected[field], "{field}: {actual}");
        }
    }

    final_decoder.release();
    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}

#[tokio::test]
async fn gateway_auth_origin_query_and_legacy_surfaces_precede_upgrade() {
    let service = speech(&ScriptedDecoder::new(), Some(&ScriptedDecoder::new()));
    let strict = server(true, &service).await;

    assert_eq!(
        rejected(
            strict.addr,
            "intent=transcription&intent=transcription",
            Some("wrong"),
            None,
        )
        .await,
        401,
        "Gateway auth runs before Realtime query validation"
    );
    assert_eq!(
        rejected(
            strict.addr,
            "intent=transcription&intent=transcription",
            Some("test-token"),
            None,
        )
        .await,
        400
    );
    assert_eq!(
        rejected(
            strict.addr,
            "intent=transcription",
            Some("test-token"),
            Some("http://evil.example"),
        )
        .await,
        403
    );
    let mut duplicate_origin = request(
        strict.addr,
        "intent=transcription",
        Some("test-token"),
        None,
        None,
    );
    duplicate_origin
        .headers_mut()
        .append("origin", HeaderValue::from_static("http://localhost:8080"));
    duplicate_origin
        .headers_mut()
        .append("origin", HeaderValue::from_static("http://localhost:8080"));
    assert_eq!(rejected_request(duplicate_origin).await, 403);

    for origin in [None, Some("http://localhost:8080")] {
        let mut socket = connect(strict.addr, Some("test-token"), None, origin).await;
        expect_type(&mut socket, "session.created").await;
        socket.close(None).await.expect("socket closes");
        drop(socket);
    }

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client builds");
    let handoff =
        send_within(http.get(format!("http://{}/auth?key=test-token", strict.addr))).await;
    let cookie = handoff
        .headers()
        .get("set-cookie")
        .expect("handoff sets a cookie")
        .to_str()
        .expect("cookie is text")
        .split(';')
        .next()
        .expect("cookie has a pair")
        .to_owned();
    let mut cookie_socket = connect(strict.addr, None, Some(&cookie), None).await;
    expect_type(&mut cookie_socket, "session.created").await;
    cookie_socket.close(None).await.expect("socket closes");
    drop(cookie_socket);

    for (method, path) in [
        ("POST", "/v1/audio/transcriptions"),
        ("GET", "/stt"),
        ("GET", "/stt/capability"),
    ] {
        let response = send_within(
            http.request(
                reqwest::Method::from_bytes(method.as_bytes()).expect("method is valid"),
                format!("http://{}{path}", strict.addr),
            )
            .bearer_auth("test-token"),
        )
        .await;
        assert_ne!(
            response.status(),
            reqwest::StatusCode::NOT_FOUND,
            "{method} {path} remains mounted"
        );
    }
    strict.shutdown().await;

    let trusted = server(false, &service).await;
    let mut socket = connect(trusted.addr, None, None, None).await;
    expect_type(&mut socket, "session.created").await;
    socket.close(None).await.expect("socket closes");
    drop(socket);
    trusted.shutdown().await;
}

#[tokio::test]
async fn mounted_route_drives_scripted_wire_ownership_errors_and_privacy() {
    let interim = ScriptedDecoder::new();
    interim.push_text("provisional transcript");
    interim.push_text("provisional transcript");
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("authoritative transcript");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;

    let created = expect_type(&mut socket, "session.created").await;
    assert_eq!(created["session"]["type"], "transcription");
    send(
        &mut socket,
        serde_json::json!({
            "type": "session.update",
            "event_id": "private-client-update",
            "session": {
                "type": "transcription",
                "audio": {"input": {"transcription": {"prompt": "private prompt"}}},
                "include": []
            }
        }),
    )
    .await;
    let updated = expect_type(&mut socket, "session.updated").await;
    assert_eq!(
        updated["session"]["audio"]["input"]["transcription"]["prompt"],
        "private prompt"
    );

    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "event_id": "bad-audio",
            "audio": 7
        }),
    )
    .await;
    let error = expect_type(&mut socket, "error").await;
    assert_eq!(error["error"]["event_id"], "bad-audio");
    assert!(
        !error.to_string().contains(&audio()),
        "errors never echo buffered audio"
    );

    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "event_id": "append-one",
            "audio": audio()
        }),
    )
    .await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "event_id": "append-two",
            "audio": audio()
        }),
    )
    .await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.commit",
            "event_id": "commit-one"
        }),
    )
    .await;
    let committed = expect_type(&mut socket, "input_audio_buffer.committed").await;
    let item_id = committed["item_id"]
        .as_str()
        .expect("commit owns an item")
        .to_owned();
    assert_eq!(committed["item_id"], item_id);
    assert!(committed["previous_item_id"].is_null());
    let item = expect_type(&mut socket, "conversation.item.created").await;
    assert_eq!(item["item"]["id"], item_id);
    let delta = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.delta",
    )
    .await;
    assert_eq!(delta["item_id"], item_id);
    assert_eq!(delta["delta"], "provisional transcript");
    let complete = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;
    assert_eq!(complete["item_id"], item_id);
    assert_eq!(complete["transcript"], "authoritative transcript");

    let interim_requests = interim.requests();
    assert_eq!(interim_requests.len(), 2);
    assert_eq!(interim_requests[0].guidance(), ["private prompt"]);
    assert_eq!(final_decoder.requests().len(), 1);

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}

#[tokio::test]
async fn admission_is_bounded_and_replacement_closes_with_1012() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.park_next();
    final_decoder.push_text("too late");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut sockets = Vec::new();
    for _ in 0..8 {
        let mut socket = connect(server.addr, Some("test-token"), None, None).await;
        expect_type(&mut socket, "session.created").await;
        sockets.push(socket);
    }
    assert_eq!(
        rejected(
            server.addr,
            "intent=transcription",
            Some("test-token"),
            None
        )
        .await,
        429
    );
    for mut socket in sockets.drain(1..) {
        socket.close(None).await.expect("socket closes");
    }
    send(
        &mut sockets[0],
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": audio()
        }),
    )
    .await;
    send(
        &mut sockets[0],
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    let committed = expect_type(&mut sockets[0], "input_audio_buffer.committed").await;
    let item_id = committed["item_id"].as_str().expect("item ID").to_owned();
    expect_type(&mut sockets[0], "conversation.item.created").await;
    let parked = final_decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || parked.wait_until_parked(PHASE_TIMEOUT))
            .await
            .expect("park observer joins"),
        "committed item owns its final decode"
    );

    let replacement = ScriptedDecoder::new();
    let replacement_final = ScriptedDecoder::new();
    let replacement_service = service.clone();
    let replacement_task = tokio::task::spawn_blocking(move || {
        begin_scripted_replacement(
            &replacement_service,
            ScriptedModelFactory::new(replacement).with_final(replacement_final),
            true,
            PHASE_TIMEOUT,
        )
    });
    let replaced = expect_type(
        &mut sockets[0],
        "conversation.item.input_audio_transcription.failed",
    )
    .await;
    assert_eq!(replaced["item_id"], item_id);
    assert_eq!(replaced["error"]["code"], "engine_replaced");
    let message = tokio::time::timeout(PHASE_TIMEOUT, sockets[0].next())
        .await
        .expect("replacement closes the socket before its deadline")
        .expect("socket emits a close frame")
        .expect("close frame is valid");
    let Message::Close(Some(close)) = message else {
        panic!("replacement emits a close frame, got {message:?}");
    };
    assert_eq!(u16::from(close.code), 1012);
    assert_eq!(close.reason, "engine_replaced");
    drop(sockets);
    final_decoder.release();

    let staged = replacement_task
        .await
        .expect("replacement task joins")
        .expect("replacement stages after session ownership drains");
    service
        .commit_replacement(staged)
        .expect("replacement commits");
    let mut replacement_socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut replacement_socket, "session.created").await;
    replacement_socket
        .close(None)
        .await
        .expect("replacement socket closes");
    drop(replacement_socket);
    server.shutdown().await;
}

#[tokio::test]
async fn blocked_server_send_expires_and_releases_admission() {
    let interim = ScriptedDecoder::new();
    interim.push_text("blocked transcript");
    let mut service = speech(&interim, Some(&ScriptedDecoder::new()));
    service.block_realtime_send_after(8);
    let server = server(true, &service).await;

    let mut blocked = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut blocked, "session.created").await;
    let mut occupants = Vec::new();
    for _ in 0..7 {
        let mut socket = connect(server.addr, Some("test-token"), None, None).await;
        expect_type(&mut socket, "session.created").await;
        occupants.push(socket);
    }
    send(
        &mut blocked,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": audio()
        }),
    )
    .await;
    send(
        &mut blocked,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    assert_eq!(
        rejected(
            server.addr,
            "intent=transcription",
            Some("test-token"),
            None
        )
        .await,
        429,
        "the blocked send initially retains its session"
    );

    tokio::time::sleep(Duration::from_secs(2)).await;
    let admitted = connect(server.addr, Some("test-token"), None, None).await;

    drop(admitted);
    for mut socket in occupants {
        socket.close(None).await.expect("socket closes");
    }
    drop(blocked);
    server.shutdown().await;
}

#[tokio::test]
async fn mounted_session_errors_keep_canonical_codes_parameters_and_correlation() {
    let interim = ScriptedDecoder::new();
    interim.push_error("scripted interim failure");
    let service = speech(&interim, Some(&ScriptedDecoder::new()));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "event_id": "invalid-audio",
            "audio": "***"
        }),
    )
    .await;
    expect_error(
        &mut socket,
        "invalid_request_error",
        "invalid_base64_audio",
        "Audio must be valid Base64",
        serde_json::json!("audio"),
        "invalid-audio",
    )
    .await;

    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "event_id": "inference",
            "audio": audio()
        }),
    )
    .await;
    expect_error(
        &mut socket,
        "server_error",
        "internal_error",
        "Transcription failed",
        serde_json::Value::Null,
        "inference",
    )
    .await;

    send(
        &mut socket,
        serde_json::json!({"type": "input_audio_buffer.clear"}),
    )
    .await;
    expect_type(&mut socket, "input_audio_buffer.cleared").await;
    let short = base64::engine::general_purpose::STANDARD.encode([0_u8, 0]);
    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": short
        }),
    )
    .await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.commit",
            "event_id": "short-commit"
        }),
    )
    .await;
    expect_error(
        &mut socket,
        "invalid_request_error",
        "audio_too_short",
        "A commit requires at least 100 ms of audio",
        serde_json::json!("audio"),
        "short-commit",
    )
    .await;

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}

#[tokio::test]
async fn standard_interims_emit_only_appendable_agreed_deltas() {
    let interim = ScriptedDecoder::new();
    for transcript in ["Hello there", "Hello world", "Hello world again"] {
        interim.push_text(transcript);
    }
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("Hello world again");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    for _ in 0..3 {
        send(
            &mut socket,
            serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": audio()
            }),
        )
        .await;
    }
    send(
        &mut socket,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    expect_type(&mut socket, "input_audio_buffer.committed").await;
    expect_type(&mut socket, "conversation.item.created").await;
    let first = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.delta",
    )
    .await;
    let second = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.delta",
    )
    .await;
    assert_eq!(first["delta"], "Hello");
    assert_eq!(second["delta"], " world");
    assert_eq!(
        format!(
            "{}{}",
            first["delta"].as_str().expect("first delta is text"),
            second["delta"].as_str().expect("second delta is text")
        ),
        "Hello world"
    );
    expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}

#[tokio::test]
async fn mounted_terminal_failures_preserve_their_typed_wire_reason() {
    let fixtures = canonical_sequences();
    let canonical_overload = canonical_server(
        &fixtures,
        "segment_admission_failure",
        "conversation.item.input_audio_transcription.failed",
    );
    for (overload, kind, code, message) in [
        (
            false,
            "server_error",
            "precommit_transcription_failed",
            "Accurate precommit transcription failed",
        ),
        (
            true,
            "overload_error",
            "final_segment_overload",
            "The authoritative segment could not be admitted",
        ),
    ] {
        let mut service = speech(&ScriptedDecoder::new(), Some(&ScriptedDecoder::new()));
        if overload {
            service.overload_realtime_final_segment();
        } else {
            service.fail_realtime_precommit();
        }
        let server = server(true, &service).await;
        let mut socket = connect(server.addr, Some("test-token"), None, None).await;
        expect_type(&mut socket, "session.created").await;
        send(
            &mut socket,
            serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": audio()
            }),
        )
        .await;
        send(
            &mut socket,
            serde_json::json!({"type": "input_audio_buffer.commit"}),
        )
        .await;
        expect_type(&mut socket, "input_audio_buffer.committed").await;
        expect_type(&mut socket, "conversation.item.created").await;
        let failed = expect_type(
            &mut socket,
            "conversation.item.input_audio_transcription.failed",
        )
        .await;
        assert_eq!(failed["error"]["type"], kind, "{failed}");
        assert_eq!(failed["error"]["code"], code, "{failed}");
        assert_eq!(failed["error"]["message"], message, "{failed}");
        assert!(failed["error"]["param"].is_null(), "{failed}");
        assert!(failed["error"].get("event_id").is_none(), "{failed}");
        if overload {
            assert_eq!(failed["error"], canonical_overload["error"]);
        }

        socket.close(None).await.expect("socket closes");
        drop(socket);
        server.shutdown().await;
    }
}

async fn commit_existing_item(
    socket: &mut Socket,
    append: &serde_json::Value,
    previous: Option<&String>,
) -> String {
    send(socket, append.clone()).await;
    send(
        socket,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    let committed = expect_type(socket, "input_audio_buffer.committed").await;
    let item_id = committed["item_id"]
        .as_str()
        .expect("committed item has an ID")
        .to_owned();
    assert_eq!(
        committed["previous_item_id"],
        previous.map_or(serde_json::Value::Null, |item| {
            serde_json::Value::String(item.clone())
        }),
        "{committed}"
    );
    let created = expect_type(socket, "conversation.item.created").await;
    assert_eq!(created["item"]["id"], item_id, "{created}");
    item_id
}

async fn expect_existing_completions(
    socket: &mut Socket,
    existing_items: &[String],
    expected_release: &serde_json::Value,
) {
    let mut completed_items = Vec::new();
    let mut released_item = None;
    for _ in existing_items {
        let completed = expect_type(
            socket,
            "conversation.item.input_audio_transcription.completed",
        )
        .await;
        if completed["transcript"] == expected_release["transcript"] {
            assert_eq!(
                completed["usage"]["type"], expected_release["usage"]["type"],
                "{completed}"
            );
            assert!(
                completed["usage"]["seconds"]
                    .as_f64()
                    .is_some_and(|seconds| seconds > 0.0),
                "{completed}"
            );
            released_item = completed["item_id"].as_str().map(str::to_owned);
        }
        completed_items.push(
            completed["item_id"]
                .as_str()
                .expect("completion has an item ID")
                .to_owned(),
        );
    }
    assert!(
        released_item.is_some(),
        "the canonical capacity-release completion is observed"
    );
    assert!(
        existing_items
            .iter()
            .all(|item| completed_items.contains(item)),
        "only the four existing items complete"
    );
}

async fn expect_retried_item(
    socket: &mut Socket,
    retry: serde_json::Value,
    existing_items: &[String],
) {
    send(socket, retry).await;
    let retried = expect_type(socket, "input_audio_buffer.committed").await;
    let retried_item = retried["item_id"]
        .as_str()
        .expect("retried commit has an item ID")
        .to_owned();
    assert_eq!(
        retried["previous_item_id"],
        serde_json::Value::String(existing_items.last().expect("four existing items").clone()),
        "{retried}"
    );
    assert!(
        !existing_items.contains(&retried_item),
        "retry promotes the preserved provisional input as a new durable item"
    );
    let created = expect_type(socket, "conversation.item.created").await;
    assert_eq!(created["item"]["id"], retried_item, "{created}");
    let completed = expect_type(
        socket,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;
    assert_eq!(completed["item_id"], retried_item, "{completed}");
    assert_eq!(completed["transcript"], "retried canonical input");
}

#[tokio::test]
async fn saturated_commit_preserves_the_canonical_input_for_retry() {
    let fixtures = canonical_sequences();
    let append = canonical_client(
        &fixtures,
        "saturated_commit_retry",
        "input_audio_buffer.append",
    );
    let commit = canonical_message(
        &fixtures,
        "saturated_commit_retry",
        "client",
        "input_audio_buffer.commit",
        0,
    );
    let retry = canonical_message(
        &fixtures,
        "saturated_commit_retry",
        "client",
        "input_audio_buffer.commit",
        1,
    );
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.park_next();
    for transcript in [
        "released",
        "existing two",
        "existing three",
        "existing four",
    ] {
        final_decoder.push_text(transcript);
    }
    final_decoder.push_text("retried canonical input");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    let mut existing_items: Vec<String> = Vec::new();
    for _ in 0..4 {
        let item_id = commit_existing_item(&mut socket, &append, existing_items.last()).await;
        existing_items.push(item_id);
    }
    let parked = final_decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || parked.wait_until_parked(PHASE_TIMEOUT))
            .await
            .expect("park observer joins"),
        "four committed items remain outstanding behind the parked final worker"
    );
    assert_eq!(
        final_decoder.requests().len(),
        1,
        "the serial final worker is parked while four items own finalization"
    );

    send(&mut socket, append.clone()).await;
    send(&mut socket, commit).await;
    let saturated = expect_type(&mut socket, "error").await;
    let requests_at_saturation = final_decoder.requests().len();
    final_decoder.release();
    let expected_error = canonical_server(&fixtures, "saturated_commit_retry", "error");
    for field in ["type", "code", "message", "param", "event_id"] {
        assert_eq!(
            saturated["error"][field], expected_error["error"][field],
            "{field}: {saturated}"
        );
    }
    assert_eq!(
        requests_at_saturation, 1,
        "the rejected commit starts no fifth finalization"
    );

    let expected_release = canonical_server(
        &fixtures,
        "saturated_commit_retry",
        "conversation.item.input_audio_transcription.completed",
    );
    expect_existing_completions(&mut socket, &existing_items, &expected_release).await;
    expect_retried_item(&mut socket, retry, &existing_items).await;

    let final_requests = final_decoder.requests();
    assert_eq!(final_requests.len(), 5);
    assert_eq!(
        final_requests[4].samples(),
        final_requests[0].samples(),
        "retry finalizes exactly the same canonical audio as an accepted item"
    );

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
