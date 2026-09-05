//! The `/stt` WebSocket endpoint with its existing streaming wire contract.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use gateway_transcribe::{MIN_WINDOW_SAMPLES, SAMPLE_RATE, Segmenter, SttEngine, is_silence, tail};
use serde::Serialize;
use tokio::sync::{mpsc, watch};
use workshop_server::{Activity, Push};

use crate::runtime::SttState;

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const MIC_PULSE_INTERVAL: Duration = Duration::from_millis(250);
const STT_START: &str = "start";
const STT_STOP: &str = "stop";
const WORKSHOP_STATUS_HEADER: &str = "x-promptforge-workshop-status";

#[derive(Debug, Clone)]
struct RouteState {
    stt: SttState,
    reporter: Reporter,
}

#[derive(Debug, Clone)]
enum Reporter {
    Workshop(Push),
    Socket(mpsc::UnboundedSender<String>),
    Silent,
}

#[derive(Debug, Serialize)]
struct RelayedStatusFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    label: String,
    description: String,
    severity: &'static str,
}

impl Reporter {
    fn push_status_update(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        match self {
            Self::Workshop(push) => push.push_status_update(label, description, activity),
            Self::Socket(statuses) => {
                relay_status(statuses, label, description, "info");
            }
            Self::Silent => {}
        }
    }

    fn push_failure(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        match self {
            Self::Workshop(push) => push.push_failure(label, description, activity),
            Self::Socket(statuses) => {
                relay_status(statuses, label, description, "error");
            }
            Self::Silent => {}
        }
    }

    fn push_activity(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        match self {
            Self::Workshop(push) => push.push_activity(label, description, activity),
            Self::Socket(statuses) => {
                relay_status(statuses, label, description, "debug");
            }
            Self::Silent => {}
        }
    }

    fn push_idle(&self) {
        match self {
            Self::Workshop(push) => push.push_idle(),
            Self::Socket(statuses) => {
                relay_status(statuses, "Ready", "idle", "info");
            }
            Self::Silent => {}
        }
    }
}

fn relay_status(
    statuses: &mpsc::UnboundedSender<String>,
    label: impl Into<String>,
    description: impl Into<String>,
    severity: &'static str,
) {
    let frame = RelayedStatusFrame {
        kind: "workshop_status",
        label: label.into(),
        description: description.into(),
        severity,
    };
    if let Ok(message) = serde_json::to_string(&frame) {
        let _ = statuses.send(message);
    }
}

/// Builds the workshop-listener STT routes.
///
/// The routes serve `/stt` and `/stt/capability`. The shared workshop
/// cross-site guard protects both routes, and the upgrade performs the
/// existing explicit Origin check as a second WebSocket-specific layer.
pub fn routes(stt: SttState, push: Push) -> Router {
    routes_with_reporter(stt, Reporter::Workshop(push))
        .route_layer(axum::middleware::from_fn(workshop_server::cross_site_guard))
}

/// Builds the gateway-listener STT routes.
///
/// Session activity is multiplexed as private `workshop_status` frames for
/// the Workshop relay to consume. Its host is responsible for authenticating
/// both routes before merging them.
pub fn gateway_routes(stt: SttState) -> Router {
    routes_with_reporter(stt, Reporter::Silent)
}

fn routes_with_reporter(stt: SttState, reporter: Reporter) -> Router {
    Router::new()
        .route("/stt/capability", get(capability))
        .route("/stt", get(upgrade))
        .with_state(RouteState { stt, reporter })
}

async fn capability(State(state): State<RouteState>) -> impl IntoResponse {
    let engine = state.stt.engine();
    let gpu = engine
        .as_ref()
        .is_some_and(|engine| engine.gpu_transcription_available());
    let engine = engine.is_some();
    (
        [(header::CONTENT_TYPE, "application/json")],
        format!(r#"{{"gpu":{gpu},"engine":{engine}}}"#),
    )
}

async fn upgrade(
    State(state): State<RouteState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !workshop_server::origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let relay_status = headers
        .get(WORKSHOP_STATUS_HEADER)
        .is_some_and(|value| value == "1");
    ws.on_upgrade(move |socket| {
        let (reporter, statuses) = match (state.reporter, relay_status) {
            (Reporter::Silent, true) => {
                let (tx, rx) = mpsc::unbounded_channel();
                (Reporter::Socket(tx), Some(rx))
            }
            (reporter, _) => (reporter, None),
        };
        run_session(socket, state.stt, reporter, statuses)
    })
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

async fn next_status(statuses: &mut Option<mpsc::UnboundedReceiver<String>>) -> Option<String> {
    match statuses {
        Some(statuses) => statuses.recv().await,
        None => std::future::pending().await,
    }
}

fn spawn_interim(
    session: u64,
    generation: u64,
    engine: Arc<SttEngine>,
    state: Arc<TakeState>,
    reporter: Reporter,
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
                reporter.push_activity(
                    "Transcribing...",
                    "an interim pass over the uncommitted audio",
                    Activity::General,
                );
                match engine.transcribe(window).await {
                    Ok(text) => text,
                    Err(error) => {
                        reporter.push_activity(
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
    reporter: &Reporter,
) -> String {
    let window = state.uncommitted_snapshot(segmenter.consumed(), engine.window_samples());
    if window.len() < MIN_WINDOW_SAMPLES || is_silence(&window) {
        return String::new();
    }
    match engine.transcribe(window).await {
        Ok(text) => text,
        Err(error) => {
            reporter.push_failure("Transcription failed", error.to_string(), Activity::General);
            tracing::warn!(session, %error, "final transcription failed");
            String::new()
        }
    }
}

/// The dropped leading samples when a take's uncommitted audio exceeds one
/// interim window, or `None` when the whole take fits.
fn truncation_drop(uncommitted: usize, window_samples: usize) -> Option<usize> {
    if uncommitted > window_samples {
        Some(uncommitted - window_samples)
    } else {
        None
    }
}

/// The status-bar description of one truncation: the window length and the
/// dropped lead, both in seconds (the lead to a truncated tenth).
fn truncation_message(window_samples: usize, dropped: usize) -> String {
    format!(
        "the take ran past the {} s interim window with no final transcription, so its first {}.{} s were dropped",
        window_samples / SAMPLE_RATE,
        dropped / SAMPLE_RATE,
        dropped % SAMPLE_RATE * 10 / SAMPLE_RATE,
    )
}

/// The interim-window fallback transcribes only the take's last window of
/// audio; a longer take loses its leading audio. Name the truncation on the
/// status bar and in the log instead of dropping it silently.
fn warn_if_truncated(
    session: u64,
    engine: &SttEngine,
    state: &TakeState,
    segmenter: &Segmenter,
    reporter: &Reporter,
) {
    let uncommitted = {
        let guard = state.lock_buffer();
        guard.len().saturating_sub(segmenter.consumed())
    };
    let window = engine.window_samples();
    let Some(dropped) = truncation_drop(uncommitted, window) else {
        return;
    };
    tracing::warn!(
        session,
        dropped_samples = dropped,
        window_samples = window,
        "take exceeded the interim window; leading audio dropped from the transcript"
    );
    reporter.push_failure(
        "Transcript truncated",
        truncation_message(window, dropped),
        Activity::General,
    );
}

async fn stop_transcript(
    session: u64,
    engine: Option<&SttEngine>,
    state: &TakeState,
    segmenter: &Segmenter,
    reporter: &Reporter,
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
            reporter.push_failure("Transcription failed", error.to_string(), Activity::General);
            tracing::warn!(
                session,
                %error,
                "final-pass transcription failed; falling back to the interim model"
            );
            warn_if_truncated(session, engine, state, segmenter, reporter);
            final_transcript(session, engine, state, segmenter, reporter).await
        }
        None => {
            tracing::info!(
                session,
                "no final model configured; the final pass uses the interim model"
            );
            warn_if_truncated(session, engine, state, segmenter, reporter);
            final_transcript(session, engine, state, segmenter, reporter).await
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
    reporter: &Reporter,
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
            reporter.clone(),
        )
    });
    reporter.push_status_update(
        "Listening...",
        "a push-to-talk take is recording",
        Activity::General,
    );
    tracing::info!(session, "stt capture started");
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
    reporter: Reporter,
}

impl Drop for SessionClose {
    fn drop(&mut self) {
        self.reporter.push_idle();
        tracing::info!(session = self.session, "stt session closed");
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

    fn receive(&mut self, payload: &[u8], engine: Option<&SttEngine>, reporter: &Reporter) {
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
            reporter.push_activity(
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

async fn run_session(
    mut socket: WebSocket,
    stt: SttState,
    reporter: Reporter,
    mut statuses: Option<mpsc::UnboundedReceiver<String>>,
) {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    tracing::info!(session, "stt session opened");
    let _closed = SessionClose {
        session,
        reporter: reporter.clone(),
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
            status = next_status(&mut statuses) => {
                if let Some(text) = status
                    && !send_text(&mut socket, text).await
                {
                    break;
                }
            }
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Binary(payload))) => {
                    audio.receive(&payload, engine.as_deref(), &reporter);
                }
                Some(Ok(Message::Text(text))) => match text.as_str() {
                    STT_START => {
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
                            &reporter,
                        );
                    }
                    STT_STOP => {
                        take = None;
                        reporter.push_status_update(
                            "Finalizing transcript...",
                            "the final pass over the take",
                            Activity::General,
                        );
                        let text = stop_transcript(
                            session,
                            engine.as_deref(),
                            &audio.state,
                            &audio.segmenter,
                            &reporter,
                        )
                        .await;
                        tracing::info!(session, frames = audio.frames, "stt capture stopped");
                        if !send_frame(
                            &mut socket,
                            &FinalFrame::new(text, audio.frames, generation),
                        )
                            .await
                        {
                            break;
                        }
                        reporter.push_idle();
                    }
                    _ => tracing::debug!(session, "ignoring an unknown stt control message"),
                },
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => {
                    tracing::warn!(session, %error, "stt session socket failed");
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
    fn the_stt_control_messages_are_bare_words() {
        assert_eq!(STT_START, "start");
        assert_eq!(STT_STOP, "stop");
    }

    #[test]
    fn truncation_starts_past_the_window() {
        let window = 15 * SAMPLE_RATE;
        assert_eq!(truncation_drop(0, window), None);
        assert_eq!(truncation_drop(window, window), None);
        assert_eq!(truncation_drop(window + 1, window), Some(1));
        assert_eq!(
            truncation_drop(20 * SAMPLE_RATE, window),
            Some(5 * SAMPLE_RATE)
        );
    }

    #[test]
    fn the_truncation_message_names_the_window_and_the_dropped_lead() {
        let message = truncation_message(15 * SAMPLE_RATE, 5 * SAMPLE_RATE);
        assert!(message.contains("15 s"), "{message}");
        assert!(message.contains("5.0 s"), "{message}");
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
