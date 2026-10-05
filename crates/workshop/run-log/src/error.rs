//! The run log's failure vocabulary.

use std::io;

// The causes behind `LogError::Database` and `LogError::Payload`. A
// caller that needs the database's or serde's own error names
// `shared_error_source` directly; this crate does not re-export the
// wrappers, so there is one name for each cause across the workspace.
use shared_error_source::{DatabaseSource, JsonSource};

use crate::RunId;

/// Why a run log operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LogError {
    /// The database refused an operation; the database's error is
    /// the source.
    #[error("run log database")]
    Database {
        /// The database's error.
        #[source]
        source: DatabaseSource,
    },
    /// The log file could not be addressed; the I/O error is the source.
    #[error("run log file")]
    Io {
        /// The I/O error.
        #[source]
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

impl From<io::Error> for LogError {
    fn from(source: io::Error) -> Self {
        LogError::Io { source }
    }
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

    use crate::LogError;

    #[test]
    fn the_database_variant_reaches_the_database_error_through_the_shared_wrapper() {
        let database = turso::Error::Corrupt("page 1 is not a b-tree page".to_owned());
        let rendered = database.to_string();
        let error = LogError::from(database);
        let Some(cause) = error.source() else {
            panic!("the database variant reports its database cause as source()");
        };
        assert_eq!(cause.to_string(), rendered);
        let Some(wrapper) = cause.downcast_ref::<DatabaseSource>() else {
            panic!("the database cause is shared_error_source's DatabaseSource");
        };
        assert!(matches!(wrapper.as_inner(), turso::Error::Corrupt(_)));
    }

    #[test]
    fn the_payload_variant_reaches_the_serde_error_through_the_shared_wrapper() {
        let Err(json) = serde_json::from_str::<u32>("nope") else {
            panic!("`nope` must not parse as a u32");
        };
        let rendered = json.to_string();
        let error = LogError::from(json);
        let Some(cause) = error.source() else {
            panic!("the payload variant reports its serde cause as source()");
        };
        assert_eq!(cause.to_string(), rendered);
        let Some(wrapper) = cause.downcast_ref::<JsonSource>() else {
            panic!("the serde cause is shared_error_source's JsonSource");
        };
        assert!(wrapper.as_inner().is_syntax());
    }
}
