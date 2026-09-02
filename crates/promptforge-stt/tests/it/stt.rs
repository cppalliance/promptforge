//! Characterization tests for the mechanically moved `/stt` socket.

#![expect(
    clippy::expect_used,
    reason = "fixture construction fails the ignored live test with the invariant named"
)]

use std::time::Duration;

use promptforge_stt::{SttRuntime, SttState};
use promptforge_transcribe::fixtures::{jfk_samples, require_model};
use serde_json::json;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::common::{JsonSocket, TestServer};

async fn send_pcm(socket: &mut JsonSocket, frames: usize) {
    socket.send_binary(vec![0u8; frames * 4]).await;
}

async fn send_samples(socket: &mut JsonSocket, samples: &[f32]) {
    const BLOCK: usize = 4096;
    for chunk in samples.chunks(BLOCK) {
        let mut bytes = Vec::with_capacity(chunk.len() * 4);
        for sample in chunk {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        socket.send_binary(bytes).await;
    }
}

fn fixture_server(with_final: bool) -> TestServer {
    let cache = tempfile::tempdir().expect("cache tempdir");
    let source = require_model().display().to_string().replace('\\', "/");
    let cache_path = cache.path().display().to_string().replace('\\', "/");
    let final_model = if with_final {
        format!(
            "[[stt_model]]\nname = \"speech-final\"\nrole = \"final\"\nsource = {source:?}\nvram_gb = 1.0\n"
        )
    } else {
        String::new()
    };
    let profile_models = if with_final {
        "[\"speech\", \"speech-final\"]"
    } else {
        "[\"speech\"]"
    };
    let catalog = promptforge_gateway_config::Config::from_toml_str(&format!(
        "config-version = 2\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
         [local]\ncache_dir = {cache_path:?}\n\
         [workshop.stt]\nwindow_seconds = 8\ninterval_ms = 400\n\
         [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {source:?}\nvram_gb = 1.0\n\
         {final_model}[[profile]]\nname = \"work\"\nmodels = {profile_models}\n"
    ))
    .expect("fixture catalog parses");
    let config = catalog
        .select_profile(
            &promptforge_gateway_config::ProfileName::parse("work").expect("profile name"),
        )
        .expect("fixture profile selects");
    let state = SttState::default();
    let runtime = SttRuntime::start(&config, state.clone(), None).expect("fixture engine loads");
    TestServer::spawn_with(state, Some(runtime))
}

#[tokio::test]
async fn a_take_counts_pcm_frames_and_tags_the_final_with_its_generation() {
    let server = TestServer::spawn();
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(
        socket.recv_json().await,
        json!({"type": "stream", "generation": 1}),
        "a start is answered by the stream announcement before any other frame"
    );
    send_pcm(&mut socket, 128).await;
    send_pcm(&mut socket, 64).await;
    socket.send_binary(vec![0u8; 3]).await;
    socket.send_text("stop").await;

    assert_eq!(
        socket.recv_json().await,
        json!({"type": "final", "text": "", "frames": 192, "generation": 1}),
        "frames are counted, the partial sample is dropped, and no engine means an empty transcript"
    );
    socket.close().await;
}

#[tokio::test]
async fn a_restart_increments_the_generation_and_a_new_connection_resets_it() {
    let server = TestServer::spawn();
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(
        socket.recv_json().await["generation"],
        1,
        "the connection's first take is generation 1"
    );
    send_pcm(&mut socket, 100).await;
    socket.send_text("start").await;
    assert_eq!(
        socket.recv_json().await,
        json!({"type": "stream", "generation": 2}),
        "a restart announces the incremented generation"
    );
    send_pcm(&mut socket, 10).await;
    socket.send_text("stop").await;
    let reply = socket.recv_json().await;
    assert_eq!(
        reply["generation"], 2,
        "the final frame carries its take's generation"
    );
    assert_eq!(
        reply["frames"], 10,
        "the second take counts only its own frames"
    );

    let mut second = JsonSocket::connect(&server.ws_url("/stt")).await;
    second.send_text("start").await;
    assert_eq!(
        second.recv_json().await["generation"],
        1,
        "generations are per-connection"
    );
    socket.close().await;
    second.close().await;
}

#[tokio::test]
async fn stt_upgrade_keeps_the_loopback_origin_allowlist() {
    let server = TestServer::spawn();
    let url = server.ws_url("/stt");
    let mut request = url.into_client_request().expect("request builds");
    request.headers_mut().insert(
        "origin",
        "https://evil.example"
            .parse()
            .expect("origin header parses"),
    );
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("foreign origin is refused");
    match error {
        tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), tungstenite::http::StatusCode::FORBIDDEN);
        }
        other => panic!("expected HTTP refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_text_is_ignored_without_changing_the_take() {
    let server = TestServer::spawn();
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(
        socket.recv_json().await,
        json!({"type": "stream", "generation": 1})
    );
    socket.send_text("bogus").await;
    send_pcm(&mut socket, 10).await;
    socket.send_text("stop").await;
    assert_eq!(
        socket.recv_json().await,
        json!({"type": "final", "text": "", "frames": 10, "generation": 1})
    );
    socket.close().await;
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn speech_produces_generation_tagged_interim_and_final_frames() {
    let server = fixture_server(false);
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(
        socket.recv_json().await,
        json!({"type": "stream", "generation": 1})
    );
    send_samples(&mut socket, &jfk_samples()).await;

    let interim = socket
        .recv_until(Duration::from_secs(90), |frame| {
            frame["type"] == "interim"
                && frame["tentative"]
                    .as_str()
                    .is_some_and(|text| text.to_lowercase().contains("country"))
        })
        .await;
    assert_eq!(
        interim["generation"], 1,
        "every interim frame is tagged with its take's generation"
    );
    assert!(
        interim["committed"].is_string(),
        "every interim frame carries a committed string"
    );

    socket.send_text("stop").await;
    let reply = socket
        .recv_until(Duration::from_secs(180), |frame| frame["type"] == "final")
        .await;
    assert_eq!(
        reply["generation"], 1,
        "the final frame carries its take's generation"
    );
    let text = reply["text"].as_str().expect("final text is a string");
    assert!(
        text.to_lowercase().contains("country"),
        "the final transcript names the fixture's words: {text:?}"
    );
    socket.close().await;
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn interim_only_stop_keeps_speech_before_a_silence_gap() {
    let server = fixture_server(false);
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(socket.recv_json().await["type"], "stream");
    send_samples(&mut socket, &jfk_samples()).await;
    send_pcm(&mut socket, 3 * 16_000).await;
    socket.send_text("stop").await;
    let reply = socket
        .recv_until(Duration::from_secs(180), |frame| frame["type"] == "final")
        .await;
    let text = reply["text"].as_str().expect("final text is a string");
    assert!(
        text.to_lowercase().contains("country"),
        "the fallback decodes the whole take, nothing consumed early: {text:?}"
    );
    socket.close().await;
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn silence_produces_no_interims_and_an_empty_final() {
    let server = fixture_server(false);
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(socket.recv_json().await["type"], "stream");
    send_pcm(&mut socket, 3 * 16_000).await;
    socket.send_text("stop").await;
    let reply = socket.recv_json().await;
    assert_eq!(
        reply,
        json!({"type": "final", "text": "", "frames": 48_000, "generation": 1}),
        "the first message after silence is the stop reply, not an interim"
    );
    socket.close().await;
}

async fn wait_for_committed(socket: &mut JsonSocket) -> String {
    socket
        .recv_until(Duration::from_secs(120), |frame| {
            frame["type"] == "interim"
                && frame["committed"]
                    .as_str()
                    .is_some_and(|text| text.to_lowercase().contains("country"))
        })
        .await["committed"]
        .as_str()
        .expect("every interim frame carries a committed string")
        .to_owned()
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn final_frame_is_the_committed_prefix_plus_the_tail() {
    let server = fixture_server(true);
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(socket.recv_json().await["type"], "stream");
    let samples = jfk_samples();
    send_samples(&mut socket, &samples).await;
    send_pcm(&mut socket, 3 * 16_000).await;
    let committed = wait_for_committed(&mut socket).await;
    send_samples(&mut socket, &samples).await;
    socket.send_text("stop").await;
    let reply = socket
        .recv_until(Duration::from_secs(180), |frame| frame["type"] == "final")
        .await;
    let text = reply["text"].as_str().expect("final text is a string");
    assert!(
        text.starts_with(&committed),
        "the final frame opens with the committed prefix: {text:?}"
    );
    let tail = text[committed.len()..]
        .strip_prefix(' ')
        .expect("a single space joins the committed prefix and tail");
    assert!(
        tail.to_lowercase().contains("country"),
        "the tail contributes its own text: {text:?}"
    );
    socket.close().await;
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn stop_at_a_segment_boundary_returns_the_committed_prefix() {
    let server = fixture_server(true);
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(socket.recv_json().await["type"], "stream");
    send_samples(&mut socket, &jfk_samples()).await;
    send_pcm(&mut socket, 3 * 16_000).await;
    let committed = wait_for_committed(&mut socket).await;
    socket.send_text("stop").await;
    let reply = socket
        .recv_until(Duration::from_secs(180), |frame| frame["type"] == "final")
        .await;
    assert_eq!(
        reply["text"], committed,
        "no uncommitted speech means no tail transcription"
    );
    socket.close().await;
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn interim_frames_keep_committed_text_append_only() {
    let server = fixture_server(true);
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(socket.recv_json().await["type"], "stream");
    let samples = jfk_samples();
    send_samples(&mut socket, &samples).await;
    send_pcm(&mut socket, 3 * 16_000).await;
    send_samples(&mut socket, &samples).await;
    send_pcm(&mut socket, 3 * 16_000).await;
    send_samples(&mut socket, &samples).await;

    let mut committed_frames = Vec::new();
    loop {
        let frame = socket
            .recv_until(Duration::from_secs(120), |frame| frame["type"] == "interim")
            .await;
        let committed = frame["committed"]
            .as_str()
            .expect("every interim frame carries a committed string")
            .to_owned();
        assert!(
            frame["tentative"].is_string(),
            "every interim frame carries a tentative string"
        );
        let complete = committed.to_lowercase().matches("country").count() >= 3;
        committed_frames.push(committed);
        if complete {
            break;
        }
    }
    for pair in committed_frames.windows(2) {
        assert!(
            pair[1].starts_with(&pair[0]),
            "committed text is append-only: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
    socket.send_text("stop").await;
    let reply = socket
        .recv_until(Duration::from_secs(180), |frame| frame["type"] == "final")
        .await;
    assert!(
        reply["text"]
            .as_str()
            .is_some_and(|text| text.starts_with(committed_frames.last().expect("frames exist"))),
        "the assembled transcript opens with the last committed prefix"
    );
    socket.close().await;
}
