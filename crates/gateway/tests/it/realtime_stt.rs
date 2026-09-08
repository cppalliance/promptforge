//! Mounted Realtime transcription route through the production Gateway wall.

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt as _, StreamExt as _};
use gateway::{Config, Gateway, ProfilesContext};
use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::{
    ScriptedDecoder, ScriptedModelFactory, begin_scripted_replacement, scripted_service,
};
use gateway_stt_engine::test_fixtures::native::require_fixture;
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
    speech_with_policy(interim, final_decoder, 15, 500)
}

fn speech_with_policy(
    interim: &ScriptedDecoder,
    final_decoder: Option<&ScriptedDecoder>,
    window_seconds: u64,
    interval_ms: u64,
) -> SpeechService {
    let factory = final_decoder.map_or_else(
        || ScriptedModelFactory::new(interim.clone()),
        |final_decoder| {
            ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone())
        },
    );
    scripted_service(factory, window_seconds, interval_ms).expect("scripted speech starts")
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
    receive_within(socket, PHASE_TIMEOUT).await
}

async fn receive_within(socket: &mut Socket, timeout: Duration) -> serde_json::Value {
    let message = tokio::time::timeout(timeout, socket.next())
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
    audio_samples(&vec![8_192; 2_400])
}

fn audio_samples(samples: &[i16]) -> String {
    let bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn native_fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/stt-fixtures")
}

#[test]
fn gateway_native_realtime_keeps_its_workspace_local_fixture_root() {
    assert_eq!(
        native_fixture_root(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/stt-fixtures")
    );
}

fn native_jfk_24khz() -> Vec<i16> {
    let path = require_fixture(
        "PROMPTFORGE_WHISPER_AUDIO",
        &native_fixture_root(),
        "jfk.wav",
    );
    let mut reader = hound::WavReader::open(path).expect("JFK fixture opens");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000);
    assert_eq!(spec.channels, 1);
    let source = reader
        .samples::<i16>()
        .map(|sample| sample.expect("JFK sample decodes"))
        .collect::<Vec<_>>();
    let mut resampled = Vec::with_capacity(source.len() * 3 / 2);
    for pair in source.chunks(2) {
        let first = pair[0];
        let second = pair.get(1).copied().unwrap_or(first);
        let midpoint = i16::try_from(i32::midpoint(i32::from(first), i32::from(second)))
            .expect("the midpoint of two i16 samples remains i16");
        resampled.extend([first, midpoint, second]);
    }
    resampled
}

fn native_speech_service() -> SpeechService {
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &native_fixture_root(),
        "ggml-tiny.en.bin",
    );
    let model = model.display().to_string().replace('\\', "/");
    std::thread::spawn(move || {
        let cache = tempfile::tempdir().expect("native test cache creates");
        let cache = cache.path().display().to_string().replace('\\', "/");
        let catalog = Config::from_toml_str(&format!(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [local]\ncache_dir = {cache:?}\n\
             [stt]\nwindow_seconds = 4\ninterval_ms = 500\n\
             [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {model:?}\nvram_gb = 1.0\n\
             [[stt_model]]\nname = \"speech-final\"\nrole = \"final\"\nsource = {model:?}\nvram_gb = 1.0\n\
             [[profile]]\nname = \"native\"\nmodels = [\"speech\", \"speech-final\"]\n"
        ))
        .expect("native fixture catalog parses");
        let config = catalog
            .select_profile(&gateway_config::ProfileName::parse("native").expect("profile name"))
            .expect("native fixture profile selects");
        let service = SpeechService::new();
        let prepared = service.prepare(&config, None).expect("artifacts prepare");
        let replacement = service
            .begin_replacement(prepared)
            .expect("native engine loads");
        service
            .commit_replacement(replacement)
            .expect("native generation publishes");
        service
    })
    .join()
    .expect("native startup thread joins")
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

async fn assert_stop_reconciles_skipped_range(short_input_samples: usize) {
    let interim = ScriptedDecoder::new();
    interim.push_text("last word");
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("corrected first");
    let service = speech_with_policy(&interim, Some(&final_decoder), 15, 50);
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "include": ["item.input_audio_transcription.hypothesis"]
            }
        }),
    )
    .await;
    expect_type(&mut socket, "session.updated").await;

    let before_stop = [
        vec![8_192; 24_000],
        vec![0; 72_000],
        vec![8_192; short_input_samples],
    ]
    .concat();
    let hypothesis = final_decoder
        .with_next_decode_blocked(
            PHASE_TIMEOUT,
            || async {
                append_audio(&mut socket, audio_samples(&before_stop)).await;
                let hypothesis = expect_type(
                    &mut socket,
                    "conversation.item.input_audio_transcription.hypothesis",
                )
                .await;
                (&mut socket, hypothesis)
            },
            |(socket, hypothesis)| async {
                assert!(
                    hypothesis["transcript"]
                        .as_str()
                        .is_some_and(|text| text.ends_with("last word"))
                );
                append_audio(socket, audio_samples(&vec![0; 72_000])).await;
                send(
                    socket,
                    serde_json::json!({"type": "input_audio_buffer.commit"}),
                )
                .await;
                expect_type(socket, "input_audio_buffer.committed").await;
                expect_type(socket, "conversation.item.created").await;
                hypothesis
            },
        )
        .await
        .expect("the accepted hypothesis is captured while earlier final work is blocked");
    let completed = loop {
        let event = receive(&mut socket).await;
        if event["type"] == "conversation.item.input_audio_transcription.completed" {
            break event;
        }
    };

    assert_eq!(
        completed["transcript"],
        "corrected first last word",
        "accepted={hypothesis}, final_lengths={:?}",
        final_decoder
            .requests()
            .iter()
            .map(|request| request.samples().len())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        final_decoder.requests().len(),
        1,
        "the 300 ms final range and stop-time silence are explicit skips"
    );

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}

async fn assert_same_range_final_authority(final_text: &str, expected: &str) {
    let interim = ScriptedDecoder::new();
    interim.push_text("provisional words");
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text(final_text);
    let service = speech_with_policy(&interim, Some(&final_decoder), 15, 50);
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "include": ["item.input_audio_transcription.hypothesis"]
            }
        }),
    )
    .await;
    expect_type(&mut socket, "session.updated").await;

    final_decoder
        .with_next_decode_blocked(
            PHASE_TIMEOUT,
            || async {
                append_audio(&mut socket, audio_samples(&vec![8_192; 12_000])).await;
                let hypothesis = expect_type(
                    &mut socket,
                    "conversation.item.input_audio_transcription.hypothesis",
                )
                .await;
                assert_eq!(hypothesis["transcript"], "provisional words");
                send(
                    &mut socket,
                    serde_json::json!({"type": "input_audio_buffer.commit"}),
                )
                .await;
                expect_type(&mut socket, "input_audio_buffer.committed").await;
                expect_type(&mut socket, "conversation.item.created").await;
            },
            |()| async {},
        )
        .await
        .expect("the exact accepted range reaches blocked authoritative final decoding");
    let completed = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;
    assert_eq!(completed["transcript"], expected);

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}

fn assert_native_incremental_spans(spans: &[(u64, u64, String)]) {
    assert!(
        spans.windows(2).all(|pair| pair[0].1 < pair[1].1),
        "incremental snapshots advance their accepted audio end"
    );
    assert_eq!(spans[0].0, 0);
    let normalized = spans
        .iter()
        .map(|(_, _, transcript)| normalized_words(transcript))
        .collect::<Vec<_>>();
    assert_eq!(
        spans.iter().map(|span| span.0).collect::<Vec<_>>(),
        [0, 0, 0, 1_000],
        "three growing windows precede the first one-second slide"
    );
    assert!(
        normalized[1].len() < normalized[2].len() && normalized[2].starts_with(&normalized[1]),
        "the fixed-origin JFK hypothesis grows before sliding: {normalized:?}"
    );
    assert!(
        spans.iter().skip(1).any(|(start, _, _)| *start > 0),
        "the packaged native route eventually slides its window origin"
    );
    assert!(
        normalized[3].starts_with(&normalized[2]),
        "sliding snapshots retain prior speech exactly once: {normalized:?}"
    );
    assert_eq!(
        normalized.last().expect("a final native snapshot exists"),
        &["and", "so", "my", "fellow", "americans", "ask", "not"],
        "the known JFK overlap is rebased without duplication"
    );
    assert!(
        spans
            .iter()
            .all(|(_, _, transcript)| !transcript.is_empty()),
        "every emitted native hypothesis carries replacement text"
    );
}

fn normalized_words(transcript: &str) -> Vec<String> {
    transcript
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

async fn assert_final_speech_route_surface(http: &reqwest::Client, address: SocketAddr) {
    let batch = send_within(
        http.post(format!("http://{address}/v1/audio/transcriptions"))
            .bearer_auth("test-token"),
    )
    .await;
    assert_ne!(
        batch.status(),
        reqwest::StatusCode::NOT_FOUND,
        "POST /v1/audio/transcriptions remains mounted"
    );
    for path in ["/stt", "/stt/capability"] {
        let response = send_within(
            http.get(format!("http://{address}{path}"))
                .bearer_auth("test-token"),
        )
        .await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::NOT_FOUND,
            "GET {path} is retired"
        );
    }
}

include!("realtime_stt/authentication.rs");
include!("realtime_stt/protocol.rs");
include!("realtime_stt/scheduling.rs");
include!("realtime_stt/lifecycle.rs");
include!("realtime_stt/recovery.rs");
include!("realtime_stt/overload.rs");
include!("realtime_stt/capacity.rs");
include!("realtime_stt/canonical_sequence.rs");
include!("realtime_stt/window_revision.rs");
include!("realtime_stt/hour.rs");
