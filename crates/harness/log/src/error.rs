//! The run log's failure vocabulary.

use std::io;

use crate::RunId;

/// Why a run log operation failed.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// The database engine refused an operation.
    #[error("run log database: {source}")]
    Database {
        /// The engine's error.
        #[from]
        source: turso::Error,
    },
    /// The log file could not be addressed.
    #[error("run log file: {source}")]
    Io {
        /// The I/O error.
        #[from]
        source: io::Error,
    },
    /// A payload could not be serialized on the way in or parsed on the
    /// way out.
    #[error("run log payload: {source}")]
    Payload {
        /// The serde error.
        #[from]
        source: serde_json::Error,
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
