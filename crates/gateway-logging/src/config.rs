//! The configuration input for [`LogRuntime::start`](crate::LogRuntime::start).

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Numbered segments retained beside `gateway.log`: `.1` is newest and
/// `.5` is oldest. Admitting a sixth retained segment prunes `.5`.
pub(crate) const RETAINED_SEGMENTS: usize = 5;

/// Marks a segment boundary or a retained tail whose earlier bytes were
/// discarded to restore the fixed-size invariant.
pub(crate) const SEGMENT_TRUNCATION_MARKER: &str = " [truncated]\n";

/// Every memory, latency, and disk budget for the logging pipeline.
///
/// Keeping these limits in one immutable value makes later queue, timeout,
/// shutdown, and rotation work consume the same policy without adding
/// configuration before logging is available.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LogLimits {
    pub(crate) max_formatted_record_bytes: usize,
    pub(crate) max_queued_bytes: usize,
    pub(crate) producer_wait: Duration,
    pub(crate) shutdown_wait: Duration,
    pub(crate) segment_bytes: u64,
    pub(crate) aggregate_retained_bytes: u64,
}

/// The process-wide logging policy. The queue, timeout, shutdown, and
/// rotation paths consume their reserved fields as those bounds are
/// enforced.
pub(crate) const LOG_LIMITS: LogLimits = LogLimits {
    max_formatted_record_bytes: 64 * 1024,
    max_queued_bytes: 32 * 1024 * 1024,
    producer_wait: Duration::from_millis(25),
    shutdown_wait: Duration::from_secs(2),
    segment_bytes: 16 * 1024 * 1024,
    aggregate_retained_bytes: 96 * 1024 * 1024,
};

const _: () = {
    assert!(LOG_LIMITS.max_formatted_record_bytes <= LOG_LIMITS.max_queued_bytes);
    assert!(LOG_LIMITS.producer_wait.as_millis() < LOG_LIMITS.shutdown_wait.as_millis());
    assert!(
        LOG_LIMITS.aggregate_retained_bytes
            == LOG_LIMITS.segment_bytes * (RETAINED_SEGMENTS as u64 + 1)
    );
    assert!(
        LOG_LIMITS.segment_bytes
            >= LOG_LIMITS.max_formatted_record_bytes as u64
                + SEGMENT_TRUNCATION_MARKER.len() as u64
    );
};

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

    /// The retained log segment paths, `gateway.log.1` (newest) through
    /// `gateway.log.5` (oldest). Diagnostics enumerates these without
    /// starting a runtime, so the log layout has exactly one owner.
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
        (1..=RETAINED_SEGMENTS)
            .map(|segment| {
                self.state_dir
                    .join("logs")
                    .join(format!("gateway.log.{segment}"))
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
    fn the_log_layout_is_current_plus_five_numbered_segments() {
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
