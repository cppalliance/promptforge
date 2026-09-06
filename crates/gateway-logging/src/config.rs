//! The configuration input for [`LogRuntime::start`](crate::LogRuntime::start).

use std::path::{Path, PathBuf};

/// The one input logging needs: the gateway state directory that holds
/// `logs/`.
///
/// The directory is deliberately the only knob: log discovery must work
/// before configuration parses, so the log location is never configurable.
#[derive(Debug, Clone)]
pub struct LogConfig {
    state_dir: PathBuf,
}

impl LogConfig {
    /// Builds a config rooted at `state_dir`; the log file lives at
    /// `state_dir/logs/gateway.log`.
    ///
    /// # Examples
    /// ```
    /// let config = gateway_logging::LogConfig::new("/tmp/pf-state");
    /// assert_eq!(config.state_dir(), std::path::Path::new("/tmp/pf-state"));
    /// ```
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
        }
    }

    /// The state directory the log file is rooted under.
    ///
    /// # Examples
    /// ```
    /// let config = gateway_logging::LogConfig::new("/tmp/pf-state");
    /// assert_eq!(config.state_dir(), std::path::Path::new("/tmp/pf-state"));
    /// ```
    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Consumes the config into its state directory, so a one-shot caller
    /// such as [`LogRuntime::start`](crate::LogRuntime::start) moves
    /// instead of cloning.
    pub(crate) fn into_state_dir(self) -> PathBuf {
        self.state_dir
    }
}
