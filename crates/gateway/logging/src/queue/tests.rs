//! Tests for lane eviction, batch order, pressure summaries, and saturation.

use super::*;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

#[path = "tests-accounting.rs"]
mod accounting;
#[path = "tests-blocking.rs"]
mod blocking;

fn line(text: &str) -> Box<str> {
    Box::from(text)
}

fn drain_all(queue: &LogQueue) -> Vec<LogRecord> {
    let mut all = Vec::new();
    loop {
        let batch = queue.take_batch();
        all.extend(batch.records);
        if batch.done {
            return all;
        }
    }
}

#[test]
fn a_full_queue_evicts_oldest_debug_then_trace_then_info() {
    let queue = LogQueue::new();
    queue.enqueue(LogPriority::Debug, line("debug-oldest"));
    queue.enqueue(LogPriority::Trace, line("trace-oldest"));
    queue.enqueue(LogPriority::Info, line("info-oldest"));
    for index in 0..(CAPACITY - 3) {
        queue.enqueue(LogPriority::Warn, line(&format!("warn-{index}")));
    }
    // Each Error evicts the oldest record of the least protected
    // eligible lane: Debug first, then Trace, then Info.
    queue.enqueue(LogPriority::Error, line("error-1"));
    queue.enqueue(LogPriority::Error, line("error-2"));
    queue.enqueue(LogPriority::Error, line("error-3"));
    queue.close();

    let records = drain_all(&queue);
    let lines: Vec<&str> = records.iter().map(|record| &*record.line).collect();
    assert!(
        !lines.contains(&"debug-oldest"),
        "the oldest Debug is the first eviction victim"
    );
    assert!(
        !lines.contains(&"trace-oldest"),
        "with the Debug lane empty, the oldest Trace goes next"
    );
    assert!(
        !lines.contains(&"info-oldest"),
        "with Debug and Trace empty, the oldest Info goes last"
    );
    for wanted in ["error-1", "error-2", "error-3"] {
        assert!(lines.contains(&wanted), "the evicting record is retained");
    }
    assert_eq!(
        records.len(),
        CAPACITY,
        "eviction keeps the queue at capacity"
    );
}

#[test]
fn eviction_never_displaces_a_more_important_record() {
    let queue = LogQueue::new();
    queue.enqueue(LogPriority::Trace, line("trace-kept"));
    queue.enqueue(LogPriority::Info, line("info-kept"));
    for index in 0..(CAPACITY - 2) {
        queue.enqueue(LogPriority::Warn, line(&format!("warn-{index}")));
    }
    // A Trace may evict a Trace but never an Info or a Warn.
    queue.enqueue(LogPriority::Trace, line("trace-new"));
    queue.close();

    let records = drain_all(&queue);
    let lines: Vec<&str> = records.iter().map(|record| &*record.line).collect();
    assert!(
        !lines.contains(&"trace-kept"),
        "Trace evicts the oldest Trace"
    );
    assert!(
        lines.contains(&"info-kept"),
        "Trace never evicts Info: no priority inversion"
    );
    assert!(
        lines.contains(&"trace-new"),
        "the evicting record is retained"
    );
    assert!(
        (0..(CAPACITY - 2)).all(|index| lines.contains(&format!("warn-{index}").as_str())),
        "Warn records are never eviction victims"
    );
}

#[test]
fn a_batch_drains_256_records_in_global_sequence_order() {
    let queue = LogQueue::new();
    let priorities = [
        LogPriority::Error,
        LogPriority::Debug,
        LogPriority::Info,
        LogPriority::Warn,
        LogPriority::Trace,
    ];
    for index in 0..(BATCH * 2) {
        queue.enqueue(
            priorities[index % priorities.len()],
            line(&format!("record-{index}")),
        );
    }

    let batch = queue.take_batch();
    assert_eq!(
        batch.records.len(),
        BATCH,
        "one drain swaps a bounded batch"
    );
    assert!(!batch.done, "an open queue with records left is not done");
    for (position, record) in batch.records.iter().enumerate() {
        assert_eq!(
            record.sequence, position as u64,
            "lane heads are merged by smallest global sequence"
        );
        assert_eq!(
            &*record.line,
            format!("record-{position}"),
            "chronological order is retained across lanes"
        );
    }
    queue.close();
    let rest = drain_all(&queue);
    assert_eq!(rest.len(), BATCH, "the remainder drains after close");
}

#[test]
fn pressure_emits_one_summary_after_the_queue_empties() {
    let queue = LogQueue::new();
    for index in 0..CAPACITY {
        queue.enqueue(LogPriority::Debug, line(&format!("debug-{index}")));
    }
    for index in 0..10 {
        queue.enqueue(LogPriority::Error, line(&format!("error-{index}")));
    }
    queue.close();

    let mut summaries = Vec::new();
    let mut total_records = 0;
    loop {
        let batch = queue.take_batch();
        total_records += batch.records.len();
        if let Some(summary) = batch.summary {
            summaries.push(summary);
        }
        if batch.done {
            break;
        }
    }
    assert_eq!(total_records, CAPACITY, "eviction keeps the queue full");
    assert_eq!(
        summaries.len(),
        1,
        "one synthetic summary per pressure episode: {summaries:?}"
    );
    assert!(
        summaries[0].contains("10 record(s)") && summaries[0].contains("debug=10"),
        "the summary counts evictions by level: {}",
        summaries[0]
    );
}

#[test]
fn a_queue_without_evictions_emits_no_summary() {
    let queue = LogQueue::new();
    queue.enqueue(LogPriority::Info, line("only"));
    queue.close();
    let batch = queue.take_batch();
    assert!(batch.summary.is_none(), "no pressure, no summary");
    assert!(batch.done);
}

#[test]
fn a_closed_queue_drops_new_records() {
    let queue = LogQueue::new();
    queue.enqueue(LogPriority::Info, line("before-close"));
    queue.close();
    queue.enqueue(LogPriority::Error, line("after-close"));
    let records = drain_all(&queue);
    assert_eq!(records.len(), 1, "admission is closed");
    assert_eq!(&*records[0].line, "before-close");
}

#[test]
fn saturation_under_load_never_evicts_or_duplicates_warn_or_error() {
    const PRODUCERS: u64 = 4;
    const PER_PRODUCER: u64 = 10000;
    let queue = Arc::new(LogQueue::new_for_test_with_wait(
        CAPACITY,
        LOG_LIMITS.max_queued_bytes,
        Duration::from_secs(1),
    ));
    let (drained_tx, drained_rx) = mpsc::channel();
    let worker_queue = Arc::clone(&queue);
    let worker = std::thread::spawn(move || {
        loop {
            let batch = worker_queue.take_batch();
            for record in batch.records {
                drained_tx
                    .send(record.line)
                    .expect("report the drained line");
            }
            if batch.done {
                break;
            }
            // A slow sink: the producers outpace the drain, so the queue
            // fills and the eviction and blocking paths fire. The worker
            // never stops draining, so a blocked producer always wakes.
            std::thread::sleep(Duration::from_millis(2));
        }
    });

    // Four producers push five times the capacity across every lane.
    let producers: Vec<_> = (0..PRODUCERS)
        .map(|id| {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || {
                for index in 0..PER_PRODUCER {
                    let priority = match index % 5 {
                        0 => LogPriority::Debug,
                        1 => LogPriority::Trace,
                        2 => LogPriority::Info,
                        3 => LogPriority::Warn,
                        _ => LogPriority::Error,
                    };
                    queue.enqueue(priority, line(&format!("p{id}-{priority:?}-{index}")));
                }
            })
        })
        .collect();
    for producer in producers {
        producer.join().expect("the producer joins");
    }
    queue.close();
    worker.join().expect("the worker joins");

    let drained: Vec<String> = drained_rx.iter().map(|line| line.to_string()).collect();
    let unique: std::collections::HashSet<&String> = drained.iter().collect();
    assert_eq!(
        drained.len(),
        unique.len(),
        "no record is written twice under saturation"
    );
    assert!(
        (drained.len() as u64) < PRODUCERS * PER_PRODUCER,
        "saturation really evicted: {} of {} records retained",
        drained.len(),
        PRODUCERS * PER_PRODUCER
    );
    for id in 0..PRODUCERS {
        for index in (3..PER_PRODUCER).step_by(5) {
            let warn = format!("p{id}-Warn-{index}");
            let error = format!("p{id}-Error-{}", index + 1);
            assert!(
                unique.contains(&warn),
                "Warn survives saturation: missing {warn}"
            );
            assert!(
                unique.contains(&error),
                "Error survives saturation: missing {error}"
            );
        }
    }
}
