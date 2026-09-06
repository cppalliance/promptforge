//! Characterization tests for the mechanically moved `/stt` socket.
//! Miri excludes these OS socket tests; native CI owns their coverage.

#![expect(
    clippy::expect_used,
    reason = "fixture construction fails the ignored live test with the invariant named"
)]

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use gateway_stt::Segmenter;
use gateway_stt_engine::EnginePolicy;
use serde_json::json;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::common::{
    JsonSocket, TestServer, copy_model_replacing_token, fixture_runtime,
    fixture_runtime_with_models, fixture_server, jfk_samples, require_model, send_pcm,
    send_samples, send_samples_once, transcribe_batch,
};

#[test]
fn legacy_stream_policy_constants_stay_pinned() {
    let capture = gateway_config::SttPipelineConfig::default();
    assert_eq!(
        EnginePolicy::SAMPLE_RATE,
        16_000,
        "wire PCM stays at 16 kHz"
    );
    assert_eq!(
        EnginePolicy::MIN_WINDOW_SAMPLES,
        EnginePolicy::SAMPLE_RATE / 2,
        "interim decoding still requires half a second"
    );
    assert_eq!(
        capture.window_seconds(),
        15,
        "the default interim window stays fifteen seconds"
    );
    assert_eq!(
        capture.interval_ms(),
        500,
        "the default interim cadence stays 500 ms"
    );
}

fn transcript_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|character: char| !character.is_ascii_alphanumeric())
                .to_ascii_lowercase()
        })
        .filter(|word| word.len() >= 4)
        .collect()
}

fn distinguishing_word(text: &str, other: &str) -> String {
    let other = transcript_words(other);
    transcript_words(text)
        .into_iter()
        .find(|word| !other.contains(word))
        .expect("the two speech segments have distinguishable words")
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn closed_segments_are_reported_in_input_order() {
    let speech = jfk_samples();
    let third = speech.len() / 3;
    let mut samples = speech[..third].to_vec();
    samples.extend(vec![0.0; 3 * EnginePolicy::SAMPLE_RATE]);
    samples.extend_from_slice(&speech[2 * third..]);
    samples.extend(vec![0.0; 3 * EnginePolicy::SAMPLE_RATE]);
    let mut segmenter = Segmenter::new();
    let mut ranges = Vec::new();
    while let Some(range) = segmenter.poll(&samples) {
        ranges.push(range);
    }
    assert_eq!(
        ranges.len(),
        2,
        "the native fixture halves form two closed speech segments"
    );

    let (state, runtime) = fixture_runtime(true);
    let (first_status, first_response) =
        transcribe_batch(state.clone(), "speech-final", &samples[ranges[0].clone()]).await;
    let (second_status, second_response) =
        transcribe_batch(state.clone(), "speech-final", &samples[ranges[1].clone()]).await;
    assert_eq!(first_status, axum::http::StatusCode::OK);
    assert_eq!(second_status, axum::http::StatusCode::OK);
    let first = first_response["text"]
        .as_str()
        .expect("first segment transcript is a string");
    let second = second_response["text"]
        .as_str()
        .expect("second segment transcript is a string");
    let first_marker = distinguishing_word(first, second);
    let second_marker = distinguishing_word(second, first);

    let server = TestServer::spawn_with(state, Some(runtime));
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(socket.recv_json().await["type"], "stream");
    send_samples_once(&mut socket, &samples).await;
    socket.send_text("stop").await;
    let reply = socket
        .recv_until(Duration::from_secs(240), |frame| frame["type"] == "final")
        .await;
    let final_text = reply["text"]
        .as_str()
        .expect("streaming final transcript is a string");
    let final_words = transcript_words(final_text);
    let first_position = final_words
        .iter()
        .position(|word| word == &first_marker)
        .expect("the streaming final contains the first segment marker");
    let second_position = final_words
        .iter()
        .position(|word| word == &second_marker)
        .expect("the streaming final contains the second segment marker");
    assert!(
        first_position < second_position,
        "the /stt final preserves submitted segment order: {first_marker:?} before \
         {second_marker:?} in {final_text:?}"
    );
    socket.close().await;
    server.shutdown().await;
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
    server.shutdown().await;
}

#[tokio::test]
async fn the_workshop_relay_can_request_private_status_frames() {
    let server = TestServer::spawn();
    let mut request = server
        .ws_url("/stt")
        .into_client_request()
        .expect("request builds");
    request.headers_mut().insert(
        "x-promptforge-workshop-status",
        "1".parse().expect("status header parses"),
    );
    let (mut socket, _response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("socket connects");
    socket
        .send(tungstenite::Message::Text("start".into()))
        .await
        .expect("start sends");
    let stream = socket
        .next()
        .await
        .expect("stream frame arrives")
        .expect("stream frame is valid")
        .into_text()
        .expect("stream frame is text");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stream).expect("stream frame is JSON"),
        json!({"type": "stream", "generation": 1})
    );
    let status = socket
        .next()
        .await
        .expect("status frame arrives")
        .expect("status frame is valid")
        .into_text()
        .expect("status frame is text");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&status).expect("status frame is JSON"),
        json!({
            "type": "workshop_status",
            "label": "Listening...",
            "description": "a push-to-talk take is recording",
            "severity": "info"
        })
    );
    socket.close(None).await.expect("socket closes");
    server.shutdown().await;
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
    server.shutdown().await;
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
    server.shutdown().await;
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
    server.shutdown().await;
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
    server.shutdown().await;
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
    server.shutdown().await;
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
    server.shutdown().await;
}

async fn wait_for_committed(socket: &mut JsonSocket, expected_word: &str) -> String {
    socket
        .recv_until(Duration::from_secs(120), |frame| {
            frame["type"] == "interim"
                && frame["committed"]
                    .as_str()
                    .is_some_and(|text| text.to_lowercase().contains(expected_word))
        })
        .await["committed"]
        .as_str()
        .expect("every interim frame carries a committed string")
        .to_owned()
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn final_model_segments_and_tail_are_authoritative_at_stop() {
    let interim_model = require_model();
    let fixture_dir = tempfile::tempdir().expect("distinct model tempdir");
    let final_model =
        copy_model_replacing_token(&interim_model, fixture_dir.path(), b"country", b"kingdom");
    let (state, runtime) = fixture_runtime_with_models(&interim_model, Some(final_model.as_path()));
    let server = TestServer::spawn_with(state, Some(runtime));
    let mut socket = JsonSocket::connect(&server.ws_url("/stt")).await;
    socket.send_text("start").await;
    assert_eq!(socket.recv_json().await["type"], "stream");
    let samples = jfk_samples();
    send_samples(&mut socket, &samples).await;
    let interim = socket
        .recv_until(Duration::from_secs(90), |frame| {
            frame["type"] == "interim"
                && frame["tentative"]
                    .as_str()
                    .is_some_and(|text| text.to_lowercase().contains("country"))
        })
        .await;
    assert!(
        !interim["tentative"]
            .as_str()
            .expect("interim tentative text is a string")
            .to_lowercase()
            .contains("kingdom"),
        "the provisional transcript comes from the unmodified interim worker"
    );
    send_pcm(&mut socket, 3 * 16_000).await;
    let committed = wait_for_committed(&mut socket, "kingdom").await;
    assert!(
        !committed.to_lowercase().contains("country"),
        "the closed segment comes from the vocabulary-distinguished final worker: {committed:?}"
    );
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
        tail.to_lowercase().contains("kingdom") && !tail.to_lowercase().contains("country"),
        "the tail comes from the vocabulary-distinguished final worker: {text:?}"
    );
    socket.close().await;
    server.shutdown().await;
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn a_disconnected_client_does_not_break_the_next_final_take() {
    let server = fixture_server(true);
    let mut abandoned = JsonSocket::connect(&server.ws_url("/stt")).await;
    abandoned.send_text("start").await;
    assert_eq!(abandoned.recv_json().await["type"], "stream");
    send_samples(&mut abandoned, &jfk_samples()).await;
    send_pcm(&mut abandoned, 3 * EnginePolicy::SAMPLE_RATE).await;
    abandoned.close().await;

    let mut survivor = JsonSocket::connect(&server.ws_url("/stt")).await;
    survivor.send_text("start").await;
    assert_eq!(survivor.recv_json().await["type"], "stream");
    send_samples(&mut survivor, &jfk_samples()).await;
    survivor.send_text("stop").await;
    let reply = survivor
        .recv_until(Duration::from_secs(180), |frame| frame["type"] == "final")
        .await;
    let text = reply["text"].as_str().expect("final text is a string");
    assert!(
        text.to_lowercase().contains("country"),
        "a dropped completion receiver does not poison the shared final worker: {text:?}"
    );
    survivor.close().await;
    server.shutdown().await;
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
    let committed = wait_for_committed(&mut socket, "country").await;
    socket.send_text("stop").await;
    let reply = socket
        .recv_until(Duration::from_secs(180), |frame| frame["type"] == "final")
        .await;
    assert_eq!(
        reply["text"], committed,
        "no uncommitted speech means no tail transcription"
    );
    socket.close().await;
    server.shutdown().await;
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
    server.shutdown().await;
}
