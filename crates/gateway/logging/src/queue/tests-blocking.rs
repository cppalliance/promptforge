//! Tests for producer blocking, deadlines, and wakeups.

use super::*;

#[test]
fn a_producer_with_no_eligible_record_blocks_until_space_opens() {
    let queue = Arc::new(LogQueue::new_for_test_with_wait(
        CAPACITY,
        LOG_LIMITS.max_queued_bytes,
        Duration::from_secs(1),
    ));
    for index in 0..CAPACITY {
        queue.enqueue(LogPriority::Warn, line(&format!("warn-{index}")));
    }
    // A Debug record may evict only Debug, and the queue holds none:
    // the producer blocks instead of dropping or inverting priority.
    let producer_queue = Arc::clone(&queue);
    let (done_tx, done_rx) = mpsc::channel();
    let producer = std::thread::spawn(move || {
        producer_queue.enqueue(LogPriority::Debug, line("debug-blocked"));
        done_tx.send(()).expect("report enqueue");
    });
    assert!(
        done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a full queue of Warn records blocks a Debug producer"
    );

    let batch = queue.take_batch();
    assert_eq!(batch.records.len(), BATCH, "the drain frees space");
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the blocked producer wakes once space opens");
    producer.join().expect("the producer joins");
    queue.close();

    let records = drain_all(&queue);
    assert!(
        records
            .iter()
            .any(|record| &*record.line == "debug-blocked"),
        "the blocked record lands once space opens"
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.line.starts_with("warn-"))
            .count(),
        CAPACITY - BATCH,
        "no Warn record was evicted to make room"
    );
}

#[test]
fn a_protected_producer_times_out_into_preallocated_loss_accounting() {
    let wait = Duration::from_millis(30);
    let queue = LogQueue::new_for_test_with_wait(2, 32, wait);
    queue.enqueue(LogPriority::Warn, line("warn-one"));
    queue.enqueue(LogPriority::Error, line("error-two"));

    let started = std::time::Instant::now();
    queue.enqueue(LogPriority::Error, line("error-timeout"));
    let elapsed = started.elapsed();
    assert!(
        elapsed >= wait,
        "the protected producer waits for its configured budget: {elapsed:?}"
    );
    assert!(
        elapsed < wait + Duration::from_millis(75),
        "the protected producer stays near its configured upper bound: {elapsed:?}"
    );

    queue.close();
    let batch = queue.take_batch();
    assert_eq!(
        batch
            .records
            .iter()
            .map(|record| record.line.as_ref())
            .collect::<Vec<_>>(),
        ["warn-one", "error-two"],
        "the timed-out record was never admitted"
    );
    assert_eq!(
        batch.summary.as_deref(),
        Some(
            "log pressure affected 1 record(s): dropped=1, debug=0, trace=0, info=0, truncated=0, rejected=1\n"
        ),
        "timeout loss uses the existing pressure summary storage"
    );
}

#[test]
fn producer_deadline_includes_waiting_to_acquire_the_mutex() {
    let wait = Duration::from_millis(40);
    let queue = Arc::new(LogQueue::new_for_test_with_wait(2, 32, wait));
    queue.enqueue(LogPriority::Warn, line("warn-one"));
    queue.enqueue(LogPriority::Error, line("error-two"));

    let lock_queue = Arc::clone(&queue);
    let (locked_tx, locked_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::channel();
    let lock_holder = std::thread::spawn(move || {
        lock_queue.hold_lock_for_test(&locked_tx, &release_rx);
    });
    locked_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the queue mutex is held");

    let producer_queue = Arc::clone(&queue);
    let (done_tx, done_rx) = mpsc::sync_channel(0);
    let producer = std::thread::spawn(move || {
        producer_queue.enqueue(LogPriority::Error, line("mutex-timeout"));
        done_tx.send(()).expect("report bounded producer");
    });
    let bounded = done_rx.recv_timeout(wait + Duration::from_millis(75));
    release_tx.send(()).expect("release queue mutex");
    lock_holder.join().expect("the lock holder joins");
    producer.join().expect("the producer joins");
    assert!(
        bounded.is_ok(),
        "mutex acquisition is part of the producer's {wait:?} budget"
    );

    queue.close();
    let batch = queue.take_batch();
    assert_eq!(
        batch.summary.as_deref(),
        Some(
            "log pressure affected 1 record(s): dropped=1, debug=0, trace=0, info=0, truncated=0, rejected=1\n"
        ),
        "a mutex-budget loss remains explicitly observable"
    );
}

#[test]
fn a_byte_blocked_producer_wakes_after_drain() {
    let queue = Arc::new(LogQueue::new_for_test_with_wait(
        4,
        4,
        Duration::from_secs(1),
    ));
    queue.enqueue(LogPriority::Warn, line("wwww"));

    let drain_queue = Arc::clone(&queue);
    let (drain_prepared_tx, drain_prepared_rx) = mpsc::channel();
    let (drain_release_tx, drain_release_rx) = mpsc::channel();
    let (drain_done_tx, drain_done_rx) = mpsc::channel();
    let drain_waiter = std::thread::spawn(move || {
        drain_queue.enqueue_after(
            LogPriority::Debug,
            line("d"),
            FormatStatus::Complete,
            || {
                drain_prepared_tx
                    .send(())
                    .expect("report prepared producer");
                drain_release_rx.recv().expect("release prepared producer");
            },
        );
        drain_done_tx.send(()).expect("report drain wakeup");
    });
    drain_prepared_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the byte-blocked producer reaches admission");
    drain_release_tx
        .send(())
        .expect("release the byte-blocked producer");
    assert!(
        drain_done_rx
            .recv_timeout(Duration::from_millis(200))
            .is_err(),
        "free record slots do not bypass the aggregate byte bound"
    );

    let freed = queue.take_batch();
    assert_eq!(
        freed
            .records
            .iter()
            .map(|record| record.line.as_ref())
            .collect::<Vec<_>>(),
        ["wwww"]
    );
    drain_done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("draining bytes wakes the producer");
    drain_waiter.join().expect("the drain waiter joins");
    let admitted = queue.take_batch();
    assert_eq!(admitted.records[0].line.as_ref(), "d");
}

#[test]
fn close_preaccounts_a_byte_blocked_protected_record() {
    let queue = Arc::new(LogQueue::new_for_test_with_wait(
        4,
        4,
        Duration::from_secs(1),
    ));
    queue.enqueue(LogPriority::Warn, line("wwww"));
    let close_queue = Arc::clone(&queue);
    let (close_prepared_tx, close_prepared_rx) = mpsc::channel();
    let (close_release_tx, close_release_rx) = mpsc::channel();
    let (close_done_tx, close_done_rx) = mpsc::channel();
    let close_waiter = std::thread::spawn(move || {
        close_queue.enqueue_after(
            LogPriority::Error,
            line("z"),
            FormatStatus::Complete,
            || {
                close_prepared_tx
                    .send(())
                    .expect("report prepared producer");
                close_release_rx.recv().expect("release prepared producer");
            },
        );
        close_done_tx.send(()).expect("report close wakeup");
    });
    close_prepared_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the closing producer reaches admission");
    close_release_tx
        .send(())
        .expect("release the closing producer");
    assert!(
        close_done_rx
            .recv_timeout(Duration::from_millis(200))
            .is_err(),
        "the producer blocks on bytes before close"
    );

    queue.close();
    close_done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("close wakes the byte-blocked producer");
    close_waiter.join().expect("the close waiter joins");
    let remaining = queue.take_batch();
    assert_eq!(
        remaining
            .records
            .iter()
            .map(|record| record.line.as_ref())
            .collect::<Vec<_>>(),
        ["wwww"],
        "close wakes the producer without admitting its record"
    );
    assert_eq!(
        remaining.summary.as_deref(),
        Some(
            "log pressure affected 1 record(s): dropped=1, debug=0, trace=0, info=0, truncated=0, rejected=1\n"
        ),
        "close pre-accounts the protected record before waking its producer"
    );
    assert!(remaining.done);
}
