//! Request and worker ownership for one admitted speech runtime.

use std::sync::Arc;
use std::time::Duration;

use gateway_stt_engine::{DecodeMode, DecodeOutput, DecodeRequest, TranscribeError};

use crate::admission::{AdmissionLease, JobLease, SessionEpoch};
use crate::artifacts::SileroModel;

use super::snapshot::SpeechRuntime;

/// One explicitly counted request or session borrowing a complete runtime.
#[derive(Debug)]
pub(crate) struct GenerationLease {
    runtime: Option<Arc<SpeechRuntime>>,
    admission: AdmissionLease,
}

impl Clone for GenerationLease {
    fn clone(&self) -> Self {
        Self {
            runtime: self.runtime.as_ref().map(Arc::clone),
            admission: self.admission.clone(),
        }
    }
}

impl Drop for GenerationLease {
    fn drop(&mut self) {
        drop(self.runtime.take());
    }
}

impl GenerationLease {
    pub(super) fn new(runtime: Arc<SpeechRuntime>, admission: AdmissionLease) -> Self {
        Self {
            runtime: Some(runtime),
            admission,
        }
    }

    fn runtime(&self) -> &SpeechRuntime {
        self.runtime
            .as_deref()
            .unwrap_or_else(|| unreachable!("generation lease is live until drop"))
    }

    pub(crate) fn guidance(&self) -> &[String] {
        &self.runtime().guidance
    }

    /// The verified Silero model path or the cause there is none, which the
    /// load already reported; `None` for a backend that provisions no model.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no take selects a Silero detector yet")
    )]
    pub(crate) fn silero_model(&self) -> Option<&SileroModel> {
        self.runtime().silero.as_ref()
    }

    pub(super) fn select(&self, name: &str) -> Option<DecodeMode> {
        self.runtime().select(name)
    }

    pub(crate) fn has_final_pass(&self) -> bool {
        self.runtime().has_final_pass()
    }

    pub(crate) fn window_samples(&self) -> usize {
        self.runtime().window_samples()
    }

    pub(crate) fn interval(&self) -> Duration {
        self.runtime().interval()
    }

    pub(crate) fn epoch(&self) -> &SessionEpoch {
        self.admission.epoch()
    }

    pub(crate) async fn cancelled(&self) {
        self.epoch().cancelled().await;
    }

    pub(crate) fn own_job(&self) -> Option<GenerationJob> {
        let ownership = self.admission.own_job()?;
        Some(GenerationJob {
            runtime: self.runtime.as_ref().map(Arc::clone),
            ownership: Some(ownership),
        })
    }

    pub(crate) async fn decode(
        &self,
        request: DecodeRequest,
    ) -> Result<DecodeOutput, TranscribeError> {
        let job = self.own_job().ok_or_else(generation_unavailable)?;
        let epoch = self.epoch().clone();
        let request = request.with_cancellation(epoch.cancellation_flag());
        let (reply, result) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if reply.is_closed() {
                return;
            }
            let result = job.runtime().decode(request).await;
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
    runtime: Option<Arc<SpeechRuntime>>,
    ownership: Option<JobLease>,
}

impl GenerationJob {
    fn runtime(&self) -> &SpeechRuntime {
        self.runtime
            .as_deref()
            .unwrap_or_else(|| unreachable!("generation job is live until drop"))
    }
}

impl Drop for GenerationJob {
    fn drop(&mut self) {
        drop(self.runtime.take());
        drop(self.ownership.take());
    }
}

fn generation_unavailable() -> TranscribeError {
    TranscribeError::inference(std::io::Error::other(
        "speech generation admission is closed",
    ))
}
