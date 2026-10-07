//! Request and worker ownership for one admitted speech runtime.

use std::sync::Arc;
use std::time::Duration;

use gateway_stt_engine::{
    DecodeMode, DecodeOutput, DecodeRequest, FallbackDetector, TranscribeError,
};

use crate::admission::{AdmissionLease, JobLease, SessionEpoch};
use crate::take::FallbackReport;

use super::snapshot::{Silero, SpeechRuntime};

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

    /// The detector a new take classifies with, Silero when the generation
    /// has a verified model that loads and loudness otherwise, and the
    /// report its later fall back goes through. A load failure is reported
    /// here, once for the take; a missing model was reported by the
    /// generation's load.
    pub(crate) fn speech_detector(&self) -> (FallbackDetector, FallbackReport) {
        let Some(Silero {
            model: Ok(model),
            source,
            progress,
        }) = &self.runtime().silero
        else {
            return (FallbackDetector::energy(), FallbackReport::default());
        };
        let report = FallbackReport::new(progress.clone());
        match source.load(model) {
            Ok(primary) => (FallbackDetector::new(primary), report),
            Err(error) => {
                report.report(
                    "Silero speech detector did not load; this take detects speech by loudness",
                    &error,
                );
                (FallbackDetector::energy(), report)
            }
        }
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
