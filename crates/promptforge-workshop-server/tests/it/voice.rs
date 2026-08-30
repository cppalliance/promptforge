//! Characterization tests for the `/voice` socket: stream announcements
//! with per-take generations, PCM frame counting through the final reply,
//! and - with the whisper fixtures - interim and final transcripts.

use std::time::Duration;

use promptforge_workshop_server::VoiceConfig;
use promptforge_workshop_server::fixtures::{jfk_samples, require_model};
use serde_json::json;

use crate::common::{JsonSocket, TestServer};

/// A gateway address nothing listens on: `/voice` never talks upstream.
const NO_GATEWAY: &str = "http://127.0.0.1:1";

/// Sends one binary message holding `frames` silent f32 PCM samples.
async fn send_pcm(socket: &mut JsonSocket, frames: usize) {
    socket.send_binary(vec![0u8; frames * 4]).await;
}

/// Streams f32 samples as binary messages of little-endian bytes.
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

#[tokio::test]
async fn a_take_counts_pcm_frames_and_tags_the_final_with_its_generation() {
    let server = TestServer::spawn(NO_GATEWAY);
    let mut socket = JsonSocket::connect(&server.ws_url("/voice")).await;
    socket.send_text("start").await;
    assert_eq!(
        socket.recv_json().await,
        json!({"type": "stream", "generation": 1}),
        "a start is answered by the stream announcement before any other frame"
    );
    send_pcm(&mut socket, 128).await;
    send_pcm(&mut socket, 64).await;
    // Three bytes are a trailing partial sample: not a whole f32 frame.
    socket.send_binary(vec![0u8; 3]).await;
    socket.send_text("stop").await;

    assert_eq!(
        socket.recv_json().await,
        json!({"type": "final", "text": "", "frames": 192, "generation": 1}),
        "frames are counted, the partial sample is dropped, and no engine \
         means an empty transcript"
    );
    socket.close().await;
}

#[tokio::test]
async fn a_restart_increments_the_generation_and_a_new_connection_resets_it() {
    let server = TestServer::spawn(NO_GATEWAY);
    let mut socket = JsonSocket::connect(&server.ws_url("/voice")).await;
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

    let mut second = JsonSocket::connect(&server.ws_url("/voice")).await;
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
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn speech_produces_generation_tagged_interim_and_final_frames() {
    let voice = VoiceConfig {
        interim_model: require_model(),
        window_seconds: 8,
        interval_ms: 400,
        ..VoiceConfig::default()
    };
    let server = TestServer::spawn_with_voice(NO_GATEWAY, voice);
    // The engine load is deferred to the provisioning task, and a /voice
    // session captures the engine at upgrade time, so the take must wait
    // for the load's completion frame.
    let mut status = JsonSocket::connect(&server.ws_url("/ws")).await;
    status
        .recv_until(Duration::from_secs(90), |frame| {
            frame["type"] == "status" && frame["label"] == "Voice ready"
        })
        .await;
    status.close().await;
    let mut socket = JsonSocket::connect(&server.ws_url("/voice")).await;
    socket.send_text("start").await;
    assert_eq!(
        socket.recv_json().await,
        json!({"type": "stream", "generation": 1})
    );
    send_samples(&mut socket, &jfk_samples()).await;

    // Interim passes run until one names the fixture's words; the timeout
    // only bounds a broken pipeline. Without a final model nothing
    // crystallizes, so the words ride the tentative field.
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
