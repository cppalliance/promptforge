//! The connection-file and launch-lock error type.
//!
//! [`SidecarError`] is what the file and lock operations return; the health
//! probe has its own [`crate::HealthError`].

use std::path::PathBuf;
use std::time::Duration;

/// A failure of a connection-file or launch-lock operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SidecarError {
    /// The run directory could not be created.
    #[error("create the run directory {path}")]
    CreateDir {
        /// The directory that could not be created.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The connection file existed but could not be read.
    #[error("read {path}")]
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The connection file was not valid JSON.
    #[error("parse {path}")]
    Parse {
        /// The file that could not be parsed.
        path: PathBuf,
        /// The underlying JSON error.
        #[source]
        source: serde_json::Error,
    },

    /// The connection file failed validation.
    #[error("invalid connection file {path}: {reason}")]
    Invalid {
        /// The file that failed validation.
        path: PathBuf,
        /// The broken invariant.
        reason: String,
    },

    /// The connection file could not be serialized for writing.
    #[error("serialize the connection file")]
    Serialize {
        /// The underlying JSON error.
        #[source]
        source: serde_json::Error,
    },

    /// The atomic write of the connection file failed.
    #[error("write {path}")]
    Write {
        /// The file that could not be written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The connection file could not be removed.
    #[error("remove {path}")]
    Remove {
        /// The file that could not be removed.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The launch lock could not be opened or taken.
    #[error("lock {path}")]
    Lock {
        /// The lock file that failed.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The launch race produced no attachable gateway within the budget.
    #[error("no launch winner became attachable within {timeout:?}")]
    LaunchTimeout {
        /// The budget that elapsed.
        timeout: Duration,
    },
}
