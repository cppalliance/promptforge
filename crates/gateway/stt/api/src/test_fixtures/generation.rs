//! Deterministic generation lifecycle fixtures.

use gateway_stt_engine::{EnginePolicy, ModelFactory};
use tokio_util::sync::CancellationToken;

use crate::generation::{GenerationJob, GenerationLease};
use crate::{SpeechError, SpeechService};

use super::ScriptedModelFactory;

/// Builds a speech service whose one initial load publishes scripted workers.
///
/// # Errors
/// Returns engine policy, startup, or worker construction failures.
pub fn scripted_service(
    factory: ScriptedModelFactory,
    window_seconds: u64,
    interval_ms: u64,
) -> Result<SpeechService, SpeechError> {
    let service = SpeechService::new();
    load_scripted_initial(&service, factory, window_seconds, interval_ms)?;
    Ok(service)
}

/// Attempts the one initial load with deterministic scripted workers.
///
/// # Errors
/// Returns the spent-attempt rejection, engine policy, startup, or worker
/// construction failures.
pub fn load_scripted_initial(
    service: &SpeechService,
    factory: ScriptedModelFactory,
    window_seconds: u64,
    interval_ms: u64,
) -> Result<(), SpeechError> {
    let gpu_available = factory.gpu_available();
    let policy = EnginePolicy::new(window_seconds, interval_ms, gpu_available)
        .map_err(SpeechError::Engine)?;
    service
        .state
        .load_scripted(factory, policy, &CancellationToken::new())
}

/// Attempts the one initial scripted load under a cancellation token.
///
/// # Errors
/// Returns the spent-attempt rejection, cancellation, engine policy, startup,
/// or worker construction failures.
pub fn load_scripted_initial_with_cancellation(
    service: &SpeechService,
    factory: impl ModelFactory,
    cancel: &CancellationToken,
) -> Result<(), SpeechError> {
    let policy = EnginePolicy::new(15, 500, false).map_err(SpeechError::Engine)?;
    service.state.load_scripted(factory, policy, cancel)
}

/// Returns explicit request and worker-job ownership for the active generation.
#[must_use]
pub fn generation_counts(service: &SpeechService) -> Option<(usize, usize)> {
    service.state.counts()
}

/// Admits one deterministic request owner from the current generation.
#[must_use]
pub fn generation_ownership(service: &SpeechService) -> Option<GenerationOwnershipFixture> {
    service
        .state
        .active()
        .map(|lease| GenerationOwnershipFixture { lease })
}

/// An admitted generation request exposed only to integration tests.
#[derive(Debug)]
pub struct GenerationOwnershipFixture {
    lease: GenerationLease,
}

impl GenerationOwnershipFixture {
    /// Adds one worker-job owner tied to this admitted request.
    #[must_use]
    pub fn own_worker_job(&self) -> Option<GenerationWorkerJobFixture> {
        self.lease
            .own_job()
            .map(|job| GenerationWorkerJobFixture { job: Some(job) })
    }
}

/// Explicit worker ownership exposed only to integration tests.
#[derive(Debug)]
pub struct GenerationWorkerJobFixture {
    job: Option<GenerationJob>,
}

impl Drop for GenerationWorkerJobFixture {
    fn drop(&mut self) {
        drop(self.job.take());
    }
}
