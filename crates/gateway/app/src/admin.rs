//! The admin surface: profile listing and switching, the status readout,
//! the progress stream, the running-config view, and queue cancellation.

pub(crate) mod config;
pub(crate) mod profiles;
pub(crate) mod progress;
pub(crate) mod queue;
pub(crate) mod status;

use crate::AppState;
use crate::error::GatewayError;

/// Single configuration file used by admin routes and profile persistence.
#[derive(Debug)]
pub(crate) struct AdminConfig {
    pub(crate) path: std::path::PathBuf,
}

pub(crate) fn config_path(state: &AppState) -> Result<&std::path::Path, GatewayError> {
    state
        .config
        .as_ref()
        .map(|config| config.path.as_path())
        .ok_or(GatewayError::ConfigPathUnavailable)
}
