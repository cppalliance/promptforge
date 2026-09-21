//! The run log's failure vocabulary.

use std::io;

// The engine and serde causes behind the variants below. A caller that
// needs the underlying error names `shared_error_source` directly; this
// crate does not re-export the wrappers, so there is one name for the
// cause across the workspace rather than one per crate.
use shared_error_source::{DatabaseSource, JsonSource};

use crate::RunId;

/// Why a run log operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LogError {
    /// The database engine refused an operation; the engine's error is
    /// the source.
    #[error("run log database")]
    Database {
        /// The engine's error.
        #[source]
        source: DatabaseSource,
    },
    /// The log file could not be addressed; the I/O error is the source.
    #[error("run log file")]
    Io {
        /// The I/O error.
        #[from]
        source: io::Error,
    },
    /// A payload could not be serialized on the way in or parsed on the
    /// way out; the serde error is the source.
    #[error("run log payload")]
    Payload {
        /// The serde error.
        #[source]
        source: JsonSource,
    },
    /// No run with this id was ever begun in this log.
    #[error("run log: unknown run {0}")]
    UnknownRun(RunId),
    /// The run has already ended; nothing more may be written to it.
    #[error("run log: run {0} has ended")]
    RunEnded(RunId),
    /// A stored row disagrees with the schema: expected the named shape,
    /// found the described value.
    #[error("run log: corrupt row: {0}")]
    Corrupt(String),
}

impl From<turso::Error> for LogError {
    fn from(source: turso::Error) -> Self {
        LogError::Database {
            source: source.into(),
        }
    }
}

impl From<serde_json::Error> for LogError {
    fn from(source: serde_json::Error) -> Self {
        LogError::Payload {
            source: source.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use shared_error_source::{DatabaseSource, JsonSource};

    use super::LogError;

    #[test]
    fn the_database_variant_reaches_the_engine_error_through_the_shared_wrapper() {
        let error = LogError::from(turso::Error::Corrupt(
            "page 1 is not a b-tree page".to_owned(),
        ));
        let Some(cause) = error.source() else {
            panic!("the database variant reports its engine cause as source()");
        };
        let Some(wrapper) = cause.downcast_ref::<DatabaseSource>() else {
            panic!("the engine cause is the shared DatabaseSource");
        };
        assert!(matches!(wrapper.as_inner(), turso::Error::Corrupt(_)));
    }

    #[test]
    fn the_payload_variant_reaches_the_serde_error_through_the_shared_wrapper() {
        let Err(json) = serde_json::from_str::<u32>("nope") else {
            panic!("`nope` must not parse as a u32");
        };
        let error = LogError::from(json);
        let Some(cause) = error.source() else {
            panic!("the payload variant reports its serde cause as source()");
        };
        let Some(wrapper) = cause.downcast_ref::<JsonSource>() else {
            panic!("the serde cause is the shared JsonSource");
        };
        assert!(wrapper.as_inner().is_syntax());
    }
}
