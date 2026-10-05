//! Tests for the fixed record budget, truncation, and invalid UTF-8 rejection.

use super::*;

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
        "the valid prefix ends with the truncation marker"
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
