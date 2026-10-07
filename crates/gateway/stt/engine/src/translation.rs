//! Backend failure translation into engine-owned errors.

use std::path::PathBuf;

use crate::{DetectorError, TranscribeError};

impl TranscribeError {
    /// Returns whether startup exceeded a deadline and abandoned at least one
    /// non-preemptible worker construction call.
    #[must_use]
    pub fn is_non_preemptible_startup_timeout(&self) -> bool {
        match self {
            Self::InterimStartupTimedOut | Self::FinalStartupTimedOut => true,
            Self::StartupFailures { failures, .. } => failures
                .iter()
                .any(Self::is_non_preemptible_startup_timeout),
            Self::StartupCleanup {
                startup, cleanup, ..
            } => {
                startup.is_non_preemptible_startup_timeout()
                    || cleanup.iter().any(Self::is_non_preemptible_startup_timeout)
            }
            Self::ShutdownFailures { cleanup, .. } => {
                cleanup.iter().any(Self::is_non_preemptible_startup_timeout)
            }
            _ => false,
        }
    }

    /// Translates a backend initialization source.
    pub fn initialize_backend(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::InitializeBackend(Box::new(source))
    }

    /// Translates a model construction source while preserving its path.
    pub fn load_model(
        path: PathBuf,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::LoadModel {
            path,
            source: Box::new(source),
        }
    }

    /// Translates a backend inference source.
    pub fn inference(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Inference(Box::new(source))
    }
}

impl DetectorError {
    /// Translates a detector construction failure message.
    pub fn load(message: impl Into<String>) -> Self {
        Self::Load(message.into())
    }

    /// Translates a detector classification failure message.
    pub fn inference(message: impl Into<String>) -> Self {
        Self::Inference(message.into())
    }
}
