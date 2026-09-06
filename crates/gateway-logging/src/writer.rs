//! The `MakeWriter` adapter between the binary's fmt layer and the queue.

use std::io;
use std::sync::Arc;

use tracing::Metadata;
use tracing_subscriber::fmt::MakeWriter;

use crate::queue::{LogPriority, LogQueue};
use crate::redact::redact_line;

/// A cloneable factory that hands the fmt layer per-event writers feeding
/// the queue.
///
/// Priority comes only from the event's tracing metadata; the formatted
/// text passes through the privacy redaction before it can reach the
/// queue. Obtained from
/// [`LogRuntime::writer`](crate::LogRuntime::writer).
///
/// # Examples
/// ```
/// # let dir = std::env::temp_dir().join(concat!("gateway-logging-doc-writer-", env!("CARGO_PKG_VERSION")));
/// let runtime = gateway_logging::LogRuntime::start(gateway_logging::LogConfig::new(&dir))?;
/// let writer = runtime.writer();
/// let _clone = writer.clone();
/// runtime.shutdown()?;
/// # std::fs::remove_dir_all(&dir).ok();
/// # Ok::<(), gateway_logging::LogError>(())
/// ```
#[derive(Debug, Clone)]
pub struct LogWriter {
    queue: Arc<LogQueue>,
}

impl LogWriter {
    pub(crate) fn new(queue: Arc<LogQueue>) -> Self {
        Self { queue }
    }
}

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = LogEventWriter;

    fn make_writer(&'a self) -> LogEventWriter {
        LogEventWriter::new(Arc::clone(&self.queue), LogPriority::Info)
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> LogEventWriter {
        LogEventWriter::new(
            Arc::clone(&self.queue),
            LogPriority::from_level(*meta.level()),
        )
    }
}

/// Buffers every `Write` call for one formatted event and enqueues the
/// owned line on drop, so a partial formatter write never becomes a
/// partial queue record.
///
/// Not public API: `MakeWriter::Writer` cannot name a private type, so the
/// compiler forces this onto the public surface; it is `#[doc(hidden)]`
/// and constructible only through [`LogWriter`].
#[doc(hidden)]
#[derive(Debug)]
pub struct LogEventWriter {
    queue: Arc<LogQueue>,
    priority: LogPriority,
    buffer: Vec<u8>,
}

impl LogEventWriter {
    fn new(queue: Arc<LogQueue>, priority: LogPriority) -> Self {
        Self {
            queue,
            priority,
            buffer: Vec::new(),
        }
    }
}

impl io::Write for LogEventWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LogEventWriter {
    fn drop(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        // The formatter's output is almost always valid UTF-8, so move the
        // buffer into the record and pay the lossy copy only when it is not.
        let line = match String::from_utf8(std::mem::take(&mut self.buffer)) {
            Ok(text) => text,
            Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
        };
        // The privacy chokepoint: every record crosses here, so the
        // well-shaped secrets are masked before they can reach the queue.
        self.queue
            .enqueue(self.priority, redact_line(&line).into_boxed_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn drop_enqueues_one_record_for_many_partial_writes() {
        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        {
            let mut event = MakeWriter::make_writer(&writer);
            event.write_all(b"partial ").expect("buffered write");
            event.write_all(b"writes\n").expect("buffered write");
            event.flush().expect("flush is a no-op");
            // Nothing is enqueued until the writer drops.
            assert!(
                queue.is_empty(),
                "a partial write is never a partial record"
            );
        }
        queue.close();
        let batch = queue.take_batch();
        assert_eq!(
            batch.records.len(),
            1,
            "one formatted event is exactly one queue record"
        );
        assert_eq!(&*batch.records[0].line, "partial writes\n");
        assert_eq!(batch.records[0].priority, LogPriority::Info);
    }

    #[test]
    fn priority_comes_from_the_event_metadata() {
        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(writer)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!("an error");
            tracing::warn!("a warning");
            tracing::info!("an info");
            tracing::debug!("a debug");
            tracing::trace!("a trace");
        });
        queue.close();
        let batch = queue.take_batch();
        let priorities: Vec<LogPriority> =
            batch.records.iter().map(|record| record.priority).collect();
        assert_eq!(
            priorities,
            [
                LogPriority::Error,
                LogPriority::Warn,
                LogPriority::Info,
                LogPriority::Debug,
                LogPriority::Trace,
            ],
            "make_writer_for derives the lane from tracing metadata alone"
        );
        assert!(
            batch.records[0].line.contains("an error"),
            "the record carries the formatted event: {}",
            batch.records[0].line
        );
    }

    #[test]
    fn a_secret_in_an_event_is_masked_before_it_reaches_the_queue() {
        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(writer)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                authorization = "Bearer tok_secret_9f8c",
                "upstream rejected the key"
            );
            tracing::info!("sending Cookie: session=abc123 to the upstream");
        });
        queue.close();
        let batch = queue.take_batch();
        assert_eq!(batch.records.len(), 2);
        for record in &batch.records {
            assert!(
                !record.line.contains("tok_secret_9f8c"),
                "a bearer token in event fields never reaches a record: {}",
                record.line
            );
            assert!(
                !record.line.contains("abc123"),
                "a cookie value in a message never reaches a record: {}",
                record.line
            );
        }
        assert!(
            batch.records[0].line.contains("[redacted]"),
            "the mask marks where the secret stood: {}",
            batch.records[0].line
        );
    }
}
