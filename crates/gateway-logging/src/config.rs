//! The configuration input for [`LogRuntime::start`](crate::LogRuntime::start).

use std::path::{Path, PathBuf};

/// Previous runs retained beside the current log: `gateway.log.1` (the
/// newest rotation) through `gateway.log.5` (the oldest). A sixth
/// previous run is deleted by the rotation that would create it.
pub(crate) const RETAINED_RUNS: usize = 5;

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

    /// The log file this run writes: `<state dir>/logs/gateway.log`.
    ///
    /// # Examples
    /// ```
    /// let config = gateway_logging::LogConfig::new("/tmp/pf-state");
    /// assert_eq!(
    ///     config.log_path(),
    ///     std::path::Path::new("/tmp/pf-state").join("logs").join("gateway.log"),
    /// );
    /// ```
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.state_dir.join("logs").join("gateway.log")
    }

    /// The retained previous-run log paths, `gateway.log.1` (newest)
    /// through `gateway.log.5` (oldest). Diagnostics enumerates these
    /// without starting a runtime, so the log layout has exactly one
    /// owner.
    ///
    /// # Examples
    /// ```
    /// let config = gateway_logging::LogConfig::new("/tmp/pf-state");
    /// let retained = config.retained_log_paths();
    /// assert_eq!(retained.len(), 5);
    /// assert!(retained[0].ends_with("gateway.log.1"));
    /// assert!(retained[4].ends_with("gateway.log.5"));
    /// ```
    #[must_use]
    pub fn retained_log_paths(&self) -> Vec<PathBuf> {
        (1..=RETAINED_RUNS)
            .map(|run| {
                self.state_dir
                    .join("logs")
                    .join(format!("gateway.log.{run}"))
            })
            .collect()
    }

    /// Consumes the config into its state directory, so a one-shot caller
    /// such as [`LogRuntime::start`](crate::LogRuntime::start) moves
    /// instead of cloning.
    pub(crate) fn into_state_dir(self) -> PathBuf {
        self.state_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_layout_is_current_plus_five_retained_runs() {
        let config = LogConfig::new("state");
        assert_eq!(
            config.log_path(),
            Path::new("state").join("logs").join("gateway.log")
        );
        let retained = config.retained_log_paths();
        assert_eq!(
            retained,
            (1..=5)
                .map(|run| Path::new("state")
                    .join("logs")
                    .join(format!("gateway.log.{run}")))
                .collect::<Vec<_>>(),
            "the retained chain is .1 through .5 in rotation order"
        );
    }
}
