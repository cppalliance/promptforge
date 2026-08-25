//! The session tape: an append-only JSONL record of chat round-trips.
//!
//! Every completed `POST /chat` round-trip is appended to the file named by
//! `tape.path` in `workshop.toml`, one JSON object per line. The tape is
//! observability, not a transaction log: a failed write is logged and
//! returned to the handler but never fails the user's chat request.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// One taped chat round-trip, serialized as a single JSON line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TapeEvent {
    /// When the round-trip completed, as an RFC 3339 UTC timestamp.
    pub ts: String,
    /// The event kind; `"chat"` for a chat completion round-trip.
    pub kind: String,
    /// The model the request named.
    pub model: String,
    /// The request body as the workshop received it.
    pub request: serde_json::Value,
    /// The gateway response body; a plain string if it was not JSON.
    pub response: serde_json::Value,
    /// Wall-clock latency of the gateway round-trip, in milliseconds.
    pub latency_ms: u64,
}

impl TapeEvent {
    /// Builds a `chat` event stamped with the current UTC time.
    ///
    /// `latency` saturates at `u64::MAX` milliseconds, which no gateway call
    /// can outlast.
    ///
    /// # Errors
    /// Returns [`TapeError::Timestamp`] if the current time cannot be
    /// rendered as RFC 3339.
    pub fn chat(
        model: String,
        request: serde_json::Value,
        response: serde_json::Value,
        latency: Duration,
    ) -> Result<Self, TapeError> {
        let ts = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|source| TapeError::Timestamp(Box::new(source)))?;
        Ok(Self {
            ts,
            kind: "chat".to_string(),
            model,
            request,
            response,
            latency_ms: u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
        })
    }
}

/// A tape open, timestamp, serialization, or append failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TapeError {
    /// The tape file could not be opened.
    #[non_exhaustive]
    #[error("open {}", path.display())]
    Open {
        /// The path that could not be opened.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The current time could not be rendered as an RFC 3339 timestamp.
    #[non_exhaustive]
    #[error("format tape timestamp")]
    Timestamp(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// An event could not be serialized to JSON.
    #[non_exhaustive]
    #[error("serialize tape event")]
    Serialize(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A serialized event could not be appended to the tape.
    #[non_exhaustive]
    #[error("write {}", path.display())]
    Write {
        /// The path that could not be written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Append-only JSONL tape writer shared across chat handlers.
///
/// Writes are serialized by a `std::sync::Mutex`; the critical section is a
/// single `write_all` with no `.await`, and callers on the async runtime are
/// expected to go through `tokio::task::spawn_blocking`.
pub struct Tape {
    path: PathBuf,
    writer: Mutex<Box<dyn Write + Send>>,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Tape>();
};

impl std::fmt::Debug for Tape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tape")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Tape {
    /// Opens the tape at `path` for appending, creating it if missing.
    ///
    /// # Errors
    /// Returns [`TapeError::Open`] if `path` cannot be opened, including when
    /// its parent directory does not exist.
    ///
    /// # Examples
    /// ```
    /// let dir = tempfile::TempDir::new()?;
    /// let tape = promptforge_ws_server::Tape::open(&dir.path().join("tape.jsonl"))?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn open(path: &Path) -> Result<Self, TapeError> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| TapeError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self {
            path: path.to_path_buf(),
            writer: Mutex::new(Box::new(file)),
        })
    }

    /// Serializes `event` and appends it as one line.
    ///
    /// The line is built before the lock is taken, so the critical section is
    /// a single `write_all` and concurrent events never interleave byte-wise.
    /// A poisoned lock is recovered: the tape keeps appending.
    ///
    /// # Errors
    /// Returns [`TapeError::Serialize`] if the event cannot be serialized and
    /// [`TapeError::Write`] if the append fails.
    pub fn record(&self, event: &TapeEvent) -> Result<(), TapeError> {
        let mut line = serde_json::to_string(event)
            .map_err(|source| TapeError::Serialize(Box::new(source)))?;
        line.push('\n');
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        writer
            .write_all(line.as_bytes())
            .map_err(|source| TapeError::Write {
                path: self.path.clone(),
                source,
            })
    }

    /// Builds a tape around an arbitrary writer, for failure-injection tests.
    #[cfg(test)]
    pub(crate) fn with_writer_for_test(writer: impl Write + Send + 'static) -> Self {
        Self {
            path: PathBuf::from("<test>"),
            writer: Mutex::new(Box::new(writer)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat_event(model: &str) -> TapeEvent {
        TapeEvent::chat(
            model.to_string(),
            serde_json::json!({"model": model, "messages": []}),
            serde_json::json!({"id": "chatcmpl-1"}),
            Duration::from_millis(7),
        )
        .expect("the current time formats as RFC 3339")
    }

    #[test]
    fn events_are_appended_as_valid_jsonl() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("tape.jsonl");
        let tape = Tape::open(&path).expect("open tape");
        tape.record(&chat_event("m1")).expect("record m1");
        tape.record(&chat_event("m2")).expect("record m2");

        let raw = std::fs::read_to_string(&path).expect("read the tape back");
        assert!(raw.ends_with('\n'), "the last line is complete: {raw:?}");
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 2, "one line per event");
        let mut models = Vec::new();
        for line in lines {
            assert!(!line.is_empty(), "no blank lines");
            let value: serde_json::Value =
                serde_json::from_str(line).expect("every line is valid JSON");
            assert_eq!(value["kind"], "chat");
            assert_eq!(value["request"]["model"], value["model"]);
            assert_eq!(value["response"]["id"], "chatcmpl-1");
            assert!(value["latency_ms"].is_u64(), "latency_ms is an integer");
            let ts = value["ts"].as_str().expect("ts is a string");
            OffsetDateTime::parse(ts, &Rfc3339).expect("ts is RFC 3339");
            models.push(
                value["model"]
                    .as_str()
                    .expect("model is a string")
                    .to_string(),
            );
        }
        assert_eq!(models, ["m1".to_string(), "m2".to_string()], "append order");
    }

    #[test]
    fn reopening_appends_instead_of_truncating() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("tape.jsonl");
        {
            let tape = Tape::open(&path).expect("open tape");
            tape.record(&chat_event("first")).expect("record first");
        }
        let tape = Tape::open(&path).expect("reopen tape");
        tape.record(&chat_event("second")).expect("record second");
        let raw = std::fs::read_to_string(&path).expect("read the tape back");
        assert_eq!(raw.lines().count(), 2, "both opens append");
    }

    #[test]
    fn opening_inside_a_missing_directory_is_an_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let err = Tape::open(&dir.path().join("missing").join("tape.jsonl"))
            .expect_err("a missing parent directory must fail");
        assert!(
            matches!(err, TapeError::Open { .. }),
            "expected Open, got {err:?}"
        );
    }

    #[test]
    fn a_write_failure_is_returned_to_the_caller() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected tape failure"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let tape = Tape::with_writer_for_test(FailingWriter);
        let err = tape
            .record(&chat_event("m1"))
            .expect_err("the write failure must surface");
        assert!(
            matches!(err, TapeError::Write { .. }),
            "expected Write, got {err:?}"
        );
    }
}
