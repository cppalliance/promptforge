//! Interim task ownership, retry, and canceled-join capacity.

use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use futures_util::FutureExt as _;

use super::{
    BLOCKING_TASK_TEST, BlockingPoll, CANCEL_JOIN_CAPACITY, encoded, session, source_message,
    wait_until_started,
};

#[tokio::test]
async fn canceling_finish_keeps_current_task_owned_for_retry() {
    let mut session = session();
    session
        .append_base64(&encoded(&[0, 0]))
        .expect("input appends");
    let (send, receive) = tokio::sync::oneshot::channel();
    session
        .spawn_interim(async move { receive.await.expect("completion is sent") })
        .expect("interim starts");

    assert!(
        session.finish_interim().now_or_never().is_none(),
        "first poll remains pending"
    );
    send.send("accepted".to_owned())
        .expect("receiver remains owned");
    let event = session
        .finish_interim()
        .await
        .expect("retry joins")
        .expect("current result is accepted");
    assert_eq!(event["delta"], "accepted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[expect(
    clippy::await_holding_lock,
    reason = "the process-wide test lock serializes deliberately blocked runtime workers"
)]
async fn canceling_join_keeps_capacity_owned_until_retry_completes() {
    let _serial = BLOCKING_TASK_TEST
        .lock()
        .expect("test lock is not poisoned");
    let mut session = session();
    session
        .append_base64(&encoded(&[0, 0]))
        .expect("input appends");
    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Arc::new(AtomicBool::new(false));
    session
        .spawn_interim(BlockingPoll {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        })
        .expect("interim starts");
    wait_until_started(&started);
    session.clear().expect("interim retires");

    assert!(
        session.join_canceled().now_or_never().is_none(),
        "first join poll remains pending"
    );
    assert_eq!(session.canceled_join_count(), 1);
    release.store(true, Ordering::Release);
    session.join_canceled().await.expect("retry joins task");
    assert_eq!(session.canceled_join_count(), 0);
}

#[tokio::test]
async fn canceled_join_capacity_is_exact_and_recoverable() {
    let mut session = session();
    for _ in 0..CANCEL_JOIN_CAPACITY {
        session
            .append_base64(&encoded(&[0, 0]))
            .expect("input appends");
        session
            .spawn_interim(pending())
            .expect("interim starts within capacity");
        session.clear().expect("task is retained");
    }
    assert_eq!(session.canceled_join_count(), CANCEL_JOIN_CAPACITY);

    session
        .append_base64(&encoded(&[0, 0]))
        .expect("capacity-plus-one input appends");
    session
        .spawn_interim(pending())
        .expect("current task starts");
    let error = session.clear().expect_err("next retirement is rejected");
    assert_eq!(error.to_string(), "clear fixture input");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the canceled interim task join capacity is reached")
    );
    assert!(session.input_snapshot().is_some());
    session.join_canceled().await.expect("retired tasks join");
    session
        .clear()
        .expect("clear retries after capacity drains");
}

#[test]
fn stale_interim_is_rejected_before_event_id_allocation() {
    let mut session = session();
    session
        .append_base64(&encoded(&[0, 0]))
        .expect("input appends");
    let (current_event, stale_event) = session
        .accept_interim_across_clear("current", "stale")
        .expect("interim clear scenario succeeds");
    let current_event = current_event.expect("current epoch is accepted");
    assert_eq!(current_event["delta"], "current");
    assert!(stale_event.is_none());
    assert_eq!(
        session.allocated_event_count(),
        1,
        "stale completion consumes no event ID"
    );
}
