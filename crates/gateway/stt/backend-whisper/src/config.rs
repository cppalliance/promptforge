//! Safe Whisper backend construction values.

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use gateway_progress::Activity;

/// Provisioned Whisper runtime, model paths, interim window, and optional
/// load progress.
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    pub(crate) library: PathBuf,
    pub(crate) interim_model: PathBuf,
    pub(crate) final_model: Option<PathBuf>,
    /// The configured interim window, which sizes the interim decode's
    /// encoder context and token budget.
    pub(crate) window_seconds: u64,
    /// The load's activity, weakly held: the factory outlives the load, so
    /// a decoder built after the caller's guard dropped reports nothing.
    progress: Option<Weak<Activity>>,
}

impl WhisperConfig {
    /// Creates a backend configuration from provisioned artifact paths and
    /// the configured interim window.
    #[must_use]
    pub fn new(
        library: PathBuf,
        interim_model: PathBuf,
        final_model: Option<PathBuf>,
        window_seconds: u64,
        progress: Option<Weak<Activity>>,
    ) -> Self {
        Self {
            library,
            interim_model,
            final_model,
            window_seconds,
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
