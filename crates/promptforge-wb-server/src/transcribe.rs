//! Whisper transcription on dedicated worker threads.
//!
//! [`VoiceEngine`] owns two worker threads: the interim worker holds the
//! streaming model and transcribes sliding windows, and the final-pass
//! worker ([`FinalTranscriber`], present when `[voice].final_model` is
//! configured) holds the larger model and transcribes completed speech
//! segments in the background while the user is still talking. Callers hand
//! owned sample buffers through channels and await transcripts on oneshots,
//! so the blocking CPU-bound inference never touches the tokio executor.
//! The pure helpers ([`rms`], [`is_silence`], [`tail`]) are the session's
//! silence gate: whisper hallucinates plausible text on silent input, so
//! quiet windows are never sent to the model.

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::config::VoiceConfig;

/// PCM sample rate the voice wire format and whisper both require.
pub(crate) const SAMPLE_RATE: usize = 16_000;

/// Windows below this RMS are treated as silence and never transcribed.
///
/// 0.001 is -60 dBFS: above the noise floor of a browser-suppressed mic
/// stream, far below conversational speech (typically 0.02 and up).
pub(crate) const SILENCE_RMS: f64 = 0.001;

/// Minimum audio the interim loop bothers to transcribe; shorter fragments
/// decode to garbage often enough that gating them is cheaper than filtering
/// their output.
pub(crate) const MIN_WINDOW_SAMPLES: usize = SAMPLE_RATE / 2;

/// Maximum conditioning prompt handed to the final pass, in chars. Whisper
/// keeps at most half its text context for the prompt (224 tokens), and
/// four chars per token is a conservative English estimate; the tail of the
/// accumulated transcript is what matters for continuity, so the cap trims
/// from the front.
const MAX_PROMPT_CHARS: usize = 800;

/// Root-mean-square amplitude of a PCM buffer.
#[expect(
    clippy::cast_precision_loss,
    reason = "audio buffers are far below 2^53 samples"
)]
pub(crate) fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (energy / samples.len() as f64).sqrt()
}

/// Returns true when the buffer is quiet enough that whisper would
/// hallucinate rather than transcribe.
pub(crate) fn is_silence(samples: &[f32]) -> bool {
    rms(samples) < SILENCE_RMS
}

/// Returns the trailing `window` samples of `buffer`, or the whole buffer
/// when it is shorter than the window.
pub(crate) fn tail(buffer: &[f32], window: usize) -> &[f32] {
    &buffer[buffer.len().saturating_sub(window)..]
}

/// The per-server voice engine: the interim and final-pass whisper workers
/// plus the interim loop's window and cadence, built once at startup from
/// `[voice]` in `workbench.toml`.
#[derive(Debug)]
pub(crate) struct VoiceEngine {
    transcriber: Transcriber,
    final_pass: Option<FinalTranscriber>,
    window_samples: usize,
    interval: Duration,
}

impl VoiceEngine {
    /// Loads the interim model, and the final model when configured, each
    /// onto a fresh worker thread.
    ///
    /// # Errors
    /// Returns [`TranscribeError::InvalidConfig`] when the window or interval
    /// is zero, [`TranscribeError::LoadModel`] when a model file cannot be
    /// loaded, and [`TranscribeError::SpawnWorker`] when a worker thread
    /// cannot be started.
    pub(crate) fn new(config: &VoiceConfig) -> Result<Self, TranscribeError> {
        if config.window_seconds == 0 {
            return Err(TranscribeError::InvalidConfig(
                "voice.window_seconds must be at least 1".to_string(),
            ));
        }
        if config.interval_ms == 0 {
            return Err(TranscribeError::InvalidConfig(
                "voice.interval_ms must be at least 1".to_string(),
            ));
        }
        let window_seconds = usize::try_from(config.window_seconds).map_err(|_| {
            TranscribeError::InvalidConfig("voice.window_seconds is too large".to_string())
        })?;
        let Some(window_samples) = window_seconds.checked_mul(SAMPLE_RATE) else {
            return Err(TranscribeError::InvalidConfig(
                "voice.window_seconds is too large".to_string(),
            ));
        };
        let transcriber = Transcriber::load(&config.interim_model)?;
        let final_pass = if config.final_model.as_os_str().is_empty() {
            None
        } else {
            Some(FinalTranscriber::load(&config.final_model)?)
        };
        Ok(Self {
            transcriber,
            final_pass,
            window_samples,
            interval: Duration::from_millis(config.interval_ms),
        })
    }

    /// Whether the final pass is configured. Segmentation and
    /// crystallization only happen when it is: without it nothing can
    /// crystallize, so the segmenter must not consume audio the interim
    /// model still needs.
    pub(crate) fn has_final_pass(&self) -> bool {
        self.final_pass.is_some()
    }

    /// Whether the final pass is absent. A test seam for the startup
    /// degradation policy, which drops an unsourced missing final model.
    #[cfg(test)]
    pub(crate) fn final_pass_absent_for_test(&self) -> bool {
        !self.has_final_pass()
    }

    /// Samples in the sliding interim window.
    pub(crate) fn window_samples(&self) -> usize {
        self.window_samples
    }

    /// Cadence of the interim loop.
    pub(crate) fn interval(&self) -> Duration {
        self.interval
    }

    /// Transcribes one 16 kHz mono f32 buffer, returning the trimmed text.
    ///
    /// # Errors
    /// Returns [`TranscribeError::Inference`] when the model rejects the
    /// audio and [`TranscribeError::WorkerGone`] when the worker thread has
    /// exited.
    pub(crate) async fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        self.transcriber.transcribe(samples).await
    }

    /// Starts a new take on the final-pass worker, discarding the previous
    /// take's accumulated transcript and installing `on_segment` as the
    /// take's completion channel: each background segment's text is sent on
    /// it as the segment finishes. A no-op without a final model.
    pub(crate) fn final_reset(&self, on_segment: std::sync::mpsc::Sender<String>) {
        if let Some(final_pass) = &self.final_pass {
            final_pass.reset(on_segment);
        }
    }

    /// Queues a completed speech segment for background final-pass
    /// transcription, conditioned on the take's accumulated transcript. A
    /// no-op without a final model.
    pub(crate) fn final_submit(&self, samples: Vec<f32>) {
        if let Some(final_pass) = &self.final_pass {
            final_pass.submit(samples);
        }
    }

    /// Queues the take's unprocessed tail and awaits the tail's own
    /// transcription - not the take's full assembled transcript, which the
    /// session already holds as crystallized segment text. The text is
    /// empty when the tail is silent or too short to decode (the worker
    /// skips those rather than hallucinating). Returns `None` when no
    /// final model is configured and the caller should fall back to the
    /// interim model.
    ///
    /// # Errors
    /// Returns [`TranscribeError::Inference`] when the model rejects the
    /// audio and [`TranscribeError::WorkerGone`] when the worker thread has
    /// exited.
    pub(crate) async fn final_finish(
        &self,
        samples: Vec<f32>,
    ) -> Option<Result<String, TranscribeError>> {
        match &self.final_pass {
            None => None,
            Some(final_pass) => Some(final_pass.finish(samples).await),
        }
    }
}

/// Shared holder for the voice engine: empty until the engine loads, then
/// filled exactly once - at startup from local model files, or later by the
/// provisioning task once the gateway cache has provided them.
///
/// Reads happen per `/voice` session upgrade and writes are one-shot, so a
/// std `RwLock` suffices; no guard ever crosses an `.await`. Lock poisoning
/// recovers the value, matching the tape's posture: a panicking writer
/// cannot wedge voice for the process's life.
#[derive(Debug, Clone, Default)]
pub(crate) struct VoiceSlot {
    engine: Arc<RwLock<Option<Arc<VoiceEngine>>>>,
}

impl VoiceSlot {
    /// The engine, when it has loaded.
    pub(crate) fn engine(&self) -> Option<Arc<VoiceEngine>> {
        self.engine
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the engine has loaded.
    pub(crate) fn is_active(&self) -> bool {
        self.engine
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Installs a loaded engine.
    pub(crate) fn activate(&self, engine: VoiceEngine) {
        *self.engine.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(engine));
    }
}

/// One transcription request handed to the worker thread.
struct Job {
    samples: Vec<f32>,
    reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
}

/// Handle to the whisper worker thread.
#[derive(Debug)]
pub(crate) struct Transcriber {
    job_tx: std::sync::mpsc::Sender<Job>,
}

impl Transcriber {
    /// Spawns the worker thread and blocks until the model is loaded or the
    /// load fails.
    ///
    /// # Errors
    /// Returns [`TranscribeError::LoadModel`] when the model file cannot be
    /// loaded and [`TranscribeError::SpawnWorker`] when the thread cannot be
    /// started.
    fn load(model_path: &Path) -> Result<Self, TranscribeError> {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let path = model_path.to_path_buf();
        std::thread::Builder::new()
            .name("whisper-transcribe".to_string())
            .spawn(move || worker_loop(&path, &job_rx, &init_tx))
            .map_err(TranscribeError::SpawnWorker)?;
        init_rx.recv().map_err(|_| TranscribeError::WorkerGone)??;
        Ok(Self { job_tx })
    }

    /// Queues `samples` for transcription and awaits the trimmed text.
    async fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        self.job_tx
            .send(Job { samples, reply })
            .map_err(|_| TranscribeError::WorkerGone)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }
}

/// The worker thread's body: load the model, then transcribe jobs in arrival
/// order until every sender is dropped.
fn worker_loop(
    path: &Path,
    job_rx: &std::sync::mpsc::Receiver<Job>,
    init_tx: &std::sync::mpsc::SyncSender<Result<(), TranscribeError>>,
) {
    let Some((_ctx, mut state)) = load_state(path, init_tx) else {
        return;
    };
    while let Ok(job) = job_rx.recv() {
        // The receiver may be gone (session closed mid-pass); the transcript
        // is computed anyway and the send failure ignored.
        let _ = job
            .reply
            .send(transcribe_blocking(&mut state, &job.samples, None, true));
    }
}

/// Loads a whisper context and state from `path`, reporting the outcome on
/// `init_tx` (which the spawning `load` blocks on). Returns `None` after
/// reporting a failure, or when the spawner is already gone.
fn load_state(
    path: &Path,
    init_tx: &std::sync::mpsc::SyncSender<Result<(), TranscribeError>>,
) -> Option<(WhisperContext, whisper_rs::WhisperState)> {
    let loaded = WhisperContext::new_with_params(path, WhisperContextParameters::default())
        .map_err(|source| TranscribeError::LoadModel {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
        .and_then(|ctx| {
            ctx.create_state()
                .map(|state| (ctx, state))
                .map_err(|source| TranscribeError::LoadModel {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                })
        });
    match loaded {
        Ok(pair) => {
            let _ = init_tx.send(Ok(()));
            Some(pair)
        }
        Err(error) => {
            let _ = init_tx.send(Err(error));
            None
        }
    }
}

/// The trailing `MAX_PROMPT_CHARS` chars of `prompt` with null bytes
/// stripped: whisper's prompt buffer is bounded, and `set_initial_prompt`
/// panics on null bytes, which a model transcript could in principle
/// contain.
fn sanitize_prompt(prompt: &str) -> String {
    let cleaned: String = prompt.chars().filter(|&c| c != '\0').collect();
    let mut start = cleaned.len().saturating_sub(MAX_PROMPT_CHARS);
    while !cleaned.is_char_boundary(start) {
        start += 1;
    }
    cleaned[start..].to_string()
}

/// Runs one blocking whisper pass over `samples` and concatenates the
/// segments. `prompt`, when non-empty after sanitizing, conditions the
/// decoder on the take's transcript so far; `single_segment` forces the
/// whole buffer into one decoding pass (the interim sliding-window case).
fn transcribe_blocking(
    state: &mut whisper_rs::WhisperState,
    samples: &[f32],
    prompt: Option<&str>,
    single_segment: bool,
) -> Result<String, TranscribeError> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_translate(false);
    // Decoder state never carries across passes: conditioning travels only
    // through the explicit prompt, or a hallucination would compound.
    params.set_no_context(true);
    params.set_single_segment(single_segment);
    params.set_no_timestamps(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    if let Some(prompt) = prompt {
        let prompt = sanitize_prompt(prompt);
        if !prompt.is_empty() {
            params.set_initial_prompt(&prompt);
        }
    }
    state
        .full(params, samples)
        .map_err(|source| TranscribeError::Inference(Box::new(source)))?;
    let mut text = String::new();
    for segment in state.as_iter() {
        let piece = segment
            .to_str_lossy()
            .map_err(|source| TranscribeError::Inference(Box::new(source)))?;
        text.push_str(&piece);
    }
    Ok(text.trim().to_string())
}

/// One take's final-pass state: the large model's whisper state plus the
/// take's accumulated transcript, which conditions each new segment so
/// domain vocabulary and phrasing survive segmentation.
#[derive(Debug)]
pub(crate) struct FinalPass {
    state: whisper_rs::WhisperState,
    /// Every segment transcript so far, joined by single spaces.
    transcript: String,
    /// The conditioning prompt used on the most recent segment, kept so
    /// tests can observe that conditioning actually happened.
    last_prompt: String,
}

impl FinalPass {
    /// Loads the final model from `path`.
    ///
    /// # Errors
    /// Returns [`TranscribeError::LoadModel`] when the model file cannot be
    /// loaded.
    fn load(path: &Path) -> Result<Self, TranscribeError> {
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let Some((_ctx, state)) = load_state(path, &init_tx) else {
            return match init_rx.recv() {
                Ok(Err(error)) => Err(error),
                // `load_state` reports every outcome on the channel before
                // returning `None`, so a disconnected or Ok(()) result here
                // means the invariant broke, not a new failure mode.
                _ => Err(TranscribeError::WorkerGone),
            };
        };
        Ok(Self {
            state,
            transcript: String::new(),
            last_prompt: String::new(),
        })
    }

    /// Forgets the previous take's transcript for a new take.
    fn reset(&mut self) {
        self.transcript.clear();
        self.last_prompt.clear();
    }

    /// The conditioning prompt the most recent segment was transcribed with.
    #[cfg(test)]
    pub(crate) fn last_prompt(&self) -> &str {
        &self.last_prompt
    }

    /// The take's accumulated transcript: every segment so far, joined by
    /// single spaces. A test-only observation point for the conditioning
    /// chain; the workers consume only each segment's own text.
    #[cfg(test)]
    pub(crate) fn transcript(&self) -> &str {
        &self.transcript
    }

    /// Transcribes one segment conditioned on the accumulated transcript,
    /// appends the result, and returns the segment's own text. Silent or
    /// tiny fragments are skipped (whisper hallucinates on them): the
    /// accumulated transcript is left unchanged and `None` comes back.
    ///
    /// # Errors
    /// Returns [`TranscribeError::Inference`] when the model rejects the
    /// audio; the accumulated transcript is left unchanged.
    fn transcribe_segment(&mut self, samples: &[f32]) -> Result<Option<String>, TranscribeError> {
        let mut segment = None;
        if samples.len() >= MIN_WINDOW_SAMPLES && !is_silence(samples) {
            let prompt = self.transcript.clone();
            let text = transcribe_blocking(&mut self.state, samples, Some(&prompt), false)?;
            if !text.is_empty() {
                if !self.transcript.is_empty() {
                    self.transcript.push(' ');
                }
                self.transcript.push_str(&text);
                segment = Some(text);
            }
            self.last_prompt = prompt;
        }
        Ok(segment)
    }
}

/// A command for the final-pass worker thread.
enum FinalJob {
    /// Start a new take, discarding the accumulated transcript and
    /// installing the take's segment-completion channel.
    Reset {
        on_segment: std::sync::mpsc::Sender<String>,
    },
    /// Transcribe a completed segment (or the closing tail) and reply with
    /// the segment's own text, empty when the fragment was skipped.
    /// `notify` marks a background submit, whose segment text is also sent
    /// on the take's channel; the closing tail reports only through its
    /// reply.
    Segment {
        samples: Vec<f32>,
        reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
        notify: bool,
    },
}

/// Handle to the final-pass worker thread: the large model transcribing
/// completed segments in the background while a take records.
#[derive(Debug)]
pub(crate) struct FinalTranscriber {
    job_tx: std::sync::mpsc::Sender<FinalJob>,
}

impl FinalTranscriber {
    /// Spawns the worker thread and blocks until the model is loaded or the
    /// load fails.
    ///
    /// # Errors
    /// Returns [`TranscribeError::LoadModel`] when the model file cannot be
    /// loaded and [`TranscribeError::SpawnWorker`] when the thread cannot be
    /// started.
    fn load(model_path: &Path) -> Result<Self, TranscribeError> {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<FinalJob>();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let path = model_path.to_path_buf();
        std::thread::Builder::new()
            .name("whisper-final".to_string())
            .spawn(move || final_worker_loop(&path, &job_rx, &init_tx))
            .map_err(TranscribeError::SpawnWorker)?;
        init_rx.recv().map_err(|_| TranscribeError::WorkerGone)??;
        Ok(Self { job_tx })
    }

    /// Starts a new take, installing `on_segment` as the channel each
    /// background segment's text is reported on. If the worker is gone the
    /// next `finish` reports it.
    fn reset(&self, on_segment: std::sync::mpsc::Sender<String>) {
        let _ = self.job_tx.send(FinalJob::Reset { on_segment });
    }

    /// Queues a completed segment for background transcription; the
    /// segment's text is reported on the take's channel.
    fn submit(&self, samples: Vec<f32>) {
        let (reply, _dropped) = tokio::sync::oneshot::channel();
        let _ = self.job_tx.send(FinalJob::Segment {
            samples,
            reply,
            notify: true,
        });
    }

    /// Queues the take's tail and awaits the tail's own text, empty when
    /// the tail was skipped. Because the channel is FIFO, awaiting this
    /// reply also drains every segment submitted earlier in the take.
    async fn finish(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        self.job_tx
            .send(FinalJob::Segment {
                samples,
                reply,
                notify: false,
            })
            .map_err(|_| TranscribeError::WorkerGone)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }
}

/// The final-pass worker's body: load the model, then process takes' jobs in
/// arrival order until every sender is dropped.
fn final_worker_loop(
    path: &Path,
    job_rx: &std::sync::mpsc::Receiver<FinalJob>,
    init_tx: &std::sync::mpsc::SyncSender<Result<(), TranscribeError>>,
) {
    let mut pass = match FinalPass::load(path) {
        Ok(pass) => {
            let _ = init_tx.send(Ok(()));
            pass
        }
        Err(error) => {
            let _ = init_tx.send(Err(error));
            return;
        }
    };
    // The current take's completion channel, installed by each `Reset`;
    // FIFO job order guarantees a take's segments all precede the next
    // take's `Reset`, so a segment can never land on the wrong channel.
    let mut on_segment: Option<std::sync::mpsc::Sender<String>> = None;
    while let Ok(job) = job_rx.recv() {
        match job {
            FinalJob::Reset {
                on_segment: channel,
            } => {
                on_segment = Some(channel);
                pass.reset();
            }
            FinalJob::Segment {
                samples,
                reply,
                notify,
            } => {
                let result = pass.transcribe_segment(&samples);
                match &result {
                    Ok(segment) => {
                        if notify && let (Some(channel), Some(text)) = (&on_segment, segment) {
                            // A gone session (socket closed mid-take) is
                            // ordinary; the transcript was computed anyway.
                            if channel.send(text.clone()).is_err() {
                                tracing::debug!("segment completion receiver is gone");
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "final-pass segment transcription failed");
                    }
                }
                // A dropped receiver (a background segment, or a session
                // closed mid-take) is fine: the transcript was computed.
                let _ = reply.send(result.map(Option::unwrap_or_default));
            }
        }
    }
}

/// A voice-engine construction or transcription failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TranscribeError {
    /// The whisper model file could not be loaded.
    #[non_exhaustive]
    #[error("load whisper model {}", path.display())]
    LoadModel {
        /// The model path that failed to load.
        path: PathBuf,
        /// The underlying whisper.cpp error, boxed to hide the dependency.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The transcription worker thread could not be started.
    #[non_exhaustive]
    #[error("spawn transcription worker")]
    SpawnWorker(#[source] std::io::Error),

    /// The model rejected an audio window.
    #[non_exhaustive]
    #[error("transcribe audio window")]
    Inference(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The transcription worker exited while requests were in flight.
    #[non_exhaustive]
    #[error("transcription worker exited")]
    WorkerGone,

    /// The `[voice]` configuration is invalid.
    #[non_exhaustive]
    #[error("invalid voice configuration: {0}")]
    InvalidConfig(String),
}

/// Shared fixtures for the transcription tests: a small GGML whisper model
/// and a 16 kHz mono WAV of known speech, both downloaded out of band (the
/// URLs are recorded in the design log) and gitignored.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::{Path, PathBuf};

    /// The directory holding the downloaded fixtures.
    pub(crate) fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// Path to the test model, `ggml-tiny.en.bin`.
    pub(crate) fn model_path() -> PathBuf {
        fixture_dir().join("ggml-tiny.en.bin")
    }

    /// Path to the test model, panicking with download instructions when it
    /// has not been fetched.
    pub(crate) fn require_model() -> PathBuf {
        let path = model_path();
        assert!(
            path.is_file(),
            "test model missing: download ggml-tiny.en.bin from \
             https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin \
             into {}",
            fixture_dir().display()
        );
        path
    }

    /// Decodes `jfk.wav` (16 kHz mono s16 PCM, "ask not what your country
    /// can do for you") into f32 samples for the wire format.
    pub(crate) fn jfk_samples() -> Vec<f32> {
        let path = fixture_dir().join("jfk.wav");
        let mut reader =
            hound::WavReader::open(&path).expect("jfk.wav fixture exists beside the test model");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000, "fixture must be 16 kHz");
        assert_eq!(spec.channels, 1, "fixture must be mono");
        assert_eq!(spec.bits_per_sample, 16, "fixture must be 16-bit PCM");
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<_, _>>()
            .expect("fixture decodes as s16 PCM");
        let mut floats = vec![0.0; samples.len()];
        whisper_rs::convert_integer_to_float_audio(&samples, &mut floats)
            .expect("s16 to f32 conversion cannot fail");
        floats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::transcribe::fixtures;

    #[tokio::test]
    async fn transcribes_known_speech_fixture() {
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            window_seconds: 12,
            ..VoiceConfig::default()
        };
        let engine = VoiceEngine::new(&config).expect("engine loads the fixture model");
        let text = engine
            .transcribe(fixtures::jfk_samples())
            .await
            .expect("transcription succeeds");
        assert!(
            text.to_lowercase().contains("country"),
            "transcript names the fixture's words: {text:?}"
        );
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[]).to_bits(), 0.0f64.to_bits());
        assert_eq!(rms(&[0.0; 1600]).to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn rms_of_a_constant_signal_is_its_amplitude() {
        assert!((rms(&[0.5; 100]) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn silence_gate_separates_quiet_from_speech() {
        assert!(is_silence(&[0.0; 1600]));
        assert!(is_silence(&[0.0005; 1600]));
        assert!(!is_silence(&[0.05; 1600]));
    }

    #[test]
    fn tail_returns_the_trailing_window() {
        let buffer: Vec<f32> = (0u8..10).map(f32::from).collect();
        assert_eq!(tail(&buffer, 4), &[6.0, 7.0, 8.0, 9.0]);
        assert_eq!(tail(&buffer, 100), &buffer[..]);
        assert_eq!(tail(&[], 4), &[] as &[f32]);
    }

    #[test]
    fn invalid_voice_config_is_rejected() {
        let config = VoiceConfig {
            window_seconds: 0,
            ..VoiceConfig::default()
        };
        let err = VoiceEngine::new(&config).expect_err("zero window must fail");
        assert!(
            matches!(err, TranscribeError::InvalidConfig(_)),
            "expected InvalidConfig, got {err:?}"
        );
    }

    #[test]
    fn sanitize_prompt_strips_nulls_and_caps_length() {
        assert_eq!(sanitize_prompt("hello"), "hello");
        assert_eq!(sanitize_prompt("a\0b"), "ab");
        let long = "x".repeat(MAX_PROMPT_CHARS + 100);
        assert_eq!(sanitize_prompt(&long).len(), MAX_PROMPT_CHARS);
        // Multibyte input is capped at a char boundary, never mid-codepoint.
        let multibyte = "é".repeat(MAX_PROMPT_CHARS + 10);
        let capped = sanitize_prompt(&multibyte);
        assert!(capped.len() <= MAX_PROMPT_CHARS);
        assert!(capped.chars().all(|c| c == 'é'));
    }

    #[test]
    fn missing_final_model_fails_engine_construction() {
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            final_model: PathBuf::from("definitely-missing-final-model.bin"),
            ..VoiceConfig::default()
        };
        let err = VoiceEngine::new(&config).expect_err("a missing final model must fail");
        assert!(
            matches!(err, TranscribeError::LoadModel { .. }),
            "expected LoadModel, got {err:?}"
        );
        assert!(
            err.to_string()
                .contains("definitely-missing-final-model.bin"),
            "error names the path: {err}"
        );
    }

    #[tokio::test]
    async fn final_pass_entry_points_are_no_ops_without_a_final_model() {
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            ..VoiceConfig::default()
        };
        let engine = VoiceEngine::new(&config).expect("engine loads the fixture model");
        let (segment_tx, _segment_rx) = std::sync::mpsc::channel();
        engine.final_reset(segment_tx);
        engine.final_submit(fixtures::jfk_samples());
        assert!(
            engine.final_finish(fixtures::jfk_samples()).await.is_none(),
            "no final model means the caller falls back"
        );
    }

    #[tokio::test]
    async fn final_submit_reports_the_segment_on_the_take_channel() {
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            final_model: fixtures::require_model(),
            ..VoiceConfig::default()
        };
        let engine = VoiceEngine::new(&config).expect("engine loads the fixture model");
        let (segment_tx, segment_rx) = std::sync::mpsc::channel();
        engine.final_reset(segment_tx);
        engine.final_submit(fixtures::jfk_samples());

        // The timeout only bounds a broken pipeline; the tiny fixture
        // model transcribes the clip in seconds.
        let segment = segment_rx
            .recv_timeout(Duration::from_secs(120))
            .expect("the submitted segment's text arrives on the channel");
        assert!(
            segment.to_lowercase().contains("country"),
            "the reported segment names the fixture's words: {segment:?}"
        );

        let tail = engine
            .final_finish(fixtures::jfk_samples())
            .await
            .expect("a final model is configured")
            .expect("the final pass succeeds");
        assert!(
            tail.to_lowercase().contains("country"),
            "the closing tail names the fixture's words: {tail:?}"
        );
        let countries = tail.to_lowercase().matches("country").count();
        assert!(
            countries < 3,
            "the finish returns the tail's text only, not the assembled \
             transcript ({countries} countries): {tail:?}"
        );
        assert!(
            segment_rx.try_recv().is_err(),
            "the closing tail reports only through its reply, not the channel"
        );
    }

    #[tokio::test]
    async fn final_finish_with_a_silent_tail_returns_empty_after_draining() {
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            final_model: fixtures::require_model(),
            ..VoiceConfig::default()
        };
        let engine = VoiceEngine::new(&config).expect("engine loads the fixture model");
        let (segment_tx, segment_rx) = std::sync::mpsc::channel();
        engine.final_reset(segment_tx);
        engine.final_submit(fixtures::jfk_samples());

        // The tail is pure silence: the worker skips it rather than
        // hallucinating, and the FIFO reply still drains the take's
        // submitted segment first.
        let tail = engine
            .final_finish(vec![0.0; SAMPLE_RATE])
            .await
            .expect("a final model is configured")
            .expect("the final pass succeeds");
        assert!(
            tail.is_empty(),
            "a silent tail is skipped, not transcribed: {tail:?}"
        );
        let segment = segment_rx
            .recv_timeout(Duration::from_secs(120))
            .expect("the submitted segment's text arrives on the channel");
        assert!(
            segment.to_lowercase().contains("country"),
            "the drained segment names the fixture's words: {segment:?}"
        );
    }

    #[test]
    fn final_pass_conditions_each_segment_on_the_accumulated_transcript() {
        let mut pass = FinalPass::load(&fixtures::require_model())
            .expect("final pass loads the fixture model");
        let jfk = fixtures::jfk_samples();

        let first = pass
            .transcribe_segment(&jfk)
            .expect("segment one transcribes")
            .expect("segment one appended text");
        assert!(
            pass.last_prompt().is_empty(),
            "the first segment has nothing to be conditioned on"
        );
        let first_lower = first.to_lowercase();
        assert!(
            first_lower.contains("country"),
            "segment one names the fixture's words: {first:?}"
        );
        let first_countries = first_lower.matches("country").count();
        assert_eq!(
            pass.transcript(),
            first,
            "the accumulated transcript is the first segment's text"
        );

        let second = pass
            .transcribe_segment(&jfk)
            .expect("segment two transcribes")
            .expect("segment two appended text");
        assert_eq!(
            pass.last_prompt(),
            first,
            "segment two was conditioned on the accumulated transcript"
        );
        assert!(
            second.to_lowercase().contains("country"),
            "the segment's own text names the fixture's words: {second:?}"
        );
        let assembled = pass.transcript();
        assert!(
            assembled.starts_with(&first),
            "segment transcripts accumulate in order: {assembled:?}"
        );
        let second_countries = assembled.to_lowercase().matches("country").count();
        assert!(
            second_countries > first_countries,
            "the second segment added its own text: {first_countries} then {second_countries}"
        );
    }

    #[test]
    fn final_pass_reset_forgets_the_accumulated_transcript() {
        let mut pass = FinalPass::load(&fixtures::require_model())
            .expect("final pass loads the fixture model");
        let jfk = fixtures::jfk_samples();

        let first = pass
            .transcribe_segment(&jfk)
            .expect("segment one transcribes")
            .expect("segment one appended text");
        pass.reset();
        let second = pass
            .transcribe_segment(&jfk)
            .expect("segment two transcribes")
            .expect("segment two appended text");
        assert!(
            pass.last_prompt().is_empty(),
            "after reset the next segment has nothing to be conditioned on"
        );
        assert_eq!(
            second, first,
            "a new take's transcript holds only its own segments"
        );
        assert_eq!(
            pass.transcript(),
            second,
            "the accumulated transcript forgot the previous take"
        );
    }

    #[test]
    fn final_pass_skips_silence_without_touching_the_transcript() {
        let mut pass = FinalPass::load(&fixtures::require_model())
            .expect("final pass loads the fixture model");
        let segment = pass
            .transcribe_segment(&vec![0.0; SAMPLE_RATE * 2])
            .expect("silence is skipped, not an error");
        assert!(segment.is_none(), "a skipped segment reports no text");
        assert!(
            pass.transcript().is_empty(),
            "silence transcribes to nothing"
        );
        assert!(
            pass.last_prompt().is_empty(),
            "a skipped segment records no conditioning"
        );
    }

    #[test]
    fn missing_model_file_fails_engine_construction() {
        let config = VoiceConfig {
            interim_model: PathBuf::from("definitely-missing-model.bin"),
            ..VoiceConfig::default()
        };
        let err = VoiceEngine::new(&config).expect_err("a missing model must fail");
        assert!(
            matches!(err, TranscribeError::LoadModel { .. }),
            "expected LoadModel, got {err:?}"
        );
        assert!(
            err.to_string().contains("definitely-missing-model.bin"),
            "error names the path: {err}"
        );
    }
}
