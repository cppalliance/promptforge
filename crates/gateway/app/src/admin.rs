//! The admin surface, in its two tiers.
//!
//! [`open`] holds the routes any authenticated caller may reach: profile
//! listing and switching, the status readout, the progress stream, and
//! queue cancellation. [`walled`] holds the routes that read secrets in
//! plaintext, write files, or launch processes; `build_router` mounts its
//! router behind the shared loopback wall in every build, so a
//! non-loopback peer is refused with 403 before bearer auth runs. A module
//! under `walled/` is loopback-only by its path, and its handlers say so
//! again by extracting [`crate::auth::LoopbackCaller`].

pub(crate) mod open;
pub(crate) mod walled;

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
