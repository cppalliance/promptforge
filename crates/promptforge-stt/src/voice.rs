//! The `/voice` WebSocket endpoint with its existing streaming wire contract.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use promptforge_transcribe::{MIN_WINDOW_SAMPLES, Segmenter, SttEngine, is_silence, tail};
use promptforge_workshop_server::{Activity, Push};
use serde::Serialize;
use tokio::sync::watch;

use crate::runtime::SttState;

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const MIC_PULSE_INTERVAL: Duration = Duration::from_millis(250);
const VOICE_START: &str = "start";
const VOICE_STOP: &str = "stop";

#[derive(Debug, Clone)]
struct VoiceState {
    stt: SttState,
    push: Push,
}

/// Builds the workshop-listener voice routes.
///
/// The routes preserve `/voice` and `/voice/capability`. The shared workshop
/// cross-site guard protects both routes, and the upgrade performs the
/// existing explicit Origin check as a second WebSocket-specific layer.
pub fn routes(stt: SttState, push: Push) -> Router {
    Router::new()
        .route("/voice/capability", get(capability))
        .route("/voice", get(upgrade))
        .route_layer(axum::middleware::from_fn(
            promptforge_workshop_server::cross_site_guard,
        ))
        .with_state(VoiceState { stt, push })
}

async fn capability() -> impl IntoResponse {
    let gpu = promptforge_transcribe::gpu_transcription_available();
    (
        [(header::CONTENT_TYPE, "application/json")],
        format!(r#"{{"gpu":{gpu}}}"#),
    )
}

async fn upgrade(
    State(state): State<VoiceState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !promptforge_workshop_server::origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| run_session(socket, state.stt, state.push))
}

#[derive(Debug, Serialize)]
struct StreamFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    generation: u64,
}

impl StreamFrame {
    fn new(generation: u64) -> Self {
        Self {
            kind: "stream",
            generation,
        }
    }
}

#[derive(Debug, Serialize)]
struct InterimFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    committed: String,
    tentative: String,
    generation: u64,
}

impl InterimFrame {
    fn new(committed: String, tentative: String, generation: u64) -> Self {
        Self {
            kind: "interim",
            committed,
            tentative,
            generation,
        }
    }
}

#[derive(Debug, Serialize)]
struct FinalFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
    frames: u64,
    generation: u64,
}

impl FinalFrame {
    fn new(text: String, frames: u64, generation: u64) -> Self {
        Self {
            kind: "final",
            text,
            frames,
            generation,
        }
    }
}

fn append_transcript(text: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(piece);
}

#[derive(Debug, Default)]
struct Committed {
    text: String,
    segments: Option<std::sync::mpsc::Receiver<String>>,
}

impl Committed {
    fn drain(&mut self) {
        if let Some(segments) = &self.segments {
            while let Ok(text) = segments.try_recv() {
                append_transcript(&mut self.text, &text);
            }
        }
    }
}

#[derive(Debug, Default)]
struct TakeState {
    buffer: Mutex<Vec<f32>>,
    committed: Mutex<Committed>,
    consumed: AtomicUsize,
}

impl TakeState {
    fn lock_buffer(&self) -> MutexGuard<'_, Vec<f32>> {
        self.buffer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_committed(&self) -> MutexGuard<'_, Committed> {
        self.committed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn reset(&self, segments: Option<std::sync::mpsc::Receiver<String>>) {
        self.lock_buffer().clear();
        self.consumed.store(0, Ordering::Relaxed);
        let mut committed = self.lock_committed();
        committed.text.clear();
        committed.segments = segments;
    }

    fn uncommitted_snapshot(&self, consumed: usize, window_samples: usize) -> Vec<f32> {
        let guard = self.lock_buffer();
        let uncommitted = &guard[consumed.min(guard.len())..];
        tail(uncommitted, window_samples).to_vec()
    }
}

#[derive(Debug)]
struct ActiveTake {
    interims: watch::Receiver<Option<String>>,
    _task: InterimTask,
}

#[derive(Debug)]
struct InterimTask(tokio::task::JoinHandle<()>);

impl Drop for InterimTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn next_interim(take: &mut Option<ActiveTake>) -> Option<String> {
    match take.as_mut() {
        Some(active) => match active.interims.changed().await {
            Ok(()) => active.interims.borrow_and_update().clone(),
            Err(_) => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

fn spawn_interim(
    session: u64,
    generation: u64,
    engine: Arc<SttEngine>,
    state: Arc<TakeState>,
    push: Push,
) -> ActiveTake {
    let (interim_tx, interims) = watch::channel(None);
    let task = InterimTask(tokio::spawn(async move {
        let mut last_committed = String::new();
        let mut last_tentative = String::new();
        let mut committed_at_last_speech = String::new();
        loop {
            tokio::time::sleep(engine.interval()).await;
            let committed_text = {
                let mut guard = state.lock_committed();
                guard.drain();
                guard.text.clone()
            };
            let window = state.uncommitted_snapshot(
                state.consumed.load(Ordering::Relaxed),
                engine.window_samples(),
            );
            let tentative = if window.len() < MIN_WINDOW_SAMPLES || is_silence(&window) {
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
                continue;
            }
            if committed_text == last_committed && tentative == last_tentative {
                continue;
            }
            last_committed.clone_from(&committed_text);
            last_tentative.clone_from(&tentative);
            let Ok(message) =
                serde_json::to_string(&InterimFrame::new(committed_text, tentative, generation))
            else {
                continue;
            };
            if interim_tx.send(Some(message)).is_err() {
                return;
            }
        }
    }));
    ActiveTake {
        interims,
        _task: task,
    }
}

async fn final_transcript(
    session: u64,
    engine: &SttEngine,
    state: &TakeState,
    segmenter: &Segmenter,
    push: &Push,
) -> String {
    let window = state.uncommitted_snapshot(segmenter.consumed(), engine.window_samples());
    if window.len() < MIN_WINDOW_SAMPLES || is_silence(&window) {
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

async fn stop_transcript(
    session: u64,
    engine: Option<&SttEngine>,
    state: &TakeState,
    segmenter: &Segmenter,
    push: &Push,
) -> String {
    let Some(engine) = engine else {
        return String::new();
    };
    let tail = {
        let guard = state.lock_buffer();
        guard[segmenter.consumed()..].to_vec()
    };
    let tail = match engine.final_finish(tail).await {
        Some(Ok(text)) => text,
        Some(Err(error)) => {
            push.push_failure("Transcription failed", error.to_string(), Activity::General);
            tracing::warn!(
                session,
                %error,
                "final-pass transcription failed; falling back to the interim model"
            );
            final_transcript(session, engine, state, segmenter, push).await
        }
        None => {
            tracing::info!(
                session,
                "no final model configured; the final pass uses the interim model"
            );
            final_transcript(session, engine, state, segmenter, push).await
        }
    };
    let mut guard = state.lock_committed();
    guard.drain();
    append_transcript(&mut guard.text, &tail);
    guard.text.clone()
}

fn begin_take(
    session: u64,
    generation: u64,
    engine: Option<&Arc<SttEngine>>,
    state: &Arc<TakeState>,
    segmenter: &mut Segmenter,
    push: &Push,
) -> Option<ActiveTake> {
    segmenter.reset();
    let segments = engine
        .filter(|engine| engine.has_final_pass())
        .map(|engine| {
            let (segment_tx, segment_rx) = std::sync::mpsc::channel();
            engine.final_reset(segment_tx);
            segment_rx
        });
    state.reset(segments);
    let take = engine.map(|engine| {
        spawn_interim(
            session,
            generation,
            Arc::clone(engine),
            Arc::clone(state),
            push.clone(),
        )
    });
    push.push_status_update(
        "Listening...",
        "a push-to-talk take is recording",
        Activity::General,
    );
    tracing::info!(session, "voice capture started");
    take
}

fn submit_closed_segments(engine: &SttEngine, state: &TakeState, segmenter: &mut Segmenter) {
    loop {
        let segment = {
            let guard = state.lock_buffer();
            segmenter.poll(&guard).map(|range| guard[range].to_vec())
        };
        match segment {
            Some(samples) => engine.final_submit(samples),
            None => break,
        }
    }
    state
        .consumed
        .store(segmenter.consumed(), Ordering::Relaxed);
    state.lock_committed().drain();
}

async fn send_frame<F: Serialize>(socket: &mut WebSocket, frame: &F) -> bool {
    let Ok(text) = serde_json::to_string(frame) else {
        return true;
    };
    send_text(socket, text).await
}

async fn send_text(socket: &mut WebSocket, text: String) -> bool {
    socket.send(Message::Text(text.into())).await.is_ok()
}

struct SessionClose {
    session: u64,
    push: Push,
}

impl Drop for SessionClose {
    fn drop(&mut self) {
        self.push.push_idle();
        tracing::info!(session = self.session, "voice session closed");
    }
}

struct SessionAudio {
    state: Arc<TakeState>,
    segmenter: Segmenter,
    frames: u64,
    last_mic_pulse: Option<Instant>,
}

impl SessionAudio {
    fn new() -> Self {
        Self {
            state: Arc::new(TakeState::default()),
            segmenter: Segmenter::new(),
            frames: 0,
            last_mic_pulse: None,
        }
    }

    fn receive(&mut self, payload: &[u8], engine: Option<&SttEngine>, push: &Push) {
        let samples: Vec<f32> = payload
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        self.frames += samples.len() as u64;
        self.state.lock_buffer().extend_from_slice(&samples);
        if self
            .last_mic_pulse
            .is_none_or(|at| at.elapsed() >= MIC_PULSE_INTERVAL)
        {
            self.last_mic_pulse = Some(Instant::now());
            push.push_activity(
                "Listening...",
                "microphone audio is arriving",
                Activity::General,
            );
        }
        if let Some(engine) = engine
            && engine.has_final_pass()
        {
            submit_closed_segments(engine, &self.state, &mut self.segmenter);
        }
    }
}

async fn run_session(mut socket: WebSocket, stt: SttState, push: Push) {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    tracing::info!(session, "voice session opened");
    let _closed = SessionClose {
        session,
        push: push.clone(),
    };

    let mut audio = SessionAudio::new();
    let mut take: Option<ActiveTake> = None;
    let mut engine = stt.engine();
    let mut engine_changes = stt.subscribe();
    let mut generation = 0u64;

    loop {
        tokio::select! {
            biased;
            changed = engine_changes.changed() => {
                if changed.is_err() {
                    break;
                }
                take = None;
                audio.state.reset(None);
                audio.segmenter.reset();
                engine = stt.engine();
            }
            interim = next_interim(&mut take) => {
                if let Some(text) = interim
                    && !send_text(&mut socket, text).await
                {
                    break;
                }
            }
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Binary(payload))) => {
                    audio.receive(&payload, engine.as_deref(), &push);
                }
                Some(Ok(Message::Text(text))) => match text.as_str() {
                    VOICE_START => {
                        audio.frames = 0;
                        audio.last_mic_pulse = None;
                        generation += 1;
                        drop(take.take());
                        if !send_frame(&mut socket, &StreamFrame::new(generation)).await {
                            break;
                        }
                        take = begin_take(
                            session,
                            generation,
                            engine.as_ref(),
                            &audio.state,
                            &mut audio.segmenter,
                            &push,
                        );
                    }
                    VOICE_STOP => {
                        take = None;
                        push.push_status_update(
                            "Finalizing transcript...",
                            "the final pass over the take",
                            Activity::General,
                        );
                        let text = stop_transcript(
                            session,
                            engine.as_deref(),
                            &audio.state,
                            &audio.segmenter,
                            &push,
                        )
                        .await;
                        tracing::info!(session, frames = audio.frames, "voice capture stopped");
                        if !send_frame(
                            &mut socket,
                            &FinalFrame::new(text, audio.frames, generation),
                        )
                            .await
                        {
                            break;
                        }
                        push.push_idle();
                    }
                    _ => tracing::debug!(session, "ignoring an unknown voice control message"),
                },
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => {
                    tracing::warn!(session, %error, "voice session socket failed");
                    break;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_frame_serializes_its_generation() {
        let frame = serde_json::to_value(StreamFrame::new(3)).expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "stream",
                "generation": 3,
            })
        );
    }

    #[test]
    fn an_interim_frame_serializes_both_transcript_fields() {
        let frame = serde_json::to_value(InterimFrame::new(
            "ask not".to_owned(),
            "what you".to_owned(),
            1,
        ))
        .expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "interim",
                "committed": "ask not",
                "tentative": "what you",
                "generation": 1,
            })
        );
    }

    #[test]
    fn a_final_frame_serializes_the_transcript_and_the_frame_count() {
        let frame = serde_json::to_value(FinalFrame::new(String::new(), 192, 2))
            .expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "final",
                "text": "",
                "frames": 192,
                "generation": 2,
            })
        );
    }

    #[test]
    fn the_voice_control_messages_are_bare_words() {
        assert_eq!(VOICE_START, "start");
        assert_eq!(VOICE_STOP, "stop");
    }

    #[test]
    fn committed_drain_appends_segments_in_arrival_order() {
        let (segment_tx, segment_rx) = std::sync::mpsc::channel();
        let mut committed = Committed {
            text: String::new(),
            segments: Some(segment_rx),
        };
        segment_tx
            .send("ask not".to_owned())
            .expect("receiver held");
        committed.drain();
        assert_eq!(committed.text, "ask not");
        segment_tx
            .send("what you can do".to_owned())
            .expect("receiver held");
        committed.drain();
        assert_eq!(committed.text, "ask not what you can do");
    }

    #[tokio::test]
    async fn a_lagging_loop_reads_only_the_newest_interim() {
        let (interim_tx, interims) = watch::channel(None);
        let mut take = Some(ActiveTake {
            interims,
            _task: InterimTask(tokio::spawn(std::future::pending::<()>())),
        });
        interim_tx
            .send(Some("old".to_owned()))
            .expect("receiver held");
        interim_tx
            .send(Some("new".to_owned()))
            .expect("receiver held");
        assert_eq!(next_interim(&mut take).await.as_deref(), Some("new"));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), next_interim(&mut take))
                .await
                .is_err()
        );
    }
}
