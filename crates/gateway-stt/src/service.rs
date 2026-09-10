//! Cloneable host facade for speech lifecycle, facts, and routes.

#[cfg(feature = "test-fixtures")]
use std::sync::Arc;

use gateway_config::Config;
use shared_progress::ProgressHandle;
use tokio_util::sync::CancellationToken;

use crate::artifacts::SpeechError;
use crate::generation::GenerationState;
use crate::model::SpeechModelInfo;
#[cfg(feature = "test-fixtures")]
use crate::realtime::ForcedPrecommitFailure;
use crate::realtime::{RoutePolicy, SessionRegistry};
use crate::status::SpeechStatus;

/// Scripted workers a test-fixture facade publishes on its one initial load
/// instead of the Whisper backend.
#[cfg(feature = "test-fixtures")]
#[derive(Debug)]
struct ScriptedInitialLoad {
    factory: Arc<dyn gateway_stt_engine::ModelFactory>,
    policy: gateway_stt_engine::EnginePolicy,
}

/// Cloneable Gateway handle for all speech behavior.
#[derive(Debug, Clone, Default)]
pub struct SpeechService {
    pub(crate) state: GenerationState,
    sessions: SessionRegistry,
    realtime_policy: RoutePolicy,
    #[cfg(feature = "test-fixtures")]
    scripted: Option<Arc<ScriptedInitialLoad>>,
}

impl SpeechService {
    /// Creates an inactive service.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Arms the empty facade to publish `factory`'s deterministic workers on
    /// its one initial load, replacing the Whisper backend for route,
    /// embedder, and boot-path tests.
    #[cfg(feature = "test-fixtures")]
    #[must_use]
    pub fn with_scripted_initial_load(
        mut self,
        factory: impl gateway_stt_engine::ModelFactory,
        policy: gateway_stt_engine::EnginePolicy,
    ) -> Self {
        self.scripted = Some(Arc::new(ScriptedInitialLoad {
            factory: Arc::new(factory),
            policy,
        }));
        self
    }

    /// Attempts the process's one initial speech load from the boot configuration.
    ///
    /// The facade starts empty and publishes at most one runtime. The attempt
    /// is spent whether it publishes, fails, or is cancelled: speech remains
    /// unavailable until process restart, and every later call is rejected.
    ///
    /// # Errors
    /// Returns a typed store, download, verification, configuration, backend,
    /// or worker startup error, [`SpeechError::InitialLoadCancelled`] when
    /// cancellation fires before publication, or
    /// [`SpeechError::InitialLoadAttempted`] for every call after the first.
    pub fn load_initial(
        &self,
        config: &Config,
        progress: Option<&ProgressHandle>,
        cancel: &CancellationToken,
    ) -> Result<(), SpeechError> {
        #[cfg(feature = "test-fixtures")]
        if let Some(scripted) = &self.scripted {
            return self.state.load_scripted_shared(
                Arc::clone(&scripted.factory),
                scripted.policy,
                cancel,
            );
        }
        self.state.load_initial(config, progress, cancel)
    }

    /// Stops admitting work and waits for the published runtime to unload.
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

    /// Closes admission on the published runtime the way [`Self::shutdown`]
    /// does - cancelling the session epoch - without draining or retiring
    /// it, so a test can observe cancelled work while worker ownership
    /// persists.
    #[cfg(feature = "test-fixtures")]
    pub fn shutdown_admission(&self) {
        self.state.shutdown_admission();
    }

    /// Returns the batch and Realtime Gateway routes.
    #[cfg(not(miri))]
    pub fn routes(&self) -> axum::Router {
        crate::batch::routes(self.state.clone()).merge(crate::realtime::routes(
            self.state.clone(),
            self.sessions.clone(),
            self.realtime_policy.clone(),
        ))
    }
}
