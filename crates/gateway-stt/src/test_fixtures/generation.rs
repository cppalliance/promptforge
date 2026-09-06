//! Deterministic generation lifecycle fixtures.

use std::time::Duration;

use gateway_stt_engine::{EnginePolicy, SttEngine};

use crate::generation::{GenerationJob, GenerationLease};
use crate::{SpeechError, SpeechService};

use super::ScriptedModelFactory;

/// Builds a speech service around deterministic scripted workers.
///
/// # Errors
/// Returns engine policy, startup, or worker construction failures.
pub fn scripted_service(
    factory: ScriptedModelFactory,
    window_seconds: u64,
    interval_ms: u64,
) -> Result<SpeechService, SpeechError> {
    let gpu_available = factory.gpu_available();
    let policy = EnginePolicy::new(window_seconds, interval_ms, gpu_available)
        .map_err(SpeechError::Engine)?;
    let engine = SttEngine::new(factory, policy).map_err(SpeechError::Engine)?;
    let final_name = engine.has_final_pass().then(|| "scripted-final".to_owned());
    let service = SpeechService::new();
    let replacement = service.scripted_replacement(engine, final_name)?;
    service.commit_replacement(replacement)?;
    Ok(service)
}

/// Quiesces the current generation and builds one deterministic replacement.
///
/// # Errors
/// Returns a quiescence, policy, startup, or replacement-ownership failure.
pub fn begin_scripted_replacement(
    service: &SpeechService,
    factory: ScriptedModelFactory,
    with_final: bool,
    timeout: Duration,
) -> Result<crate::SpeechReplacement, SpeechError> {
    let gpu_available = factory.gpu_available();
    service.state.stage_scripted(
        factory,
        with_final.then(|| "scripted-final".to_owned()),
        gpu_available,
        timeout,
    )
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
    /// Returns the replaceable session epoch captured at admission.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.lease.epoch().id()
    }

    /// Whether replacement or shutdown canceled this request's epoch.
    #[must_use]
    pub fn is_replaced(&self) -> bool {
        self.lease.epoch().is_cancelled()
    }

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
