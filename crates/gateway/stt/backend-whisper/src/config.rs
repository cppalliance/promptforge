//! Safe Whisper backend construction values.

use std::path::PathBuf;

use shared_progress::ProgressHandle;

/// Provisioned Whisper runtime, model paths, and optional load progress.
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    pub(crate) library: PathBuf,
    pub(crate) interim_model: PathBuf,
    pub(crate) final_model: Option<PathBuf>,
    pub(crate) progress: Option<ProgressHandle>,
}

impl WhisperConfig {
    /// Creates a backend configuration from provisioned artifact paths.
    #[must_use]
    pub fn new(
        library: PathBuf,
        interim_model: PathBuf,
        final_model: Option<PathBuf>,
        progress: Option<ProgressHandle>,
    ) -> Self {
        Self {
            library,
            interim_model,
            final_model,
            progress,
        }
    }
}
