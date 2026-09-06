//! Request and worker ownership for one admitted generation.

use std::sync::Arc;
use std::time::Duration;

use gateway_stt_engine::{DecodeMode, DecodeRequest, TranscribeError};

use crate::replacement::{AdmissionLease, JobLease, SessionEpoch};

use super::snapshot::Generation;

/// One explicitly counted request or session borrowing a complete generation.
#[derive(Debug)]
pub(crate) struct GenerationLease {
    generation: Option<Arc<Generation>>,
    admission: AdmissionLease,
}

impl Clone for GenerationLease {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation.as_ref().map(Arc::clone),
            admission: self.admission.clone(),
        }
    }
}

impl Drop for GenerationLease {
    fn drop(&mut self) {
        drop(self.generation.take());
    }
}

impl GenerationLease {
    pub(super) fn new(generation: Arc<Generation>, admission: AdmissionLease) -> Self {
        Self {
            generation: Some(generation),
            admission,
        }
    }

    fn generation(&self) -> &Generation {
        self.generation
            .as_deref()
            .unwrap_or_else(|| unreachable!("generation lease is live until drop"))
    }

    pub(crate) fn guidance(&self) -> &[String] {
        &self.generation().guidance
    }

    pub(super) fn select(&self, name: &str) -> Option<DecodeMode> {
        self.generation().select(name)
    }

    pub(crate) fn has_final_pass(&self) -> bool {
        self.generation().has_final_pass()
    }

    pub(crate) fn window_samples(&self) -> usize {
        self.generation().window_samples()
    }

    pub(crate) fn interval(&self) -> Duration {
        self.generation().interval()
    }

    pub(crate) fn epoch(&self) -> &SessionEpoch {
        self.admission.epoch()
    }

    pub(crate) fn own_job(&self) -> Option<GenerationJob> {
        let ownership = self.admission.own_job()?;
        Some(GenerationJob {
            generation: self.generation.as_ref().map(Arc::clone),
            ownership: Some(ownership),
        })
    }

    pub(crate) async fn decode(&self, request: DecodeRequest) -> Result<String, TranscribeError> {
        let job = self.own_job().ok_or_else(generation_unavailable)?;
        let epoch = self.epoch().clone();
        let (reply, result) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if reply.is_closed() {
                return;
            }
            let result = job.generation().decode(request).await;
            drop(job);
            drop(reply.send(result));
        });
        tokio::select! {
            biased;
            () = epoch.cancelled() => Err(generation_unavailable()),
            result = result => result.unwrap_or_else(|_| Err(generation_unavailable())),
        }
    }
}

/// One worker job whose count outlives cancellation of its request future.
#[derive(Debug)]
pub(crate) struct GenerationJob {
    generation: Option<Arc<Generation>>,
    ownership: Option<JobLease>,
}

impl GenerationJob {
    fn generation(&self) -> &Generation {
        self.generation
            .as_deref()
            .unwrap_or_else(|| unreachable!("generation job is live until drop"))
    }
}

impl Drop for GenerationJob {
    fn drop(&mut self) {
        drop(self.generation.take());
        drop(self.ownership.take());
    }
}

fn generation_unavailable() -> TranscribeError {
    TranscribeError::inference(std::io::Error::other(
        "speech generation admission is closed",
    ))
}
