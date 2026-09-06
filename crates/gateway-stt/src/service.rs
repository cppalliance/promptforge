//! Cloneable host facade for speech lifecycle, facts, and routes.

use gateway_config::Config;
use shared_progress::ProgressHandle;

use crate::artifacts::{self, PreparedSpeech, SpeechError};
use crate::generation::{GenerationState, SpeechReplacement};
use crate::model::SpeechModelInfo;
#[cfg(feature = "test-fixtures")]
use crate::realtime::ForcedPrecommitFailure;
use crate::realtime::{RoutePolicy, SessionRegistry};
use crate::status::SpeechStatus;

/// Cloneable Gateway handle for all speech behavior.
#[derive(Debug, Clone, Default)]
pub struct SpeechService {
    pub(crate) state: GenerationState,
    sessions: SessionRegistry,
    realtime_policy: RoutePolicy,
}

impl SpeechService {
    /// Creates an inactive service.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Verifies and stages configured artifacts without starting workers.
    ///
    /// # Errors
    /// Returns a typed store, download, verification, or configuration error.
    pub fn prepare(
        &self,
        config: &Config,
        progress: Option<&ProgressHandle>,
    ) -> Result<PreparedSpeech, SpeechError> {
        artifacts::prepare(config, progress)
    }

    /// Serializes replacement, drains old ownership, and loads a staged generation.
    ///
    /// # Errors
    /// Returns a drain deadline, backend, policy, or worker startup error.
    pub fn begin_replacement(
        &self,
        prepared: PreparedSpeech,
    ) -> Result<SpeechReplacement, SpeechError> {
        self.state.stage(prepared)
    }

    /// Serializes replacement and constrains drain plus worker startup to one deadline.
    ///
    /// # Errors
    /// Returns a drain deadline, backend, policy, or worker startup error.
    pub fn begin_replacement_before(
        &self,
        prepared: PreparedSpeech,
        deadline: std::time::Instant,
    ) -> Result<SpeechReplacement, SpeechError> {
        self.state.stage_until(prepared, deadline)
    }

    /// Publishes every fact in a staged generation through one transition.
    ///
    /// # Errors
    /// Returns an ownership error for a foreign or shutdown-invalidated token.
    pub fn commit_replacement(&self, replacement: SpeechReplacement) -> Result<(), SpeechError> {
        self.state.commit(replacement)
    }

    /// Stops a staged generation and reconstructs the old specification.
    ///
    /// # Errors
    /// Returns an ownership, shutdown, or old-generation reconstruction error.
    pub fn abort_replacement(&self, replacement: SpeechReplacement) -> Result<(), SpeechError> {
        self.state.abort(replacement)
    }

    /// Stops admitting work and waits for the active generation to unload.
    pub fn shutdown(&self) {
        self.state.shutdown();
    }

    /// Returns one point-in-time status snapshot.
    #[must_use]
    pub fn status(&self) -> SpeechStatus {
        self.state.status()
    }

    /// Returns physical batch models and any ready logical model from one snapshot.
    #[must_use]
    pub fn models(&self) -> Vec<SpeechModelInfo> {
        self.state.models()
    }

    /// Blocks Realtime sends after `successful_sends` for deadline tests.
    #[cfg(feature = "test-fixtures")]
    pub fn block_realtime_send_after(&mut self, successful_sends: usize) {
        self.realtime_policy = RoutePolicy::blocking_after(successful_sends);
    }

    /// Forces a typed precommit transcription failure for route tests.
    #[cfg(feature = "test-fixtures")]
    pub fn fail_realtime_precommit(&mut self) {
        self.realtime_policy
            .force_precommit_failure(ForcedPrecommitFailure::Transcription);
    }

    /// Forces a typed final-segment overload for route tests.
    #[cfg(feature = "test-fixtures")]
    pub fn overload_realtime_final_segment(&mut self) {
        self.realtime_policy
            .force_precommit_failure(ForcedPrecommitFailure::FinalSegmentOverload);
    }

    /// Returns the batch and temporary legacy Gateway routes.
    #[cfg(not(miri))]
    pub fn routes(&self) -> axum::Router {
        crate::batch::routes(self.state.clone())
            .merge(crate::stt::gateway_router(self.state.clone()))
            .merge(crate::realtime::routes(
                self.state.clone(),
                self.sessions.clone(),
                self.realtime_policy.clone(),
            ))
    }

    /// Returns the temporary Workshop-hosted legacy routes.
    #[cfg(not(miri))]
    pub fn workshop_routes(&self, push: workshop_server::Push) -> axum::Router {
        crate::stt::workshop_router(self.state.clone(), push)
    }
}
