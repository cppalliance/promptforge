//! The interim whisper worker thread and the shared blocking inference pass.

use std::io::Read;
use std::path::Path;

use promptforge_progress::ProgressHandle;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::MAX_PROMPT_TOKENS;
use crate::error::TranscribeError;
use crate::prompt::{fit_glossary, sanitize_prompt};

/// Chunk size for the prewarm read: large enough to bound syscall count on
/// multi-GiB models, small enough that `set_units` moves visibly.
const PREWARM_CHUNK: usize = 4 * 1024 * 1024;

/// One transcription request handed to the worker thread.
struct Job {
    samples: Vec<f32>,
    reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
}

/// Handle to the whisper worker thread.
#[derive(Debug)]
pub(crate) struct Transcriber {
    job_tx: Option<std::sync::mpsc::Sender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Transcriber {
    /// Spawns the worker thread, which prewarms and loads the model and then
    /// reports the load outcome on the returned channel. The caller waits on
    /// the channel, so several workers can load in parallel.
    ///
    /// # Errors
    /// Returns [`TranscribeError::SpawnWorker`] when the thread cannot be
    /// started. A model load failure arrives on the returned channel as
    /// [`TranscribeError::LoadModel`].
    pub(super) fn spawn(
        model_path: &Path,
        vocabulary: &[String],
        progress: Option<ProgressHandle>,
    ) -> Result<(Self, std::sync::mpsc::Receiver<Result<(), TranscribeError>>), TranscribeError>
    {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let path = model_path.to_path_buf();
        let vocabulary = vocabulary.to_vec();
        let worker = std::thread::Builder::new()
            .name("whisper-transcribe".to_string())
            .spawn(move || worker_loop(&path, &vocabulary, progress.as_ref(), &job_rx, &init_tx))
            .map_err(TranscribeError::SpawnWorker)?;
        Ok((
            Self {
                job_tx: Some(job_tx),
                worker: Some(worker),
            },
            init_rx,
        ))
    }

    /// Queues `samples` for transcription and awaits the trimmed text.
    pub(super) async fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        let Some(job_tx) = &self.job_tx else {
            return Err(TranscribeError::WorkerGone);
        };
        job_tx
            .send(Job { samples, reply })
            .map_err(|_| TranscribeError::WorkerGone)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }
}

impl Drop for Transcriber {
    fn drop(&mut self) {
        // Close the queue before joining so the worker exits after any
        // in-progress inference and releases its Whisper context.
        drop(self.job_tx.take());
        if let Some(worker) = self.worker.take() {
            let _ignored = worker.join();
        }
    }
}

/// The worker thread's body: load the model, fit the glossary prompt, then
/// transcribe jobs in arrival order until every sender is dropped.
fn worker_loop(
    path: &Path,
    vocabulary: &[String],
    progress: Option<&ProgressHandle>,
    job_rx: &std::sync::mpsc::Receiver<Job>,
    init_tx: &std::sync::mpsc::SyncSender<Result<(), TranscribeError>>,
) {
    let Some((ctx, mut state)) = load_state(path, progress, init_tx) else {
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
/// `init_tx` (which the spawner blocks on). Returns `None` after reporting a
/// failure, or when the spawner is already gone.
pub(super) fn load_state(
    path: &Path,
    progress: Option<&ProgressHandle>,
    init_tx: &std::sync::mpsc::SyncSender<Result<(), TranscribeError>>,
) -> Option<(WhisperContext, whisper_rs::WhisperState)> {
    let loaded = load_context(path, progress);
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

/// Prewarms the model file, then loads the whisper context and state. The
/// byte-counted prewarm and the indeterminate whisper/CUDA init report as
/// sibling leaves under `progress`.
fn load_context(
    path: &Path,
    progress: Option<&ProgressHandle>,
) -> Result<(WhisperContext, whisper_rs::WhisperState), TranscribeError> {
    let prewarm_leaf = progress.map(|handle| handle.child("prewarm", 1.0));
    prewarm(path, prewarm_leaf.as_ref())?;
    let init_leaf = progress.map(|handle| handle.child("init", 1.0));
    let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default()).map_err(
        |source| TranscribeError::LoadModel {
            path: path.to_path_buf(),
            source: Box::new(source),
        },
    )?;
    let state = ctx
        .create_state()
        .map_err(|source| TranscribeError::LoadModel {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
    if let Some(leaf) = &init_leaf {
        leaf.complete();
    }
    Ok((ctx, state))
}

/// Reads `path` sequentially through a reused buffer so the model file sits
/// in the page cache before whisper maps it, reporting bytes read on
/// `progress`. Unconditional: the engine only runs on machines with memory
/// for the models it loads, so the thrash case is excluded by design.
///
/// # Errors
/// Returns [`TranscribeError::LoadModel`] naming `path` when the file
/// cannot be statted, opened, or read.
fn prewarm(path: &Path, progress: Option<&ProgressHandle>) -> Result<(), TranscribeError> {
    let load_error = |source: std::io::Error| TranscribeError::LoadModel {
        path: path.to_path_buf(),
        source: Box::new(source),
    };
    let total = std::fs::metadata(path).map_err(load_error)?.len();
    let mut file = std::fs::File::open(path).map_err(load_error)?;
    let mut buffer = vec![0u8; PREWARM_CHUNK];
    let mut done = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(load_error)?;
        if read == 0 {
            break;
        }
        done += read as u64;
        if let Some(leaf) = progress {
            leaf.set_units(done, total);
        }
    }
    if let Some(leaf) = progress {
        leaf.complete();
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    // Fractions are fixed-point millionths, so equality comparisons are exact.
    #![expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]

    use std::sync::Arc;

    use promptforge_progress::ProgressHub;

    use super::*;

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn prewarm_drives_the_leaf_to_completion() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("prewarm", 1.0);
        prewarm(&crate::fixtures::require_model(), Some(&leaf))
            .expect("prewarm reads the fixture model");
        assert_eq!(
            leaf.fraction(),
            1.0,
            "reading the whole file completes the leaf"
        );
    }

    #[test]
    fn prewarm_of_a_plain_file_drives_the_leaf_to_completion() {
        let dir = tempfile::tempdir().expect("temp dir for the prewarm test");
        let path = dir.path().join("model.bin");
        std::fs::write(&path, vec![0u8; 1024]).expect("write the fake model");
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("prewarm", 1.0);
        prewarm(&path, Some(&leaf)).expect("prewarm reads the file");
        assert_eq!(
            leaf.fraction(),
            1.0,
            "reading the whole file completes the leaf"
        );
    }

    #[test]
    fn prewarm_of_a_missing_file_fails_as_load_model_naming_the_path() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("prewarm", 1.0);
        let err = prewarm(
            Path::new("definitely-missing-prewarm-model.bin"),
            Some(&leaf),
        )
        .expect_err("a missing file must fail");
        assert!(
            matches!(err, TranscribeError::LoadModel { .. }),
            "expected LoadModel, got {err:?}"
        );
        assert!(
            err.to_string()
                .contains("definitely-missing-prewarm-model.bin"),
            "error names the path: {err}"
        );
    }
}
