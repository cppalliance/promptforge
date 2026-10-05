//! Tests for sequence assignment, abandonment accounting, and pressure episodes.

use super::*;

#[test]
fn sequence_follows_admission_when_a_prepared_producer_is_paused() {
    let queue = Arc::new(LogQueue::new_for_test(8, 128));
    let (pause, control) = crate::fault_injection::release_point();
    let paused_queue = Arc::clone(&queue);
    let paused = std::thread::spawn(move || {
        paused_queue.enqueue_after(
            LogPriority::Info,
            line("prepared-first"),
            FormatStatus::Complete,
            || pause.wait(),
        );
    });
    control
        .wait_until_reached(Duration::from_secs(5))
        .expect("the first producer pauses before admission");

    queue.enqueue(LogPriority::Warn, line("admitted-first"));
    control.release();
    paused.join().expect("the paused producer joins");
    queue.close();

    let records = drain_all(&queue);
    assert_eq!(
        records
            .iter()
            .map(|record| (record.sequence, record.line.as_ref()))
            .collect::<Vec<_>>(),
        [(0, "admitted-first"), (1, "prepared-first")],
        "sequence is assigned atomically with successful admission"
    );
}

#[test]
fn abandonment_counts_admission_boundary_record_exactly_once() {
    let queue = Arc::new(LogQueue::new_for_test(8, 128));
    let producer_queue = Arc::clone(&queue);
    let (pause, control) = crate::fault_injection::release_point();
    let producer = std::thread::spawn(move || {
        producer_queue.enqueue_around(
            LogPriority::Warn,
            line("admitted-before-timeout"),
            FormatStatus::Complete,
            || {},
            || pause.wait(),
        );
    });
    control
        .wait_until_reached(Duration::from_secs(5))
        .expect("the producer pauses after admission");

    let loss = queue.abandon();
    assert_eq!(
        loss,
        ShutdownLoss {
            abandoned_records: 1,
            abandoned_summaries: 0,
            unreported_pressure_records: 0,
        },
        "one admitting producer is one undelivered record, not an active-plus-admitted double count"
    );
    control.release();
    producer.join().expect("the producer joins");
    assert_eq!(
        queue.shutdown_loss_for_test(),
        loss,
        "the persisted shutdown accounting keeps the same exact snapshot"
    );
}

#[test]
fn variable_size_pressure_obeys_record_and_byte_peaks() {
    let count_queue = LogQueue::new_for_test(3, 100);
    for text in ["aaaa", "bb", "ccc"] {
        count_queue.enqueue(LogPriority::Debug, line(text));
    }
    count_queue.enqueue(LogPriority::Error, line("e"));
    assert_eq!(
        count_queue.accounting_for_test(),
        (3, 6, 9),
        "record capacity evicts one old Debug while byte accounting stays exact"
    );

    let byte_queue = LogQueue::new_for_test(8, 10);
    for text in ["aaaa", "bb", "ccc"] {
        byte_queue.enqueue(LogPriority::Debug, line(text));
    }
    byte_queue.enqueue(LogPriority::Error, line("1234567"));
    assert_eq!(
        byte_queue.accounting_for_test(),
        (2, 10, 10),
        "variable-size eviction admits only after enough exact bytes are freed"
    );
    byte_queue.close();
    let records = drain_all(&byte_queue);
    assert_eq!(
        records
            .iter()
            .map(|record| record.line.as_ref())
            .collect::<Vec<_>>(),
        ["ccc", "1234567"],
        "the oldest eligible records are evicted until the byte bound fits"
    );
}

#[test]
fn pressure_summary_follows_the_admitted_tail_and_resets_for_a_second_episode() {
    let queue = LogQueue::new_for_test(600, 600);
    for _ in 0..600 {
        queue.enqueue(LogPriority::Debug, line("d"));
    }
    queue.enqueue_formatted(LogPriority::Error, line("e"), FormatStatus::Truncated);
    for _ in 0..4 {
        queue.enqueue(LogPriority::Error, line("e"));
    }

    let above_low_water = queue.take_batch();
    assert_eq!(above_low_water.records.len(), BATCH);
    assert!(
        above_low_water.summary.is_none(),
        "the episode remains open above both half-capacity thresholds"
    );
    let recovered = queue.take_batch();
    assert_eq!(recovered.records.len(), BATCH);
    assert!(
        recovered.summary.is_none(),
        "the summary waits behind records admitted before recovery"
    );
    assert_eq!(
        queue.accounting_for_test().0,
        600 - (BATCH * 2),
        "crossing low water leaves an admitted tail"
    );

    queue.enqueue(LogPriority::Info, line("admitted-after-recovery"));
    let admitted_tail = queue.take_batch();
    assert_eq!(
        admitted_tail.records.len(),
        600 - (BATCH * 2),
        "only records older than the pending summary drain"
    );
    assert!(
        admitted_tail
            .records
            .iter()
            .all(|record| record.line.as_ref() != "admitted-after-recovery"),
        "a later admission cannot move ahead of the pending summary"
    );
    assert_eq!(
        admitted_tail.summary.as_deref(),
        Some(
            "log pressure affected 6 record(s): dropped=5, debug=5, trace=0, info=0, truncated=1, rejected=0\n"
        ),
        "the first episode closes immediately after its admitted tail"
    );

    queue.enqueue(LogPriority::Error, line(&"x".repeat(601)));
    queue.close();
    let second_episode = queue.take_batch();
    assert_eq!(
        second_episode
            .records
            .iter()
            .map(|record| record.line.as_ref())
            .collect::<Vec<_>>(),
        ["admitted-after-recovery"],
        "the later admission follows the first summary"
    );
    assert_eq!(
        second_episode.summary.as_deref(),
        Some(
            "log pressure affected 1 record(s): dropped=1, debug=0, trace=0, info=0, truncated=0, rejected=1\n"
        ),
        "the oversized protected record starts a clean second episode"
    );
    assert!(second_episode.done);
}
