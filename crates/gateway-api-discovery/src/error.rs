//! The gateway-discovery-file and launch-lock error type.
//!
//! [`SidecarError`] is what the file and lock operations return; the health
//! probe has its own [`crate::HealthError`].

use std::path::PathBuf;
use std::time::Duration;

// The JSON cause behind a `SidecarError` variant. A caller that needs the
// JSON error itself names `shared_error_source` directly; this crate does not
// re-export the wrapper, so there is one name for the cause across the
// workspace rather than one per crate.
use shared_error_source::JsonSource;

/// A failure of a gateway-discovery-file or launch-lock operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SidecarError {
    /// The caller cancelled the sidecar operation.
    #[error("the sidecar operation was cancelled")]
    Cancelled,

    /// The run directory could not be created.
    #[error("create the run directory {path}")]
    CreateDir {
        /// The directory that could not be created.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The gateway discovery file existed but could not be read.
    #[error("read {path}")]
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The gateway discovery file was not valid JSON.
    #[error("parse {path}")]
    Parse {
        /// The file that could not be parsed.
        path: PathBuf,
        /// The underlying JSON error.
        #[source]
        source: JsonSource,
    },

    /// The gateway discovery file failed validation.
    #[error("invalid gateway discovery file {path}: {reason}")]
    Invalid {
        /// The file that failed validation.
        path: PathBuf,
        /// The broken invariant.
        reason: String,
    },

    /// The gateway discovery file could not be serialized for writing.
    #[error("serialize the gateway discovery file")]
    Serialize {
        /// The underlying JSON error.
        #[source]
        source: JsonSource,
    },

    /// The atomic write of the gateway discovery file failed.
    #[error("write {path}")]
    Write {
        /// The file that could not be written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The gateway discovery file could not be removed.
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

#[cfg(test)]
mod tests {
    use super::{JsonSource, SidecarError};
    use std::error::Error as _;
    use std::path::PathBuf;

    #[test]
    fn the_parse_variant_reaches_the_json_error_through_the_shared_wrapper() {
        let Err(json) = serde_json::from_str::<u32>("nope") else {
            panic!("`nope` must not parse as a u32");
        };
        let error = SidecarError::Parse {
            path: PathBuf::from("gateway.json"),
            source: json.into(),
        };
        let Some(cause) = error.source() else {
            panic!("the parse variant returns its JSON cause from source()");
        };
        let Some(wrapper) = cause.downcast_ref::<JsonSource>() else {
            panic!("the JSON cause is the shared JsonSource");
        };
        assert!(wrapper.as_inner().is_syntax());
    }
}
