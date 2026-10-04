//! The `MakeWriter` adapter between the binary's fmt layer and the queue.

use std::fmt;
use std::io;
use std::sync::Arc;

use tracing::Metadata;
use tracing::field::{Field, Visit};
use tracing_subscriber::field::{RecordFields, VisitOutput};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::{DefaultVisitor, FormatFields, Writer};

use crate::config::LOG_LIMITS;
use crate::queue::{FormatStatus, LogPriority, LogQueue};
use crate::redact::{REDACTED, is_sensitive_field, redact_line_bounded};

/// The suffix replacing omitted formatter bytes. It includes the record's
/// terminal newline because truncation may discard the formatter's own.
const TRUNCATION_MARKER: &str = " [truncated]\n";

/// A cloneable factory that hands the fmt layer per-event writers feeding
/// the queue.
///
/// Priority comes only from the event's tracing metadata; the formatted
/// text passes through the privacy redaction before it can reach the
/// queue. Use one clone as the file layer's field formatter so classified
/// values are replaced without invoking their formatting implementation.
/// Obtained from
/// [`LogRuntime::writer`](crate::LogRuntime::writer).
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

impl<'writer> FormatFields<'writer> for LogWriter {
    fn format_fields<R>(&self, writer: Writer<'writer>, fields: R) -> fmt::Result
    where
        R: RecordFields,
    {
        let mut visitor = RedactingVisitor {
            inner: DefaultVisitor::new(writer, true),
        };
        fields.record(&mut visitor);
        visitor.inner.finish()
    }
}

struct RedactingVisitor<'writer> {
    inner: DefaultVisitor<'writer>,
}

impl RedactingVisitor<'_> {
    fn redact(&mut self, field: &Field) -> bool {
        if is_sensitive_field(field.name()) {
            self.inner.record_str(field, REDACTED);
            true
        } else {
            false
        }
    }
}

impl Visit for RedactingVisitor<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if !self.redact(field) {
            self.inner.record_f64(field, value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if !self.redact(field) {
            self.inner.record_i64(field, value);
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if !self.redact(field) {
            self.inner.record_u64(field, value);
        }
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        if !self.redact(field) {
            self.inner.record_i128(field, value);
        }
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        if !self.redact(field) {
            self.inner.record_u128(field, value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if !self.redact(field) {
            self.inner.record_bool(field, value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if !self.redact(field) {
            self.inner.record_str(field, value);
        }
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        if !self.redact(field) {
            self.inner.record_bytes(field, value);
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        if !self.redact(field) {
            self.inner.record_error(field, value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !self.redact(field) {
            self.inner.record_debug(field, value);
        }
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
    buffer: BoundedBytes,
    truncated: bool,
    utf8: Utf8Validator,
}

impl LogEventWriter {
    fn new(queue: Arc<LogQueue>, priority: LogPriority) -> Self {
        Self {
            queue,
            priority,
            buffer: BoundedBytes::new(LOG_LIMITS.max_formatted_record_bytes),
            truncated: false,
            utf8: Utf8Validator::default(),
        }
    }
}

impl io::Write for LogEventWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.utf8.push(buffer);
        let retained = self.buffer.extend_from_slice(buffer);
        self.truncated |= retained < buffer.len();
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
        if !self.utf8.is_complete() {
            self.queue.reject_formatted();
            return;
        }
        let Some(line) = finish_formatter_text(&mut self.buffer, self.truncated) else {
            self.queue.reject_formatted();
            return;
        };
        // The privacy chokepoint: every record crosses here, so the
        // well-shaped secrets are masked before they can reach the queue.
        let Some((line, truncated)) =
            redact_line_bounded(line, LOG_LIMITS.max_formatted_record_bytes)
                .finish(TRUNCATION_MARKER, self.truncated)
        else {
            self.queue.reject_formatted();
            return;
        };
        let status = if truncated {
            FormatStatus::Truncated
        } else {
            FormatStatus::Complete
        };
        self.queue.enqueue_formatted(self.priority, line, status);
    }
}

/// Formatter storage whose allocation is fixed at construction.
#[derive(Debug)]
struct BoundedBytes {
    storage: Box<[u8]>,
    len: usize,
}

impl BoundedBytes {
    fn new(capacity: usize) -> Self {
        #[cfg(test)]
        crate::allocation_tracking::record(capacity);
        Self {
            storage: vec![0; capacity].into_boxed_slice(),
            len: 0,
        }
    }

    fn capacity(&self) -> usize {
        self.storage.len()
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn as_slice(&self) -> &[u8] {
        &self.storage[..self.len]
    }

    fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }

    fn extend_from_slice(&mut self, bytes: &[u8]) -> usize {
        let retained = bytes.len().min(self.capacity().saturating_sub(self.len));
        self.storage[self.len..self.len + retained].copy_from_slice(&bytes[..retained]);
        self.len += retained;
        retained
    }
}

/// Allocation-free incremental validation covering retained and discarded
/// formatter bytes across arbitrary `Write` boundaries.
#[derive(Debug, Default)]
struct Utf8Validator {
    tail: [u8; 3],
    tail_len: usize,
    invalid: bool,
}

impl Utf8Validator {
    fn push(&mut self, mut bytes: &[u8]) {
        if self.invalid {
            return;
        }
        if self.tail_len != 0 {
            let mut combined = [0; 4];
            combined[..self.tail_len].copy_from_slice(&self.tail[..self.tail_len]);
            let taken = bytes.len().min(4 - self.tail_len);
            combined[self.tail_len..self.tail_len + taken].copy_from_slice(&bytes[..taken]);
            let combined_len = self.tail_len + taken;
            match std::str::from_utf8(&combined[..combined_len]) {
                Ok(_) => self.tail_len = 0,
                Err(error) if error.error_len().is_some() => {
                    self.invalid = true;
                    return;
                }
                Err(error) => {
                    let tail = &combined[error.valid_up_to()..combined_len];
                    self.tail[..tail.len()].copy_from_slice(tail);
                    self.tail_len = tail.len();
                    return;
                }
            }
            bytes = &bytes[taken..];
        }
        if let Err(error) = std::str::from_utf8(bytes) {
            if error.error_len().is_some() {
                self.invalid = true;
            } else {
                let tail = &bytes[error.valid_up_to()..];
                self.tail[..tail.len()].copy_from_slice(tail);
                self.tail_len = tail.len();
            }
        }
    }

    fn is_complete(&self) -> bool {
        !self.invalid && self.tail_len == 0
    }
}

/// Borrows valid text from the bounded byte buffer without repairing invalid
/// formatter bytes. A retained prefix ending inside a code point rewinds to
/// its valid boundary; validation of the original bytes happened on write.
fn finish_formatter_text(buffer: &mut BoundedBytes, truncated: bool) -> Option<&str> {
    if truncated {
        let payload_limit = LOG_LIMITS
            .max_formatted_record_bytes
            .saturating_sub(TRUNCATION_MARKER.len());
        buffer.truncate(payload_limit);
        if let Err(error) = std::str::from_utf8(buffer.as_slice()) {
            if error.error_len().is_some() {
                return None;
            }
            buffer.truncate(error.valid_up_to());
        }
    }
    std::str::from_utf8(buffer.as_slice()).ok()
}

#[cfg(test)]
#[path = "writer-tests.rs"]
mod tests;
