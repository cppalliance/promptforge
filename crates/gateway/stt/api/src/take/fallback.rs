//! A take's report that it detects speech by loudness instead of Silero.

use std::sync::{Arc, OnceLock};

use gateway_progress::{Activity, ProgressHub};
use gateway_stt_engine::DetectorError;

/// Reports a take's fall back to loudness through logs and, when its
/// generation has a hub, gateway progress.
///
/// The progress activity lasts until the take ends: the Workshop never
/// shows an activity shorter than its show delay.
#[derive(Debug, Default)]
pub(crate) struct FallbackReport {
    hub: Option<Arc<ProgressHub>>,
    activity: OnceLock<Activity>,
}

impl FallbackReport {
    pub(crate) fn new(hub: Option<Arc<ProgressHub>>) -> Self {
        Self {
            hub,
            activity: OnceLock::new(),
        }
    }

    /// Logs `message` with `error` and shows both as progress until the
    /// take ends.
    pub(crate) fn report(&self, message: &str, error: &DetectorError) {
        tracing::warn!(%error, "{message}");
        if let Some(hub) = &self.hub {
            drop(self.activity.set(hub.begin(format!("{message}: {error}"))));
        }
    }
}
