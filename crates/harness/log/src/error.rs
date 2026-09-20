//! The run log's failure vocabulary.

use std::io;

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
        source: PayloadSource,
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

/// The database engine's error behind [`LogError::Database`], owned by
/// this crate so the public vocabulary names no third-party type. Renders
/// and sources exactly as the engine's error does.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct DatabaseSource(turso::Error);

/// The serde error behind [`LogError::Payload`], owned by this crate so
/// the public vocabulary names no third-party type. Renders and sources
/// exactly as the serde error does.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct PayloadSource(serde_json::Error);

impl From<turso::Error> for LogError {
    fn from(source: turso::Error) -> Self {
        LogError::Database {
            source: DatabaseSource(source),
        }
    }
}

impl From<serde_json::Error> for LogError {
    fn from(source: serde_json::Error) -> Self {
        LogError::Payload {
            source: PayloadSource(source),
        }
    }
}
