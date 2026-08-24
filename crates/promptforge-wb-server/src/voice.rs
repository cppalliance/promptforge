//! The `/voice` WebSocket endpoint: push-to-talk PCM capture with streaming
//! interim transcription.
//!
//! A client upgrades `GET /voice`, sends the text control message `start`,
//! streams binary messages of little-endian f32 PCM (16 kHz mono), and sends
//! `stop` to end a take. While a take records, an interim loop transcribes
//! the trailing `voice.window_seconds` of audio every `voice.interval_ms`
//! and pushes `{"type":"interim","text":"..."}` text messages; on `stop` the
//! server answers with one `{"type":"final","text":"...","frames":N}` text
//! message holding the best full-window transcript and the total PCM frames
//! received since the most recent `start`. Silent windows are never
//! transcribed (whisper hallucinates on silence), and empty transcripts are
//! never sent as interims. Without a configured `[voice]` model the endpoint
//! still captures and counts PCM, and transcripts come back empty.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};

use crate::app::AppState;
use crate::transcribe::{self, MIN_WINDOW_SAMPLES, VoiceEngine};

/// Session ids for log correlation, handed out in connection order.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// Upgrades a `GET /voice` request to a WebSocket voice-capture session.
pub(crate) async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    let engine = state.voice_engine();
    ws.on_upgrade(move |socket| run_session(session, socket, engine))
}

/// Locks the PCM buffer, recovering from poisoning the way the tape does: a
/// panicking writer cannot leave the session permanently wedged.
fn lock_buffer(buffer: &Mutex<Vec<f32>>) -> MutexGuard<'_, Vec<f32>> {
    buffer.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Copies the trailing interim window out of the shared PCM buffer.
fn window_snapshot(buffer: &Mutex<Vec<f32>>, window_samples: usize) -> Vec<f32> {
    let guard = lock_buffer(buffer);
    transcribe::tail(&guard, window_samples).to_vec()
}

/// Aborts the interim loop, if one is running.
fn stop_interim(interim: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = interim.take() {
        task.abort();
    }
}

/// The interim loop: every `interval`, transcribe the trailing window and
/// push non-empty transcripts to the client. Runs until aborted (on `start`,
/// `stop`, or session end) or until the outbound channel closes.
fn spawn_interim_loop(
    session: u64,
    engine: Arc<VoiceEngine>,
    buffer: Arc<Mutex<Vec<f32>>>,
    out: tokio::sync::mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(engine.interval()).await;
            let window = window_snapshot(&buffer, engine.window_samples());
            if window.len() < MIN_WINDOW_SAMPLES || transcribe::is_silence(&window) {
                continue;
            }
            match engine.transcribe(window).await {
                // `transcribe` returns trimmed text, so empty here means a
                // whitespace-only hallucination: suppress it.
                Ok(text) if text.is_empty() => {}
                Ok(text) => {
                    let message = serde_json::json!({"type": "interim", "text": text}).to_string();
                    if out.send(Message::Text(message.into())).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(session, %error, "interim transcription failed");
                }
            }
        }
    })
}

/// The stop-message transcript: one last pass over the trailing window, or
/// an empty string when there is no engine, the window is silent, or the
/// pass fails (logged; the client still gets its reply).
async fn final_transcript(
    session: u64,
    engine: Option<&VoiceEngine>,
    buffer: &Mutex<Vec<f32>>,
) -> String {
    let Some(engine) = engine else {
        return String::new();
    };
    let window = window_snapshot(buffer, engine.window_samples());
    if transcribe::is_silence(&window) {
        return String::new();
    }
    match engine.transcribe(window).await {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!(session, %error, "final transcription failed");
            String::new()
        }
    }
}

/// Runs one capture session until the socket closes or fails.
async fn run_session(session: u64, socket: WebSocket, engine: Option<Arc<VoiceEngine>>) {
    tracing::info!(session, "voice session opened");
    let (mut sink, mut stream) = socket.split();
    // The receive loop and the interim loop both speak to the client, so
    // outbound messages funnel through one channel into the writer task.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(32);
    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let mut frames = 0u64;
    let mut interim: Option<tokio::task::JoinHandle<()>> = None;

    while let Some(received) = stream.next().await {
        match received {
            Ok(Message::Binary(payload)) => {
                // One PCM frame is a single 16 kHz mono f32 sample: four
                // bytes, little-endian. A trailing partial sample is dropped.
                let samples: Vec<f32> = payload
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| f32::from_le_bytes(*bytes))
                    .collect();
                frames += samples.len() as u64;
                lock_buffer(&buffer).extend_from_slice(&samples);
            }
            Ok(Message::Text(text)) => match text.as_str() {
                "start" => {
                    frames = 0;
                    lock_buffer(&buffer).clear();
                    stop_interim(&mut interim);
                    if let Some(engine) = &engine {
                        interim = Some(spawn_interim_loop(
                            session,
                            Arc::clone(engine),
                            Arc::clone(&buffer),
                            out_tx.clone(),
                        ));
                    }
                    tracing::info!(session, "voice capture started");
                }
                "stop" => {
                    stop_interim(&mut interim);
                    let text = final_transcript(session, engine.as_deref(), &buffer).await;
                    tracing::info!(session, frames, "voice capture stopped");
                    let reply = serde_json::json!({
                        "type": "final",
                        "text": text,
                        "frames": frames,
                    })
                    .to_string();
                    if out_tx.send(Message::Text(reply.into())).await.is_err() {
                        break;
                    }
                }
                _ => {}
            },
            // Pings and pongs are answered by axum itself.
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(Message::Close(_)) => break,
            Err(error) => {
                tracing::warn!(session, %error, "voice session socket failed");
                break;
            }
        }
    }
    stop_interim(&mut interim);
    drop(out_tx);
    writer.abort();
    tracing::info!(session, frames, "voice session closed");
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use tokio_tungstenite::tungstenite;

    use crate::config::{Config, GatewayConfig, ServerConfig, TapeConfig, VoiceConfig};
    use crate::transcribe::fixtures;

    /// Binds the workbench router on a free loopback port with the given
    /// voice configuration and returns the `/voice` WebSocket URL plus the
    /// tempdir keeping the tape alive.
    async fn spawn_voice_server(voice: VoiceConfig) -> (String, tempfile::TempDir) {
        let tape_dir = tempfile::TempDir::new().expect("tempdir");
        let config = Config {
            gateway: GatewayConfig {
                base_url: "http://127.0.0.1:1".to_string(),
                api_key: "k".to_string(),
            },
            tape: TapeConfig {
                path: tape_dir.path().join("tape.jsonl"),
            },
            server: ServerConfig::default(),
            voice,
        };
        let state = AppState::new(&config).expect("state builds in tests");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the voice test server");
        let addr = listener.local_addr().expect("voice test server address");
        tokio::spawn(async move {
            axum::serve(listener, crate::router(state))
                .await
                .expect("voice test server serves");
        });
        (format!("ws://{addr}/voice"), tape_dir)
    }

    /// The engine used by the transcription session tests: an 8 s window
    /// covers the fixture's second "country" (at ~10 s) once the clip is
    /// fully buffered, and a 400 ms cadence keeps the test quick.
    fn test_voice_config() -> VoiceConfig {
        VoiceConfig {
            interim_model: fixtures::require_model(),
            window_seconds: 8,
            interval_ms: 400,
            ..VoiceConfig::default()
        }
    }

    /// Sends one binary message holding `frames` silent f32 PCM samples.
    async fn send_pcm<S>(socket: &mut S, frames: usize)
    where
        S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
    {
        socket
            .send(tungstenite::Message::Binary(vec![0u8; frames * 4].into()))
            .await
            .expect("the PCM block is sent");
    }

    /// Streams f32 samples as binary messages of little-endian bytes.
    async fn send_samples<S>(socket: &mut S, samples: &[f32])
    where
        S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
    {
        const BLOCK: usize = 4096;
        for chunk in samples.chunks(BLOCK) {
            let mut bytes = Vec::with_capacity(chunk.len() * 4);
            for sample in chunk {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            socket
                .send(tungstenite::Message::Binary(bytes.into()))
                .await
                .expect("the PCM block is sent");
        }
    }

    /// Reads one text message from the client socket, failing on anything
    /// else.
    async fn read_text<S>(socket: &mut S) -> String
    where
        S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
    {
        let message = socket
            .next()
            .await
            .expect("a message follows")
            .expect("the message is not a socket error");
        message
            .into_text()
            .expect("the message is text")
            .to_string()
    }

    /// Parses a server text message as JSON.
    fn parse_message(text: &str) -> serde_json::Value {
        serde_json::from_str(text).expect("the server message is JSON")
    }

    #[tokio::test]
    async fn pcm_frames_are_counted_until_stop() {
        let (url, _tape_dir) = spawn_voice_server(VoiceConfig::default()).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        send_pcm(&mut socket, 128).await;
        send_pcm(&mut socket, 64).await;
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");

        let reply = parse_message(&read_text(&mut socket).await);
        assert_eq!(reply["type"], "final");
        assert_eq!(reply["frames"], 192);
        assert_eq!(
            reply["text"], "",
            "no engine is configured, so the transcript is empty"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn start_resets_the_frame_count_for_a_new_take() {
        let (url, _tape_dir) = spawn_voice_server(VoiceConfig::default()).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        send_pcm(&mut socket, 100).await;
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send a second start");
        send_pcm(&mut socket, 10).await;
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");

        let reply = parse_message(&read_text(&mut socket).await);
        assert_eq!(reply["type"], "final");
        assert_eq!(
            reply["frames"], 10,
            "the second take counts only its own frames"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn unknown_text_is_ignored_and_partial_samples_are_dropped() {
        let (url, _tape_dir) = spawn_voice_server(VoiceConfig::default()).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        socket
            .send(tungstenite::Message::Text("bogus".into()))
            .await
            .expect("send an unknown control message");
        send_pcm(&mut socket, 10).await;
        // Three bytes are a trailing partial sample: not a whole f32 frame.
        socket
            .send(tungstenite::Message::Binary(vec![0u8; 3].into()))
            .await
            .expect("send a partial sample");
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");

        let reply = parse_message(&read_text(&mut socket).await);
        assert_eq!(reply["type"], "final");
        assert_eq!(
            reply["frames"], 10,
            "unknown text is ignored and the partial sample is not counted"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn speech_fixture_produces_interim_and_final_transcripts() {
        let (url, _tape_dir) = spawn_voice_server(test_voice_config()).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        send_samples(&mut socket, &fixtures::jfk_samples()).await;

        // Interim passes run until one names the fixture's words; the
        // timeout only bounds a broken pipeline.
        let saw_interim = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let message = parse_message(&read_text(&mut socket).await);
                if message["type"] == "interim"
                    && message["text"]
                        .as_str()
                        .expect("interim text is a string")
                        .to_lowercase()
                        .contains("country")
                {
                    return true;
                }
            }
        })
        .await
        .expect("an interim naming the fixture's words arrives within 90 s");
        assert!(saw_interim);

        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");
        let reply = loop {
            let message = parse_message(&read_text(&mut socket).await);
            if message["type"] == "final" {
                break message;
            }
        };
        let text = reply["text"].as_str().expect("final text is a string");
        assert!(
            text.to_lowercase().contains("country"),
            "the final transcript names the fixture's words: {text:?}"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn silence_produces_no_interims_and_an_empty_final() {
        let (url, _tape_dir) = spawn_voice_server(test_voice_config()).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        // Three seconds of pure zeros: several interim ticks pass over the
        // window, and the silence gate must swallow every one.
        send_pcm(&mut socket, 3 * 16000).await;
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");

        let message = tokio::time::timeout(Duration::from_secs(30), read_text(&mut socket))
            .await
            .expect("the stop reply arrives within 30 s");
        let reply = parse_message(&message);
        assert_eq!(
            reply["type"], "final",
            "the first message after silence is the stop reply, not an interim: {message}"
        );
        assert_eq!(reply["frames"], 48000);
        assert_eq!(
            reply["text"], "",
            "silence transcribes to nothing, not a hallucination"
        );
        socket.close(None).await.expect("close the socket");
    }
}
