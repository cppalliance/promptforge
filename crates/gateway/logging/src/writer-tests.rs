//! Tests for event buffering, metadata priority, and field redaction.

use super::*;
use crate::config::LOG_LIMITS;
use std::fmt;
use std::io::Write as _;

#[path = "writer-tests-bounds.rs"]
mod bounds;

struct ProtectedValue<'a> {
    value: &'a str,
    formatted: &'a crate::fault_injection::InvocationProbe,
}

impl fmt::Debug for ProtectedValue<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.formatted.record();
        formatter.write_str(self.value)
    }
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
    let priorities: Vec<LogPriority> = batch.records.iter().map(|record| record.priority).collect();
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
        "the record contains the formatted event: {}",
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
    let formatted = crate::fault_injection::InvocationProbe::default();
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
        !formatted.was_recorded(),
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
    let formatted =
        std::array::from_fn::<_, 3, _>(|_| crate::fault_injection::InvocationProbe::default());
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
            !formatted[index].was_recorded(),
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
