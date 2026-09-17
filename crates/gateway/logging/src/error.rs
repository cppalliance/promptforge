//! The opaque error type returned by [`LogRuntime`](crate::LogRuntime).

use std::fmt;
use std::io;
use std::path::PathBuf;

/// A failure to start or shut down a [`LogRuntime`](crate::LogRuntime).
///
/// Opaque on purpose: the sources stay private so the crate's I/O shape can
/// change without a breaking change, and callers classify with
/// [`is_io`](Self::is_io) instead of matching variants.
#[derive(Debug)]
pub struct LogError(Repr);

#[derive(Debug)]
enum Repr {
    /// Creating `logs/`, rotating the previous log, or opening the fresh
    /// one failed.
    Open { path: PathBuf, source: io::Error },
    /// The worker thread failed to spawn.
    Spawn(io::Error),
    /// The worker thread panicked instead of joining cleanly.
    WorkerPanicked,
}

impl LogError {
    pub(crate) fn open(path: PathBuf, source: io::Error) -> Self {
        Self(Repr::Open { path, source })
    }

    pub(crate) fn spawn(source: io::Error) -> Self {
        Self(Repr::Spawn(source))
    }

    pub(crate) fn worker_panicked() -> Self {
        Self(Repr::WorkerPanicked)
    }

    /// Whether the failure came from an operating-system resource (the
    /// filesystem or thread spawn) rather than a worker panic.
    ///
    /// # Examples
    /// ```no_run
    /// # let config = gateway_logging::LogConfig::new("/tmp/pf-state");
    /// match gateway_logging::LogRuntime::start(config) {
    ///     Ok(runtime) => drop(runtime),
    ///     Err(error) if error.is_io() => eprintln!("log file unavailable: {error}"),
    ///     Err(error) => eprintln!("logging failed: {error}"),
    /// }
    /// ```
    #[must_use]
    pub fn is_io(&self) -> bool {
        matches!(self.0, Repr::Open { .. } | Repr::Spawn(_))
    }
}

impl fmt::Display for LogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Open { path, .. } => {
                write!(f, "could not open the log file {}", path.display())
            }
            Repr::Spawn(_) => f.write_str("could not spawn the log worker thread"),
            Repr::WorkerPanicked => f.write_str("the log worker thread panicked"),
        }
    }
}

impl std::error::Error for LogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0 {
            Repr::Open { source, .. } | Repr::Spawn(source) => Some(source),
            Repr::WorkerPanicked => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn is_io_separates_os_failures_from_worker_panics() {
        assert!(
            LogError::open(PathBuf::from("gateway.log"), io::Error::other("denied")).is_io(),
            "a filesystem failure classifies as I/O"
        );
        assert!(
            LogError::spawn(io::Error::other("no threads")).is_io(),
            "a thread-spawn failure classifies as I/O"
        );
        assert!(
            !LogError::worker_panicked().is_io(),
            "a worker panic is not an I/O failure"
        );
    }

    #[test]
    fn the_source_chain_reaches_the_io_cause() {
        let error = LogError::open(PathBuf::from("gateway.log"), io::Error::other("denied"));
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("denied"),
            "the wrapped I/O error stays on the chain"
        );
        assert!(
            LogError::worker_panicked().source().is_none(),
            "a worker panic carries no source"
        );
    }
}
