//! The interim whisper worker thread and the shared blocking inference pass.

use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::transcribe::MAX_PROMPT_TOKENS;
use crate::transcribe::error::TranscribeError;
use crate::transcribe::prompt::{fit_glossary, sanitize_prompt};

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
    pub(super) fn load(model_path: &Path, vocabulary: &[String]) -> Result<Self, TranscribeError> {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let path = model_path.to_path_buf();
        let vocabulary = vocabulary.to_vec();
        std::thread::Builder::new()
            .name("whisper-transcribe".to_string())
            .spawn(move || worker_loop(&path, &vocabulary, &job_rx, &init_tx))
            .map_err(TranscribeError::SpawnWorker)?;
        init_rx.recv().map_err(|_| TranscribeError::WorkerGone)??;
        Ok(Self { job_tx })
    }

    /// Queues `samples` for transcription and awaits the trimmed text.
    pub(super) async fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        self.job_tx
            .send(Job { samples, reply })
            .map_err(|_| TranscribeError::WorkerGone)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }
}

/// The worker thread's body: load the model, fit the glossary prompt, then
/// transcribe jobs in arrival order until every sender is dropped.
fn worker_loop(
    path: &Path,
    vocabulary: &[String],
    job_rx: &std::sync::mpsc::Receiver<Job>,
    init_tx: &std::sync::mpsc::SyncSender<Result<(), TranscribeError>>,
) {
    let Some((ctx, mut state)) = load_state(path, init_tx) else {
        return;
    };
    // The interim pass carries no transcript, so the glossary gets the full
    // prompt budget.
    let glossary = fit_glossary(&ctx, vocabulary, MAX_PROMPT_TOKENS);
    while let Ok(job) = job_rx.recv() {
        // The receiver may be gone (session closed mid-pass); the transcript
        // is computed anyway and the send failure ignored.
        let _ = job.reply.send(transcribe_blocking(
            &mut state,
            &job.samples,
            glossary.as_deref(),
            true,
        ));
    }
}

/// Loads a whisper context and state from `path`, reporting the outcome on
/// `init_tx` (which the spawning `load` blocks on). Returns `None` after
/// reporting a failure, or when the spawner is already gone.
pub(super) fn load_state(
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

/// Runs one blocking whisper pass over `samples` and concatenates the
/// segments. `prompt`, when non-empty after sanitizing, conditions the
/// decoder on the take's transcript so far; `single_segment` forces the
/// whole buffer into one decoding pass (the interim sliding-window case).
pub(super) fn transcribe_blocking(
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
