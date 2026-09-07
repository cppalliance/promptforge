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
///
/// # Examples
/// ```
/// # let dir = std::env::temp_dir().join(concat!("gateway-logging-doc-writer-", env!("CARGO_PKG_VERSION")));
/// let runtime = gateway_logging::LogRuntime::start(gateway_logging::LogConfig::new(&dir))?;
/// let writer = runtime.writer();
/// let _subscriber = tracing_subscriber::fmt()
///     .fmt_fields(writer.clone())
///     .with_writer(writer)
///     .finish();
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

/// Fixed-capacity formatter storage whose allocation cannot grow.
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
mod tests {
    use super::*;
    use crate::config::LOG_LIMITS;
    use std::fmt;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct ProtectedValue<'a> {
        value: &'a str,
        formatted: &'a AtomicBool,
    }

    impl fmt::Debug for ProtectedValue<'_> {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.formatted.store(true, Ordering::SeqCst);
            formatter.write_str(self.value)
        }
    }

    #[test]
    fn an_oversized_event_never_allocates_or_enqueues_above_the_record_limit() {
        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        let oversized = vec![b'x'; LOG_LIMITS.max_formatted_record_bytes + 1];
        {
            let mut event = MakeWriter::make_writer(&writer);
            for chunk in oversized.chunks(997) {
                event.write_all(chunk).expect("accept formatter chunk");
            }
            assert!(
                event.buffer.capacity() == LOG_LIMITS.max_formatted_record_bytes,
                "formatter storage has one fixed capacity equal to the record budget"
            );
        }
        queue.close();

        let batch = queue.take_batch();
        assert_eq!(batch.records.len(), 1, "the bounded prefix is retained");
        assert!(
            batch.records[0].line.len() <= LOG_LIMITS.max_formatted_record_bytes,
            "the queued record obeys the byte budget"
        );
        assert!(
            batch.records[0].line.ends_with(TRUNCATION_MARKER),
            "the retained prefix explicitly marks omitted text"
        );
        assert!(
            batch
                .summary
                .as_deref()
                .is_some_and(|summary| summary.contains("truncated=1")),
            "truncation enters the queue's observable loss episode"
        );
    }

    #[test]
    fn expanding_redaction_never_requests_an_allocation_above_the_record_limit() {
        const UNIT: &str = "api_key=a prompt=b path=c payload=d ";

        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        let expansion_heavy = UNIT.repeat(LOG_LIMITS.max_formatted_record_bytes / UNIT.len());

        let allocations = crate::allocation_tracking::AllocationTracker::start();
        {
            let mut event = MakeWriter::make_writer(&writer);
            event
                .write_all(expansion_heavy.as_bytes())
                .expect("accept expansion-heavy formatter bytes");
        }
        let largest_request = allocations.finish();
        queue.close();

        assert!(
            largest_request <= LOG_LIMITS.max_formatted_record_bytes,
            "largest per-record allocation request {largest_request} exceeds {}",
            LOG_LIMITS.max_formatted_record_bytes
        );
        let batch = queue.take_batch();
        assert_eq!(batch.records.len(), 1);
        assert!(
            batch.records[0].line.ends_with(TRUNCATION_MARKER),
            "bounded expansion is explicitly marked"
        );
        for cleartext in ["api_key=a", "prompt=b", "path=c", "payload=d"] {
            assert!(
                !batch.records[0].line.contains(cleartext),
                "retained assignments are redacted before enqueue"
            );
        }
    }

    #[test]
    fn truncation_rewinds_to_a_valid_multibyte_boundary() {
        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        let payload_budget = LOG_LIMITS.max_formatted_record_bytes - TRUNCATION_MARKER.len();
        let mut oversized = vec![b'x'; payload_budget - 1];
        oversized.extend_from_slice("😀".as_bytes());
        oversized.extend(std::iter::repeat_n(b'y', TRUNCATION_MARKER.len()));
        {
            let mut event = MakeWriter::make_writer(&writer);
            event
                .write_all(&oversized)
                .expect("accept the formatted event");
        }
        queue.close();

        let batch = queue.take_batch();
        let line = &batch.records[0].line;
        assert!(
            line.len() <= LOG_LIMITS.max_formatted_record_bytes,
            "the multibyte record obeys the byte budget"
        );
        assert!(
            line.ends_with(TRUNCATION_MARKER),
            "the valid prefix carries the truncation marker"
        );
        assert!(
            !line.contains('\u{fffd}'),
            "truncation never replaces a split code point"
        );
    }

    #[test]
    fn invalid_formatter_bytes_are_rejected_with_observable_loss() {
        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        {
            let mut event = MakeWriter::make_writer(&writer);
            event
                .write_all(&[b'v', 0xff, b'\n'])
                .expect("accept formatter bytes");
        }
        queue.close();

        let batch = queue.take_batch();
        assert!(
            batch.records.is_empty(),
            "invalid UTF-8 is rejected instead of repaired"
        );
        assert!(
            batch
                .summary
                .as_deref()
                .is_some_and(|summary| summary.contains("rejected=1")),
            "rejection enters the queue's observable loss episode"
        );
    }

    #[test]
    fn invalid_utf8_after_the_retained_prefix_rejects_the_whole_record() {
        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        let retained = vec![b'v'; LOG_LIMITS.max_formatted_record_bytes];
        {
            let mut event = MakeWriter::make_writer(&writer);
            event
                .write_all(&retained)
                .expect("accept the retained valid prefix");
            event
                .write_all(&[b'x', 0xff])
                .expect("accept discarded formatter bytes");
        }
        queue.close();

        let batch = queue.take_batch();
        assert!(
            batch.records.is_empty(),
            "invalid UTF-8 hidden beyond the retained prefix rejects the record"
        );
        assert_eq!(
            batch.summary.as_deref(),
            Some(
                "log pressure affected 1 record(s): dropped=1, debug=0, trace=0, info=0, truncated=0, rejected=1\n"
            )
        );
    }

    #[test]
    fn truncation_rejection_and_eviction_share_exactly_one_loss_summary() {
        use crate::queue::CAPACITY;

        let queue = Arc::new(LogQueue::new());
        for index in 0..CAPACITY {
            queue.enqueue(
                LogPriority::Debug,
                format!("debug-{index}\n").into_boxed_str(),
            );
        }
        let writer = LogWriter::new(Arc::clone(&queue));
        {
            let mut truncated = MakeWriter::make_writer(&writer);
            truncated
                .write_all(&vec![b't'; LOG_LIMITS.max_formatted_record_bytes + 1])
                .expect("accept oversized formatter bytes");
        }
        {
            let mut rejected = MakeWriter::make_writer(&writer);
            rejected
                .write_all(&[b'i', 0xff])
                .expect("accept invalid formatter bytes");
        }
        queue.close();

        let mut summaries = Vec::new();
        let mut saw_truncated_record = false;
        loop {
            let batch = queue.take_batch();
            saw_truncated_record |= batch
                .records
                .iter()
                .any(|record| record.line.ends_with(TRUNCATION_MARKER));
            if let Some(summary) = batch.summary {
                summaries.push(summary);
            }
            if batch.done {
                break;
            }
        }

        assert!(saw_truncated_record, "the truncated record was admitted");
        assert_eq!(
            summaries.iter().map(Box::as_ref).collect::<Vec<_>>(),
            [
                "log pressure affected 3 record(s): dropped=2, debug=1, trace=0, info=0, truncated=1, rejected=1\n"
            ],
            "eviction, truncation, and rejection close as one exactly-accounted episode"
        );
    }

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

    #[test]
    fn classified_fields_are_redacted_without_formatting_their_values() {
        const SECRETS: [&str; 7] = [
            "basic-secret",
            "cookie-secret",
            "url-secret",
            "prompt-secret",
            "path-secret",
            "payload-secret",
            "typed-secret",
        ];

        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        let formatted = AtomicBool::new(false);
        let protected = ProtectedValue {
            value: SECRETS[6],
            formatted: &formatted,
        };
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .fmt_fields(writer.clone())
            .with_writer(writer)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                authorization = "Basic basic-secret",
                request_cookie = "session=cookie-secret",
                upstream_url = "https://user:url-secret@example.test/v1",
                system_prompt = "prompt-secret",
                config_path = "C:\\private\\path-secret\\model.gguf",
                request_payload = "{\"token\":\"payload-secret\"}",
                secret = ?protected,
                ordinary = 7_u64,
                "classified field matrix"
            );
        });
        queue.close();

        let batch = queue.take_batch();
        assert_eq!(batch.records.len(), 1);
        let line = &batch.records[0].line;
        for secret in SECRETS {
            assert!(
                !line.contains(secret),
                "classified value reached the queue: {line}"
            );
        }
        assert!(
            !formatted.load(Ordering::SeqCst),
            "a secret-typed debug value was formatted before redaction"
        );
        assert!(
            line.contains("ordinary=7"),
            "unclassified structured fields keep their formatting: {line}"
        );
        assert!(
            line.contains("classified field matrix"),
            "unstructured wording remains intact: {line}"
        );
    }

    #[test]
    fn composite_sensitive_aliases_never_invoke_adversarial_formatters() {
        const SECRETS: [&str; 3] = [
            "authorization-alias-secret",
            "cookie-alias-secret",
            "token-alias-secret",
        ];
        let formatted = std::array::from_fn::<_, 3, _>(|_| AtomicBool::new(false));
        let authorization = ProtectedValue {
            value: SECRETS[0],
            formatted: &formatted[0],
        };
        let cookie = ProtectedValue {
            value: SECRETS[1],
            formatted: &formatted[1],
        };
        let token = ProtectedValue {
            value: SECRETS[2],
            formatted: &formatted[2],
        };
        let queue = Arc::new(LogQueue::new());
        let writer = LogWriter::new(Arc::clone(&queue));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .fmt_fields(writer.clone())
            .with_writer(writer)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                authorization_header = ?authorization,
                cookie_header = ?cookie,
                token_value = ?token,
                "composite alias matrix"
            );
        });
        queue.close();

        let batch = queue.take_batch();
        assert_eq!(batch.records.len(), 1);
        for (index, secret) in SECRETS.iter().enumerate() {
            assert!(
                !formatted[index].load(Ordering::SeqCst),
                "the formatter for alias {index} was invoked"
            );
            assert!(
                !batch.records[0].line.contains(secret),
                "a composite alias reached the queue: {}",
                batch.records[0].line
            );
        }
    }

    #[test]
    fn unclassified_fields_keep_default_formatting_byte_for_byte() {
        let default_queue = Arc::new(LogQueue::new());
        let default_writer = LogWriter::new(Arc::clone(&default_queue));
        let default_subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(default_writer)
            .finish();
        tracing::subscriber::with_default(default_subscriber, || {
            tracing::info!(
                target: "format-regression",
                count = 7_u64,
                label = "ordinary",
                "unchanged wording"
            );
        });
        default_queue.close();
        let default_line = default_queue.take_batch().records.remove(0).line;

        let redacting_queue = Arc::new(LogQueue::new());
        let redacting_writer = LogWriter::new(Arc::clone(&redacting_queue));
        let redacting_subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .fmt_fields(redacting_writer.clone())
            .with_writer(redacting_writer)
            .finish();
        tracing::subscriber::with_default(redacting_subscriber, || {
            tracing::info!(
                target: "format-regression",
                count = 7_u64,
                label = "ordinary",
                "unchanged wording"
            );
        });
        redacting_queue.close();
        let redacting_line = redacting_queue.take_batch().records.remove(0).line;

        assert_eq!(
            redacting_line, default_line,
            "the redacting visitor delegates ordinary values to DefaultVisitor"
        );
    }
}
