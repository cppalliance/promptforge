//! The `/voice` WebSocket endpoint: push-to-talk PCM capture with streaming
//! interim transcription and a pipelined final pass.
//!
//! A client upgrades `GET /voice`, sends the text control message `start`,
//! streams binary messages of little-endian f32 PCM (16 kHz mono), and sends
//! `stop` to end a take. Each `start` opens a new stream generation,
//! counted from 1 per connection: the server answers it with a
//! `{"type":"stream","generation":N}` announcement before any of that
//! generation's frames, and tags every interim and final frame with the
//! generation, so the client can discard frames a stop/restart race left
//! behind. While a take records, an interim loop transcribes
//! the take's uncommitted audio every `voice.interval_ms` and pushes
//! `{"type":"interim","committed":"...","tentative":"...","generation":N}`
//! text messages:
//! `committed` is the crystallized prefix (final-pass segment transcripts,
//! append-only within a take) and `tentative` is the interim model's decode
//! of the audio past it. In parallel, an energy-based segmenter
//! ([`crate::segment::Segmenter`]) cuts completed speech segments at
//! silence boundaries and hands them to the final-pass worker, which
//! transcribes them with the `voice.final_model` model in the background,
//! each conditioned on the take's accumulated transcript. On `stop` the
//! worker transcribes the unprocessed tail (its FIFO reply drains every
//! background segment first) and the server answers with one
//! `{"type":"final","text":"...","frames":N,"generation":N}` text
//! message: the take's
//! crystallized committed prefix joined with the tail's own text, plus the
//! total PCM frames received since the most recent `start`. Without a
//! configured final model nothing crystallizes, segmentation stays off so
//! no audio is consumed early, and the final pass falls back to one last
//! interim-model decode of the uncommitted audio (logged); without any
//! `[voice]` model the endpoint still captures and counts PCM, and
//! transcripts come back empty. Silent audio is never transcribed (whisper
//! hallucinates on silence), and an interim frame is sent only when
//! `committed` or `tentative` changed since the last one.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::StreamExt;

use crate::app::AppState;
use crate::protocol::{Activity, FinalFrame, InterimFrame, StreamFrame, VOICE_START, VOICE_STOP};
use crate::push::Push;
use crate::segment::Segmenter;
use crate::transcribe::{self, MIN_WINDOW_SAMPLES, VoiceEngine};
use crate::ws_session::WsSession;

/// Floor between microphone activity pulses: the worklet posts chunks far
/// faster than the status bar can usefully change, so mic activity pulses
/// at 4 Hz rather than per frame.
const MIC_PULSE_INTERVAL: Duration = Duration::from_millis(250);

/// Upgrades a `GET /voice` request to a WebSocket voice-capture session.
pub(crate) async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let engine = state.voice_engine();
    let push = state.push();
    ws.on_upgrade(move |socket| run_session(socket, engine, push))
}

/// Locks the PCM buffer, recovering from poisoning the way the tape does: a
/// panicking writer cannot leave the session permanently wedged.
fn lock_buffer(buffer: &Mutex<Vec<f32>>) -> MutexGuard<'_, Vec<f32>> {
    buffer.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Appends `piece` to `text` with the single-space join the final-pass
/// worker uses between segments; an empty piece changes nothing.
fn append_transcript(text: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(piece);
}

/// One take's crystallized transcript: the segment texts the final-pass
/// worker has reported on the take's channel, joined by single spaces
/// exactly as the worker assembles its own transcript. Shared between the
/// receive loop (drained on each binary message and once at `stop`) and
/// the interim loop (drained each tick, since a segment can finish while
/// no audio arrives), behind the same kind of std mutex as the PCM
/// buffer; no guard ever crosses an `.await`.
#[derive(Debug, Default)]
struct Committed {
    text: String,
    segments: Option<std::sync::mpsc::Receiver<String>>,
}

impl Committed {
    /// Appends every segment text the worker has reported since the last
    /// drain. Append-only within a take.
    fn drain(&mut self) {
        if let Some(segments) = &self.segments {
            while let Ok(text) = segments.try_recv() {
                append_transcript(&mut self.text, &text);
            }
        }
    }
}

/// Locks the committed transcript, recovering from poisoning the way the
/// PCM buffer does.
fn lock_committed(committed: &Mutex<Committed>) -> MutexGuard<'_, Committed> {
    committed.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Copies the take's uncommitted audio - everything past the segmenter's
/// consumed offset - capped to the trailing interim window. Committed
/// audio is never re-decoded by the interim model, and the cap keeps a
/// take whose segments never close (or which has no final pass) from
/// re-decoding its whole length on every pass.
fn uncommitted_snapshot(
    buffer: &Mutex<Vec<f32>>,
    consumed: usize,
    window_samples: usize,
) -> Vec<f32> {
    let guard = lock_buffer(buffer);
    // A take reset can clear the buffer behind a stale offset read by a
    // not-yet-aborted previous interim loop; clamp rather than panic on it.
    let uncommitted = &guard[consumed.min(guard.len())..];
    transcribe::tail(uncommitted, window_samples).to_vec()
}

/// Aborts the interim loop, if one is running.
fn stop_interim(interim: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = interim.take() {
        task.abort();
    }
}

/// The interim loop: every `interval`, drain newly crystallized segments,
/// transcribe the take's uncommitted audio (everything past the segmenter's
/// consumed offset, so a long take does not re-decode its own prefix), and
/// push an interim frame - tagged with the take's stream generation - when
/// either field changed since the last send. Runs until aborted (on
/// `start`, `stop`, or session end) or until the outbound channel closes.
#[expect(
    clippy::too_many_arguments,
    reason = "the take's shared state travels piecemeal so run_session's message loop stays flat"
)]
fn spawn_interim_loop(
    session: u64,
    generation: u64,
    engine: Arc<VoiceEngine>,
    buffer: Arc<Mutex<Vec<f32>>>,
    committed: Arc<Mutex<Committed>>,
    consumed: Arc<AtomicUsize>,
    out: tokio::sync::mpsc::Sender<Message>,
    push: Push,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_committed = String::new();
        let mut last_tentative = String::new();
        // The latch: committed text at the time of the last non-empty
        // tentative. When tentative goes empty (the segmenter advanced
        // past speech but the final worker hasn't crystallized it yet),
        // suppress the frame until committed grows past this snapshot.
        // This prevents the brief empty-display gap between segment
        // close and crystallization.
        let mut committed_at_last_speech = String::new();
        loop {
            tokio::time::sleep(engine.interval()).await;
            let committed_text = {
                let mut guard = lock_committed(&committed);
                guard.drain();
                guard.text.clone()
            };
            let window = uncommitted_snapshot(
                &buffer,
                consumed.load(Ordering::Relaxed),
                engine.window_samples(),
            );
            let tentative = if window.len() < MIN_WINDOW_SAMPLES || transcribe::is_silence(&window)
            {
                String::new()
            } else {
                push.push_activity(
                    "Transcribing...",
                    "an interim pass over the uncommitted audio",
                    Activity::General,
                );
                match engine.transcribe(window).await {
                    Ok(text) => text,
                    Err(error) => {
                        push.push_activity(
                            "Transcription failed",
                            error.to_string(),
                            Activity::General,
                        );
                        tracing::warn!(session, %error, "interim transcription failed");
                        continue;
                    }
                }
            };
            if !tentative.is_empty() {
                committed_at_last_speech.clone_from(&committed_text);
            } else if committed_text.len() <= committed_at_last_speech.len() {
                // Tentative is empty and committed hasn't grown past
                // the snapshot: the final worker hasn't caught up yet.
                // Hold the display at the previous frame.
                continue;
            }
            if committed_text == last_committed && tentative == last_tentative {
                continue;
            }
            last_committed.clone_from(&committed_text);
            last_tentative.clone_from(&tentative);
            // Serializing two strings and an integer cannot fail.
            let Ok(message) =
                serde_json::to_string(&InterimFrame::new(committed_text, tentative, generation))
            else {
                continue;
            };
            if out.send(Message::Text(message.into())).await.is_err() {
                return;
            }
        }
    })
}

/// The stop-message tail when the final model is absent or fails: one last
/// interim-model pass over the take's uncommitted audio, or an empty string
/// when the slice is tiny or silent or the pass fails (logged; the client
/// still gets its reply). The committed prefix is already final, so it is
/// never re-transcribed here.
async fn final_transcript(
    session: u64,
    engine: &VoiceEngine,
    buffer: &Mutex<Vec<f32>>,
    segmenter: &Segmenter,
    push: &Push,
) -> String {
    let window = uncommitted_snapshot(buffer, segmenter.consumed(), engine.window_samples());
    if window.len() < MIN_WINDOW_SAMPLES || transcribe::is_silence(&window) {
        return String::new();
    }
    match engine.transcribe(window).await {
        Ok(text) => text,
        Err(error) => {
            push.push_failure("Transcription failed", error.to_string(), Activity::General);
            tracing::warn!(session, %error, "final transcription failed");
            String::new()
        }
    }
}

/// The stop-message transcript: the take's crystallized committed prefix
/// joined with the closing tail's own text. With a final model configured
/// the tail is queued behind the take's background segments, so awaiting
/// its reply drains them; without one (or when it fails) the tail falls
/// back to the interim-model decode of the uncommitted audio.
async fn stop_transcript(
    session: u64,
    engine: Option<&VoiceEngine>,
    buffer: &Mutex<Vec<f32>>,
    committed: &Mutex<Committed>,
    segmenter: &Segmenter,
    push: &Push,
) -> String {
    let Some(engine) = engine else {
        return String::new();
    };
    let tail = {
        let guard = lock_buffer(buffer);
        guard[segmenter.consumed()..].to_vec()
    };
    let tail = match engine.final_finish(tail).await {
        Some(Ok(text)) => text,
        Some(Err(error)) => {
            push.push_failure("Transcription failed", error.to_string(), Activity::General);
            tracing::warn!(session, %error, "final-pass transcription failed; falling back to the interim model");
            final_transcript(session, engine, buffer, segmenter, push).await
        }
        None => {
            tracing::info!(
                session,
                "no final model configured; the final pass uses the interim model"
            );
            final_transcript(session, engine, buffer, segmenter, push).await
        }
    };
    // Awaiting the tail's reply drained every segment submitted this take
    // (the worker's channel is FIFO), so this crystallizes everything the
    // take reported before the final frame is assembled.
    let mut guard = lock_committed(committed);
    guard.drain();
    append_transcript(&mut guard.text, &tail);
    guard.text.clone()
}

/// Starts a new take: clears the buffer and the committed transcript,
/// resets the segmenter and the final-pass pipeline, installs the take's
/// segment-completion receiver, and spawns the interim loop when an engine
/// is configured. `generation` is the take's announced stream generation,
/// carried by every interim frame the loop pushes.
#[expect(
    clippy::too_many_arguments,
    reason = "the take's shared state travels piecemeal so run_session's message loop stays flat"
)]
fn begin_take(
    session: u64,
    generation: u64,
    engine: Option<&Arc<VoiceEngine>>,
    buffer: &Arc<Mutex<Vec<f32>>>,
    committed: &Arc<Mutex<Committed>>,
    consumed: &Arc<AtomicUsize>,
    segmenter: &mut Segmenter,
    interim: &mut Option<tokio::task::JoinHandle<()>>,
    out: &tokio::sync::mpsc::Sender<Message>,
    push: &Push,
) {
    lock_buffer(buffer).clear();
    segmenter.reset();
    consumed.store(0, Ordering::Relaxed);
    {
        let mut guard = lock_committed(committed);
        guard.text.clear();
        guard.segments = engine
            .filter(|engine| engine.has_final_pass())
            .map(|engine| {
                let (segment_tx, segment_rx) = std::sync::mpsc::channel();
                engine.final_reset(segment_tx);
                segment_rx
            });
    }
    stop_interim(interim);
    if let Some(engine) = engine {
        *interim = Some(spawn_interim_loop(
            session,
            generation,
            Arc::clone(engine),
            Arc::clone(buffer),
            Arc::clone(committed),
            Arc::clone(consumed),
            out.clone(),
            push.clone(),
        ));
    }
    push.push_status_update(
        "Listening...",
        "a push-to-talk take is recording",
        Activity::General,
    );
    tracing::info!(session, "voice capture started");
}

/// Cuts any speech segments the newly arrived audio completed, hands them
/// to the background final pass, publishes the segmenter's consumed offset
/// for the interim loop, and crystallizes the segments the worker has
/// finished since the last message.
fn submit_closed_segments(
    engine: &VoiceEngine,
    buffer: &Arc<Mutex<Vec<f32>>>,
    committed: &Arc<Mutex<Committed>>,
    consumed: &Arc<AtomicUsize>,
    segmenter: &mut Segmenter,
) {
    loop {
        let segment = {
            let guard = lock_buffer(buffer);
            segmenter.poll(&guard).map(|range| guard[range].to_vec())
        };
        match segment {
            Some(samples) => engine.final_submit(samples),
            None => break,
        }
    }
    consumed.store(segmenter.consumed(), Ordering::Relaxed);
    lock_committed(committed).drain();
}

/// Announces a new stream generation on the socket, ahead of every frame
/// belonging to it; a false return means the client is gone.
async fn announce_stream(out: &tokio::sync::mpsc::Sender<Message>, generation: u64) -> bool {
    // Serializing an integer cannot fail. An announcement that somehow
    // cannot serialize is skipped, which is not a gone client.
    let Ok(message) = serde_json::to_string(&StreamFrame::new(generation)) else {
        return true;
    };
    out.send(Message::Text(message.into())).await.is_ok()
}

/// Sends the take's `final` reply, tagged with its stream generation; a
/// false return means the client is gone.
async fn send_final_reply(
    out: &tokio::sync::mpsc::Sender<Message>,
    text: String,
    frames: u64,
    generation: u64,
) -> bool {
    // Serializing a string and two integers cannot fail. A reply that
    // somehow cannot serialize is skipped, which is not a gone client.
    let Ok(reply) = serde_json::to_string(&FinalFrame::new(text, frames, generation)) else {
        return true;
    };
    out.send(Message::Text(reply.into())).await.is_ok()
}

/// Runs one capture session until the socket closes or fails.
async fn run_session(socket: WebSocket, engine: Option<Arc<VoiceEngine>>, push: Push) {
    let (sink, mut stream) = socket.split();
    // The receive loop and the interim loop both speak to the client, so
    // outbound messages funnel through the session's outbox into its
    // writer task.
    let ws = WsSession::new(sink);
    let session = ws.id();
    tracing::info!(session, "voice session opened");

    let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let committed: Arc<Mutex<Committed>> = Arc::new(Mutex::new(Committed::default()));
    // The segmenter's consumed offset, published for the interim loop,
    // which cannot borrow the session-local segmenter.
    let consumed = Arc::new(AtomicUsize::new(0));
    let mut frames = 0u64;
    // The connection's stream generation: 0 until the first `start`, then
    // incremented per take, so every frame names the take it belongs to.
    let mut generation = 0u64;
    let mut interim: Option<tokio::task::JoinHandle<()>> = None;
    let mut segmenter = Segmenter::new();
    let mut last_mic_pulse: Option<Instant> = None;

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
                if last_mic_pulse.is_none_or(|at| at.elapsed() >= MIC_PULSE_INTERVAL) {
                    last_mic_pulse = Some(Instant::now());
                    push.push_activity(
                        "Listening...",
                        "microphone audio is arriving",
                        Activity::General,
                    );
                }
                // Cut any speech segments the new audio completed and hand
                // them to the background final pass. Without a final pass
                // nothing can crystallize, so the segmenter stays off and
                // the interim loop and stop fallback keep seeing the whole
                // take as uncommitted.
                if let Some(engine) = &engine
                    && engine.has_final_pass()
                {
                    submit_closed_segments(engine, &buffer, &committed, &consumed, &mut segmenter);
                }
            }
            Ok(Message::Text(text)) => match text.as_str() {
                VOICE_START => {
                    frames = 0;
                    last_mic_pulse = None;
                    generation += 1;
                    // Announced before begin_take spawns the interim loop,
                    // so the announcement enters the outbox ahead of every
                    // frame of its generation.
                    if !announce_stream(ws.outbox(), generation).await {
                        break;
                    }
                    begin_take(
                        session,
                        generation,
                        engine.as_ref(),
                        &buffer,
                        &committed,
                        &consumed,
                        &mut segmenter,
                        &mut interim,
                        ws.outbox(),
                        &push,
                    );
                }
                VOICE_STOP => {
                    stop_interim(&mut interim);
                    push.push_status_update(
                        "Finalizing transcript...",
                        "the final pass over the take",
                        Activity::General,
                    );
                    let text = stop_transcript(
                        session,
                        engine.as_deref(),
                        &buffer,
                        &committed,
                        &segmenter,
                        &push,
                    )
                    .await;
                    tracing::info!(session, frames, "voice capture stopped");
                    if !send_final_reply(ws.outbox(), text, frames, generation).await {
                        break;
                    }
                    push.push_idle();
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
    push.push_idle();
    ws.close();
    tracing::info!(session, frames, "voice session closed");
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite;

    use crate::config::{Config, GatewayConfig, ServerConfig, TapeConfig, VoiceConfig};
    use crate::transcribe::fixtures;

    /// Binds the workshop router on a free loopback port with the given
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

    /// Reads the stream announcement that answers a `start`, returning
    /// its generation.
    async fn read_stream_generation<S>(socket: &mut S) -> u64
    where
        S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
    {
        let frame = parse_message(&read_text(socket).await);
        assert_eq!(
            frame["type"], "stream",
            "a start is answered by the stream announcement before any other frame"
        );
        frame["generation"]
            .as_u64()
            .expect("every stream frame carries a numeric generation")
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
        read_stream_generation(&mut socket).await;
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
        read_stream_generation(&mut socket).await;
        send_pcm(&mut socket, 100).await;
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send a second start");
        read_stream_generation(&mut socket).await;
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
        read_stream_generation(&mut socket).await;
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
    async fn each_start_announces_its_stream_generation() {
        let (url, _tape_dir) = spawn_voice_server(VoiceConfig::default()).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        assert_eq!(
            read_stream_generation(&mut socket).await,
            1,
            "the connection's first take is generation 1"
        );
        send_pcm(&mut socket, 8).await;
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send a second start");
        assert_eq!(
            read_stream_generation(&mut socket).await,
            2,
            "a restart announces the incremented generation"
        );
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");
        let reply = parse_message(&read_text(&mut socket).await);
        assert_eq!(reply["type"], "final");
        assert_eq!(
            reply["generation"], 2,
            "the final frame carries its take's generation"
        );

        // Generations are per-connection: a fresh socket starts over at 1.
        let (mut second, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect a second /voice socket");
        second
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start on the second socket");
        assert_eq!(
            read_stream_generation(&mut second).await,
            1,
            "a new connection resets the generation"
        );
        socket.close(None).await.expect("close the socket");
        second.close(None).await.expect("close the second socket");
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
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
        // timeout only bounds a broken pipeline. Without a final model
        // nothing crystallizes, so the words ride the tentative field.
        let saw_interim = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let message = parse_message(&read_text(&mut socket).await);
                if message["type"] == "interim"
                    && message["tentative"]
                        .as_str()
                        .expect("interim tentative is a string")
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

    /// Without a final model the segmenter stays off: the trailing silence
    /// would close the speech segment if segmentation ran, advancing the
    /// consumed offset past audio nothing crystallized, and the stop
    /// fallback's uncommitted slice would come back empty. The final reply
    /// must still name the fixture's words.
    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn stop_without_a_final_model_keeps_audio_past_a_silence_gap() {
        let (url, _tape_dir) = spawn_voice_server(test_voice_config()).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        send_samples(&mut socket, &fixtures::jfk_samples()).await;
        send_pcm(&mut socket, 3 * 16000).await;
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");
        let reply = tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let message = parse_message(&read_text(&mut socket).await);
                if message["type"] == "final" {
                    break message;
                }
            }
        })
        .await
        .expect("the final reply arrives within 180 s");
        let text = reply["text"].as_str().expect("final text is a string");
        assert!(
            text.to_lowercase().contains("country"),
            "the fallback decodes the whole take, nothing consumed early: {text:?}"
        );
        socket.close(None).await.expect("close the socket");
    }

    /// The stop path's tail-only finish: two speech segments separated by a
    /// silence gap (the jfk fixture, three seconds of zeros, then the fixture
    /// again). The first segment closes mid-take, crystallizes into the
    /// committed prefix, and is never transcribed again; the second is the
    /// unclosed tail, transcribed once on `stop`, conditioned on the first.
    /// The final frame must be exactly the committed prefix plus the tail's
    /// text, joined by a single space. One pass over the fixture says
    /// "country" twice, so the committed prefix holds two and the frame
    /// adds two more - never a re-transcribed prefix. Both model paths
    /// point at the tiny fixture model; production config selects large-v3
    /// for the final pass.
    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn stop_frame_is_the_committed_prefix_plus_the_tail() {
        let voice = VoiceConfig {
            interim_model: fixtures::require_model(),
            final_model: fixtures::require_model(),
            window_seconds: 8,
            interval_ms: 400,
            ..VoiceConfig::default()
        };
        let (url, _tape_dir) = spawn_voice_server(voice).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        let jfk = fixtures::jfk_samples();
        send_samples(&mut socket, &jfk).await;
        send_pcm(&mut socket, 3 * 16000).await;

        // Wait for the first segment to crystallize into the committed
        // prefix; the timeout only bounds a broken pipeline.
        let committed = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let message = parse_message(&read_text(&mut socket).await);
                if message["type"] != "interim" {
                    continue;
                }
                let committed = message["committed"]
                    .as_str()
                    .expect("every interim frame carries a committed string")
                    .to_string();
                if committed.to_lowercase().contains("country") {
                    break committed;
                }
            }
        })
        .await
        .expect("the first segment crystallizes within 120 s");
        let committed_countries = committed.to_lowercase().matches("country").count();
        assert!(
            committed_countries <= 2,
            "the committed prefix is one pass over the fixture, reported \
             once on the channel ({committed_countries} countries): {committed:?}"
        );

        send_samples(&mut socket, &jfk).await;
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");

        // Interims may interleave; read until the stop reply arrives.
        let reply = tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let message = parse_message(&read_text(&mut socket).await);
                if message["type"] == "final" {
                    break message;
                }
            }
        })
        .await
        .expect("the final reply arrives within 180 s");
        let text = reply["text"].as_str().expect("final text is a string");
        assert!(
            text.starts_with(&committed),
            "the final frame opens with the committed prefix: {text:?}"
        );
        let tail = text[committed.len()..]
            .strip_prefix(' ')
            .expect("a single space joins the committed prefix and the tail");
        assert!(
            tail.to_lowercase().contains("country"),
            "the tail contributed its own text: {text:?}"
        );
        let countries = text.to_lowercase().matches("country").count();
        assert!(
            countries >= 3,
            "both segments are in the final frame ({countries} countries): {text:?}"
        );
        assert!(
            countries <= committed_countries + 2,
            "the tail added at most one pass over the fixture - committed \
             segments were not transcribed again ({committed_countries} \
             committed, {countries} total): {text:?}"
        );
        socket.close(None).await.expect("close the socket");
    }

    /// Stopping right at a segment boundary: the speech has closed and
    /// crystallized, and only the closing silence is uncommitted. The stop
    /// must not transcribe the silent tail (whisper hallucinates on
    /// silence); the final frame is exactly the committed prefix.
    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn stop_at_a_segment_boundary_returns_the_committed_prefix() {
        let voice = VoiceConfig {
            interim_model: fixtures::require_model(),
            final_model: fixtures::require_model(),
            window_seconds: 8,
            interval_ms: 400,
            ..VoiceConfig::default()
        };
        let (url, _tape_dir) = spawn_voice_server(voice).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        let jfk = fixtures::jfk_samples();
        send_samples(&mut socket, &jfk).await;
        send_pcm(&mut socket, 3 * 16000).await;

        // Wait for the segment to close and crystallize; the timeout only
        // bounds a broken pipeline.
        let committed = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let message = parse_message(&read_text(&mut socket).await);
                if message["type"] != "interim" {
                    continue;
                }
                let committed = message["committed"]
                    .as_str()
                    .expect("every interim frame carries a committed string")
                    .to_string();
                if committed.to_lowercase().contains("country") {
                    break committed;
                }
            }
        })
        .await
        .expect("the segment crystallizes within 120 s");

        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");
        let reply = tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let message = parse_message(&read_text(&mut socket).await);
                if message["type"] == "final" {
                    break message;
                }
            }
        })
        .await
        .expect("the final reply arrives within 180 s");
        let text = reply["text"].as_str().expect("final text is a string");
        assert_eq!(
            text, committed,
            "no uncommitted speech means no tail text and no whisper call"
        );
        socket.close(None).await.expect("close the socket");
    }

    /// The drain is the append-only guarantee behind the wire protocol:
    /// segment texts accumulate in arrival order, joined by single spaces
    /// exactly as the final-pass worker assembles its transcript, and a
    /// drain with nothing new changes nothing.
    #[test]
    fn committed_drain_appends_segments_in_arrival_order() {
        let (segment_tx, segment_rx) = std::sync::mpsc::channel();
        let mut committed = Committed {
            text: String::new(),
            segments: Some(segment_rx),
        };
        segment_tx
            .send("ask not".to_string())
            .expect("the receiver is held");
        committed.drain();
        assert_eq!(committed.text, "ask not");
        committed.drain();
        assert_eq!(committed.text, "ask not", "an empty drain changes nothing");
        segment_tx
            .send("what you can do".to_string())
            .expect("the receiver is held");
        committed.drain();
        assert_eq!(
            committed.text, "ask not what you can do",
            "the second segment appended with a single-space join"
        );
    }

    /// The committed/tentative wire protocol: three passes over the speech
    /// fixture separated by silence gaps crystallize two segments mid-take.
    /// Every interim frame must carry both `committed` and `tentative`
    /// strings, `committed` must be append-only across the frames that
    /// arrive, and the final reply - the committed prefix joined with the
    /// tail's own text - must open with the last committed prefix, which a
    /// replace-instead-of-append regression would break.
    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn interim_frames_carry_append_only_committed() {
        let voice = VoiceConfig {
            interim_model: fixtures::require_model(),
            final_model: fixtures::require_model(),
            window_seconds: 8,
            interval_ms: 400,
            ..VoiceConfig::default()
        };
        let (url, _tape_dir) = spawn_voice_server(voice).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        let jfk = fixtures::jfk_samples();
        send_samples(&mut socket, &jfk).await;
        send_pcm(&mut socket, 3 * 16000).await;
        send_samples(&mut socket, &jfk).await;
        send_pcm(&mut socket, 3 * 16000).await;
        send_samples(&mut socket, &jfk).await;

        // Collect interim frames until both segments have crystallized;
        // the timeout only bounds a broken pipeline. Frames arrive only on
        // change, and the tiny worker can finish both segments inside one
        // interim pass, so intermediate committed values are not
        // guaranteed to appear as their own frames. One pass over the
        // fixture says "country" twice, so three or more occurrences prove
        // both segments crystallized.
        let frames = tokio::time::timeout(Duration::from_secs(120), async {
            let mut frames: Vec<String> = Vec::new();
            loop {
                let message = parse_message(&read_text(&mut socket).await);
                if message["type"] != "interim" {
                    continue;
                }
                let committed = message["committed"]
                    .as_str()
                    .expect("every interim frame carries a committed string")
                    .to_string();
                message["tentative"]
                    .as_str()
                    .expect("every interim frame carries a tentative string");
                let crystallized = committed.to_lowercase().matches("country").count();
                frames.push(committed);
                if crystallized >= 3 {
                    break frames;
                }
            }
        })
        .await
        .expect("both segments crystallize into committed within 120 s");

        for pair in frames.windows(2) {
            assert!(
                pair[1].starts_with(&pair[0]),
                "committed is append-only across frames: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
        let committed = frames.last().expect("frames were collected");

        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");
        let reply = tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let message = parse_message(&read_text(&mut socket).await);
                if message["type"] == "final" {
                    break message;
                }
            }
        })
        .await
        .expect("the final reply arrives within 180 s");
        let text = reply["text"].as_str().expect("final text is a string");
        assert!(
            text.starts_with(committed.as_str()),
            "the assembled transcript opens with the committed prefix: {text:?}"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn silence_produces_no_interims_and_an_empty_final() {
        let (url, _tape_dir) = spawn_voice_server(test_voice_config()).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        read_stream_generation(&mut socket).await;
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
