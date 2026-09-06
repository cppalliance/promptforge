//! One backend-neutral transcription worker.

use std::sync::Arc;

use crate::{Decoder, ModelFactory, TranscribeError};

struct Job {
    samples: Vec<f32>,
    guidance: Vec<String>,
    finalized: String,
    reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
}

/// Handle to a decoder confined to its worker thread.
#[derive(Debug)]
pub(crate) struct Transcriber {
    job_tx: Option<std::sync::mpsc::Sender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Transcriber {
    /// Spawns one worker and reports whether its optional decoder exists.
    pub(super) fn spawn(
        name: &'static str,
        factory: Arc<dyn ModelFactory>,
        final_model: bool,
    ) -> Result<
        (
            Self,
            std::sync::mpsc::Receiver<Result<bool, TranscribeError>>,
        ),
        TranscribeError,
    > {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || worker_loop(factory.as_ref(), final_model, &job_rx, &init_tx))
            .map_err(TranscribeError::SpawnWorker)?;
        Ok((
            Self {
                job_tx: Some(job_tx),
                worker: Some(worker),
            },
            init_rx,
        ))
    }

    pub(super) async fn transcribe(
        &self,
        samples: Vec<f32>,
        guidance: Vec<String>,
        finalized: String,
    ) -> Result<String, TranscribeError> {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        let Some(job_tx) = &self.job_tx else {
            return Err(TranscribeError::WorkerGone);
        };
        job_tx
            .send(Job {
                samples,
                guidance,
                finalized,
                reply,
            })
            .map_err(|_| TranscribeError::WorkerGone)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }
}

impl Drop for Transcriber {
    fn drop(&mut self) {
        drop(self.job_tx.take());
        if let Some(worker) = self.worker.take() {
            let _ignored = worker.join();
        }
    }
}

fn worker_loop(
    factory: &dyn ModelFactory,
    final_model: bool,
    job_rx: &std::sync::mpsc::Receiver<Job>,
    init_tx: &std::sync::mpsc::SyncSender<Result<bool, TranscribeError>>,
) {
    let decoder = if final_model {
        factory.create_final()
    } else {
        factory.create_interim().map(Some)
    };
    let Some(mut decoder): Option<Box<dyn Decoder>> = (match decoder {
        Ok(decoder) => {
            if init_tx.send(Ok(decoder.is_some())).is_err() {
                return;
            }
            decoder
        }
        Err(error) => {
            // Initialization failure is terminal, and cancellation leaves no
            // engine constructor to receive it.
            match init_tx.send(Err(error)) {
                Ok(()) | Err(_) => return,
            }
        }
    }) else {
        return;
    };
    while let Ok(job) = job_rx.recv() {
        let result = decoder.transcribe(&job.samples, &job.guidance, &job.finalized);
        if job.reply.send(result).is_err() {
            // A canceled caller abandons only its reply; the stateless worker
            // remains available for later jobs.
        }
    }
}
