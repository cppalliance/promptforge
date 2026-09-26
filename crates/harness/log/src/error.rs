//! The run log's failure vocabulary.

use std::io;

use crate::RunId;

/// The database engine's error behind [`LogError::Database`], so the
/// public error surface names no engine type. Renders and sources exactly
/// as the engine's error does; [`as_inner`](Self::as_inner) restores
/// branching on the engine's own variant, which the chain walks past.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct DatabaseSource(turso::Error);

impl DatabaseSource {
    /// The wrapped engine error.
    #[must_use]
    pub fn as_inner(&self) -> &turso::Error {
        &self.0
    }

    /// Takes the wrapped engine error out of the wrapper.
    #[must_use]
    pub fn into_inner(self) -> turso::Error {
        self.0
    }
}

impl From<turso::Error> for DatabaseSource {
    fn from(source: turso::Error) -> Self {
        DatabaseSource(source)
    }
}

/// The JSON error behind [`LogError::Payload`], so the public error
/// surface names no `serde_json` type. Renders and sources exactly as the
/// JSON error does; [`as_inner`](Self::as_inner) restores serde's own
/// classification, which the chain walks past.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct JsonSource(serde_json::Error);

impl JsonSource {
    /// The wrapped JSON error.
    #[must_use]
    pub fn as_inner(&self) -> &serde_json::Error {
        &self.0
    }

    /// Takes the wrapped JSON error out of the wrapper.
    #[must_use]
    pub fn into_inner(self) -> serde_json::Error {
        self.0
    }
}

impl From<serde_json::Error> for JsonSource {
    fn from(source: serde_json::Error) -> Self {
        JsonSource(source)
    }
}

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

    use crate::{DatabaseSource, JsonSource, LogError};

    #[test]
    fn the_database_variant_reaches_the_engine_error_through_the_log_wrapper() {
        let engine = turso::Error::Corrupt("page 1 is not a b-tree page".to_owned());
        let rendered = engine.to_string();
        let error = LogError::from(engine);
        let Some(cause) = error.source() else {
            panic!("the database variant reports its engine cause as source()");
        };
        assert_eq!(cause.to_string(), rendered);
        let Some(wrapper) = cause.downcast_ref::<DatabaseSource>() else {
            panic!("the engine cause is harness-log's DatabaseSource");
        };
        assert!(matches!(wrapper.as_inner(), turso::Error::Corrupt(_)));
    }

    #[test]
    fn the_payload_variant_reaches_the_serde_error_through_the_log_wrapper() {
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
            panic!("the serde cause is harness-log's JsonSource");
        };
        assert!(wrapper.as_inner().is_syntax());
    }

    #[test]
    fn the_database_wrapper_hands_back_the_engine_error_it_wraps() {
        let engine = turso::Error::Corrupt("page 1 is not a b-tree page".to_owned());
        let rendered = engine.to_string();
        let inner = DatabaseSource::from(engine).into_inner();
        assert_eq!(inner.to_string(), rendered);
        assert!(matches!(inner, turso::Error::Corrupt(_)));
    }

    #[test]
    fn the_json_wrapper_hands_back_the_serde_error_it_wraps() {
        let Err(json) = serde_json::from_str::<u32>("nope") else {
            panic!("`nope` must not parse as a u32");
        };
        let rendered = json.to_string();
        let inner = JsonSource::from(json).into_inner();
        assert_eq!(inner.to_string(), rendered);
        assert!(inner.is_syntax());
    }
}
