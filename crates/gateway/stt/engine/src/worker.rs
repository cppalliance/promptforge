//! One backend-neutral transcription worker.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use crate::{DecodeMode, DecodeRequest, Decoder, ModelFactory, TranscribeError};

pub(crate) const INTERIM_JOB_CAPACITY: usize = 8;
pub(crate) const FINAL_JOB_CAPACITY: usize = 8;

struct Job {
    request: DecodeRequest,
    reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
    lifetime_guard: Option<Arc<dyn Send + Sync>>,
}

/// Handle to a decoder confined to its worker thread.
#[derive(Debug)]
pub(crate) struct Transcriber {
    state: Mutex<TranscriberState>,
    stopping: Arc<AtomicBool>,
}

#[derive(Debug)]
struct TranscriberState {
    job_tx: Option<mpsc::SyncSender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
    join_panicked: bool,
}

impl Transcriber {
    /// Spawns one worker and reports whether its optional decoder exists.
    pub(super) fn spawn(
        name: &'static str,
        factory: Arc<dyn ModelFactory>,
        mode: DecodeMode,
        capacity: usize,
    ) -> Result<(Self, mpsc::Receiver<Result<bool, TranscribeError>>), TranscribeError> {
        let (job_tx, job_rx) = mpsc::sync_channel::<Job>(capacity);
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let worker = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                worker_loop(factory.as_ref(), mode, &job_rx, &init_tx, &worker_stopping);
            })
            .map_err(TranscribeError::SpawnWorker)?;
        Ok((
            Self {
                state: Mutex::new(TranscriberState {
                    job_tx: Some(job_tx),
                    worker: Some(worker),
                    join_panicked: false,
                }),
                stopping,
            },
            init_rx,
        ))
    }

    fn submit(
        &self,
        mut request: DecodeRequest,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<String, TranscribeError>>, TranscribeError>
    {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        let lifetime_guard = request.take_lifetime_guard();
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(job_tx) = &state.job_tx else {
            return Err(TranscribeError::WorkerGone);
        };
        job_tx
            .try_send(Job {
                request,
                reply,
                lifetime_guard,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => TranscribeError::Overloaded,
                mpsc::TrySendError::Disconnected(_) => TranscribeError::WorkerGone,
            })?;
        Ok(reply_rx)
    }

    pub(super) async fn transcribe(
        &self,
        request: DecodeRequest,
    ) -> Result<String, TranscribeError> {
        let reply_rx = self.submit(request)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }

    /// Closes the job queue and joins the worker thread.
    ///
    /// This is the blocking, error-reporting path; `Drop` only signals
    /// and detaches through [`Transcriber::signal_and_detach`].
    pub(super) fn shutdown(&self) -> Result<(), TranscribeError> {
        self.stopping.store(true, Ordering::Release);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        drop(state.job_tx.take());
        if let Some(worker) = state.worker.take() {
            state.join_panicked = worker.join().is_err();
        }
        if state.join_panicked {
            Err(TranscribeError::ShutdownPanicked)
        } else {
            Ok(())
        }
    }

    pub(super) fn abandon_startup(&self) {
        // Construction is non-preemptible. Dropping this handle explicitly
        // abandons only a timed-out startup worker so the caller can classify
        // the fatal outcome without claiming the thread was stopped.
        self.signal_and_detach();
    }

    /// Signals the worker and detaches its thread without joining.
    ///
    /// The worker captures only owned or `Arc` state (the factory, the job
    /// receiver, the stop flag) and borrows nothing from this handle, so a
    /// detached thread finishes any running decode on its own.
    pub(super) fn signal_and_detach(&self) {
        self.stopping.store(true, Ordering::Release);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        drop(state.job_tx.take());
        drop(state.worker.take());
    }

    pub(super) fn startup_failure(
        startup: TranscribeError,
        cleanup: impl IntoIterator<Item = Result<(), TranscribeError>>,
    ) -> TranscribeError {
        let cleanup = cleanup
            .into_iter()
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        if cleanup.is_empty() {
            startup
        } else {
            TranscribeError::StartupCleanup {
                startup: Box::new(startup),
                cleanup,
            }
        }
    }
}

impl Drop for Transcriber {
    fn drop(&mut self) {
        // `shutdown` is the blocking, error-reporting path; Drop can
        // neither wait nor report, so it signals and detaches.
        self.signal_and_detach();
    }
}

fn worker_loop(
    factory: &dyn ModelFactory,
    mode: DecodeMode,
    job_rx: &mpsc::Receiver<Job>,
    init_tx: &mpsc::SyncSender<Result<bool, TranscribeError>>,
    stopping: &AtomicBool,
) {
    let decoder = catch_unwind(AssertUnwindSafe(|| factory.create(mode)));
    let decoder = match decoder {
        Ok(result) => result,
        Err(_) => Err(TranscribeError::WorkerPanicked),
    };
    let Some(mut decoder): Option<Box<dyn Decoder>> = (match decoder {
        Ok(decoder) => {
            if init_tx.send(Ok(decoder.is_some())).is_err() {
                return;
            }
            decoder
        }
        Err(error) => {
            // Initialization is terminal; cancellation leaves no constructor to receive it.
            drop(init_tx.send(Err(error)));
            return;
        }
    }) else {
        return;
    };
    while !stopping.load(Ordering::Acquire) {
        let Ok(job) = job_rx.recv() else {
            return;
        };
        if stopping.load(Ordering::Acquire) {
            return;
        }
        if job.reply.is_closed() {
            continue;
        }
        let result = catch_unwind(AssertUnwindSafe(|| decoder.decode(job.request)));
        drop(job.lifetime_guard);
        if let Ok(result) = result {
            if !stopping.load(Ordering::Acquire) {
                // A disconnected caller no longer needs this stateless result.
                drop(job.reply.send(result));
            }
        } else {
            // A disconnected caller cannot make the panicked worker reusable.
            drop(job.reply.send(Err(TranscribeError::WorkerPanicked)));
            return;
        }
    }
}

#[cfg(test)]
#[path = "worker-tests.rs"]
mod tests;
