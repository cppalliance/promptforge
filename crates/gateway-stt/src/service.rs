//! Cloneable host facade for speech lifecycle, facts, and routes.

use gateway_config::Config;
use shared_progress::ProgressHandle;

use crate::artifacts::{self, PreparedSpeech, SpeechError};
use crate::generation::{GenerationState, SpeechReplacement};
use crate::model::SpeechModelInfo;
use crate::status::SpeechStatus;

/// Cloneable Gateway handle for all speech behavior.
#[derive(Debug, Clone, Default)]
pub struct SpeechService {
    pub(crate) state: GenerationState,
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

    /// Publishes every fact in a staged generation through one transition.
    ///
    /// # Errors
    /// Returns an ownership error for a foreign or shutdown-invalidated token.
    pub fn commit_replacement(&self, replacement: SpeechReplacement) -> Result<(), SpeechError> {
        self.state.commit(replacement)
    }

    /// Drops a staged generation without publishing it.
    pub fn abort_replacement(&self, replacement: SpeechReplacement) {
        drop(replacement);
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

    /// Returns physical models from one point-in-time generation snapshot.
    #[must_use]
    pub fn models(&self) -> Vec<SpeechModelInfo> {
        self.state.models()
    }

    /// Returns the batch and temporary legacy Gateway routes.
    #[cfg(not(miri))]
    pub fn routes(&self) -> axum::Router {
        crate::batch::routes(self.state.clone())
            .merge(crate::stt::gateway_router(self.state.clone()))
    }

    /// Returns the temporary Workshop-hosted legacy routes.
    #[cfg(not(miri))]
    pub fn workshop_routes(&self, push: workshop_server::Push) -> axum::Router {
        crate::stt::workshop_router(self.state.clone(), push)
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn scripted_replacement(
        &self,
        engine: gateway_stt_engine::SttEngine,
        final_model: Option<String>,
    ) -> Result<SpeechReplacement, SpeechError> {
        self.state.stage_loaded_scripted(
            engine,
            "scripted-interim".to_owned(),
            final_model,
            Vec::new(),
            std::time::Duration::from_secs(30),
        )
    }
}
