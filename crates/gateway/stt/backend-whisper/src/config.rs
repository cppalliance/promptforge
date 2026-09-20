//! Safe Whisper backend construction values.

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use shared_progress::Activity;

/// Provisioned Whisper runtime, model paths, and optional load progress.
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    pub(crate) library: PathBuf,
    pub(crate) interim_model: PathBuf,
    pub(crate) final_model: Option<PathBuf>,
    /// The load's activity, weakly held: the factory outlives the load, so
    /// a decoder built after the caller's guard dropped reports nothing.
    pub(crate) progress: Option<Weak<Activity>>,
}

impl WhisperConfig {
    /// Creates a backend configuration from provisioned artifact paths.
    #[must_use]
    pub fn new(
        library: PathBuf,
        interim_model: PathBuf,
        final_model: Option<PathBuf>,
        progress: Option<Weak<Activity>>,
    ) -> Self {
        Self {
            library,
            interim_model,
            final_model,
            progress,
        }
    }

    /// The load's activity while its owner's guard is alive, `None` once the
    /// guard dropped or no progress was configured, so a decoder built
    /// after the load ended reports nothing.
    pub(crate) fn live_progress(&self) -> Option<Arc<Activity>> {
        self.progress.as_ref().and_then(Weak::upgrade)
    }
}
