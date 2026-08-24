use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Deterministically wait until exactly `n` requests are enqueued as waiters,
/// yielding to the runtime so spawned admits can register (no sleeps).
async fn await_waiters(queue: &DominionQueue, n: usize) {
    while queue.waiter_count() != n {
        tokio::task::yield_now().await;
    }
}

#[test]
fn client_id_accepts_bounded_identity() {
    assert_eq!(ClientId::parse("agent-7").as_str(), "agent-7");
    assert_eq!(ClientId::parse("  tenant.a:1  ").as_str(), "tenant.a:1");
}

#[test]
fn client_id_rejects_invalid_and_oversized() {
    assert_eq!(ClientId::parse("").as_str(), ClientId::DEFAULT);
    assert_eq!(ClientId::parse("has space").as_str(), ClientId::DEFAULT);
    assert_eq!(ClientId::parse("bad/slash").as_str(), ClientId::DEFAULT);
    let oversized = "x".repeat(ClientId::MAX_LEN + 1);
    assert_eq!(ClientId::parse(&oversized).as_str(), ClientId::DEFAULT);
    assert_eq!(ClientId::from_header(None).as_str(), ClientId::DEFAULT);
}

#[tokio::test]
async fn unlimited_queue_admits_immediately() {
    let queue = DominionQueue::unlimited();
    let permit = queue.admit("default").await.unwrap();
    assert!(permit.limited.is_none());
}

#[tokio::test]
async fn concurrency_one_serializes_two_admits() {
    let queue = DominionQueue::new(1, 100, true, QueuePolicy::Queue);
    let phase = Arc::new(AtomicUsize::new(0));

    let first = queue.admit("a").await.unwrap();
    phase.store(1, Ordering::SeqCst);

    let queue2 = queue.clone();
    let phase2 = Arc::clone(&phase);
    let second = tokio::spawn(async move {
        phase2.store(2, Ordering::SeqCst);
        let permit = queue2.admit("a").await.unwrap();
        phase2.store(3, Ordering::SeqCst);
        permit
    });

    await_waiters(&queue, 1).await;
    assert_eq!(
        phase.load(Ordering::SeqCst),
        2,
        "second admit still waiting"
    );

    drop(first);
    let _second_permit = second.await.unwrap();
    assert_eq!(phase.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn queue_full_rejects_when_max_depth_waiters_present() {
    // concurrency=1, max_depth=1: one in-flight + one waiting; third fails.
    let queue = DominionQueue::new(1, 1, true, QueuePolicy::Queue);
    let inflight = queue.admit("a").await.unwrap();

    let queue_wait = queue.clone();
    let waiting = tokio::spawn(async move { queue_wait.admit("b").await });
    await_waiters(&queue, 1).await;

    let err = queue.admit("c").await.unwrap_err();
    assert_eq!(err, AdmitError::QueueFull);

    drop(inflight);
    let _waiting_permit = waiting.await.unwrap().unwrap();
}

#[tokio::test]
async fn reject_policy_fails_fast_at_capacity_without_queueing() {
    // policy=Reject: a full in-flight set rejects immediately; no waiter is
    // ever enqueued, so max_depth is irrelevant under this policy.
    let queue = DominionQueue::new(1, 10, true, QueuePolicy::Reject);
    let held = queue.admit("a").await.unwrap();

    let err = queue.admit("b").await.unwrap_err();
    assert_eq!(err, AdmitError::Rejected);
    assert_eq!(queue.waiter_count(), 0, "reject policy never enqueues");

    // Once the slot frees, admission succeeds again.
    drop(held);
    let permit = queue.admit("b").await.unwrap();
    drop(permit);
}

#[tokio::test]
async fn reject_policy_admits_up_to_capacity_then_rejects() {
    let queue = DominionQueue::new(2, 10, true, QueuePolicy::Reject);
    let _first = queue.admit("a").await.unwrap();
    let _second = queue.admit("b").await.unwrap();
    assert_eq!(queue.admit("c").await.unwrap_err(), AdmitError::Rejected);
}

#[test]
fn from_queue_config_preserves_legacy_queue_settings() {
    // The legacy `[queue]` shim carries depth and fairness and always uses
    // the Queue policy; the reject policy only arrives via dominions.
    let queue = DominionQueue::from_queue_config(1, &QueueConfig::new(3, false));
    let QueueInner::Limited(limited) = &queue.inner else {
        panic!("expected a limited queue");
    };
    assert_eq!(limited.max_depth, 3);
    assert!(!limited.fair);
    assert_eq!(limited.policy, QueuePolicy::Queue);
}

#[tokio::test]
async fn fair_scheduling_interleaves_clients() {
    // concurrency=1; enqueue A, A, B while one A holds. Fair wake order
    // is A, B, A (not FIFO A, A, B).
    let queue = DominionQueue::new(1, 10, true, QueuePolicy::Queue);
    let held = queue.admit("A").await.unwrap();

    let order = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for (registered, key) in ["A", "A", "B"].into_iter().enumerate() {
        let queue_task = queue.clone();
        let order = Arc::clone(&order);
        let key = key.to_string();
        handles.push(tokio::spawn(async move {
            let permit = queue_task.admit(&key).await.unwrap();
            order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(key);
            // Release immediately so the next fair waiter can run.
            drop(permit);
        }));
        // Deterministically wait for this waiter to enqueue before the next
        // one, so fair-scheduling order is well defined (no sleeps).
        await_waiters(&queue, registered + 1).await;
    }

    drop(held);
    for handle in handles {
        handle.await.unwrap();
    }

    let got = order
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(got, vec!["A", "B", "A"]);
}

#[tokio::test]
async fn cancel_while_queued_frees_the_waiter_slot() {
    // concurrency=1: hold the only slot, queue a waiter, then cancel the waiter
    // future before it is ever woken. Its queue entry must be removed so the
    // waiter count returns to zero and a fresh admit can queue again.
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    let queue = DominionQueue::new(1, 10, true, QueuePolicy::Queue);
    let held = queue.admit("a").await.unwrap();

    let mut waiting = Box::pin(queue.admit("b"));
    poll_fn(|cx| match waiting.as_mut().poll(cx) {
        Poll::Ready(_) => panic!("waiter should be queued, not ready"),
        Poll::Pending => Poll::Ready(()),
    })
    .await;
    await_waiters(&queue, 1).await;

    drop(waiting);
    await_waiters(&queue, 0).await;

    // The slot is still held, so a new caller queues rather than being admitted.
    let queue_next = queue.clone();
    let next = tokio::spawn(async move { queue_next.admit("c").await });
    await_waiters(&queue, 1).await;
    drop(held);
    let _permit = next.await.unwrap().unwrap();
}

#[test]
fn queue_full_rejected_and_unavailable_are_distinct() {
    // Q-006: a closed notification channel is not the same condition as live
    // back-pressure, and neither is the fail-fast rejection; the queue layer
    // keeps all three variants distinct.
    let variants = [
        AdmitError::QueueFull,
        AdmitError::Rejected,
        AdmitError::Unavailable,
    ];
    for (index, first) in variants.iter().enumerate() {
        for second in &variants[index + 1..] {
            assert_ne!(first, second);
            assert_ne!(first.to_string(), second.to_string());
        }
    }
}

#[tokio::test]
async fn cancel_after_wake_does_not_leak_slot() {
    // concurrency=1: queue a waiter, wake it by dropping the holder, then
    // cancel the waiter future before it returns a Permit. The granted slot
    // must be released so a later admit can still acquire.
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    let queue = DominionQueue::new(1, 10, true, QueuePolicy::Queue);
    let held = queue.admit("a").await.unwrap();

    let mut waiting = Box::pin(queue.admit("b"));
    poll_fn(|cx| match waiting.as_mut().poll(cx) {
        Poll::Ready(_) => panic!("waiter should be queued, not ready"),
        Poll::Pending => Poll::Ready(()),
    })
    .await;

    drop(held);
    // Cancel while CancelOnDrop is still armed after the oneshot transfer.
    drop(waiting);

    let permit = tokio::time::timeout(Duration::from_millis(500), queue.admit("c"))
        .await
        .expect("slot leaked: subsequent admit timed out")
        .unwrap();
    drop(permit);
}

#[test]
fn new_is_total_and_never_panics_on_zero_settings() {
    // Q-002: a zero concurrency yields an unlimited queue; a zero max_depth is
    // clamped. Construction never panics on out-of-range runtime settings.
    let unlimited = DominionQueue::new(0, 100, true, QueuePolicy::Queue);
    assert!(matches!(unlimited.inner, QueueInner::Unlimited));
    let clamped = DominionQueue::new(1, 0, true, QueuePolicy::Queue);
    assert!(matches!(clamped.inner, QueueInner::Limited(_)));
}

#[tokio::test]
async fn distinct_client_labels_are_capped_to_bound_fair_scheduling() {
    // Q-001: one caller minting many labels cannot expand the round-robin
    // breadth without bound; labels beyond the cap fold into `default`.
    let queue = DominionQueue::new(1, 1000, true, QueuePolicy::Queue);
    // Occupy the only in-flight slot so subsequent admits queue as waiters.
    let _held = queue.admit("holder").await.unwrap();

    let total = MAX_DISTINCT_CLIENTS + 10;
    let mut handles = Vec::new();
    for i in 0..total {
        let queue = queue.clone();
        handles.push(tokio::spawn(async move {
            let _ = queue.admit(&format!("client-{i}")).await;
        }));
    }
    await_waiters(&queue, total).await;

    // Distinct buckets are bounded: at most the cap of named labels plus the one
    // shared `default` overflow bucket, regardless of how many labels are minted.
    assert!(
        queue.distinct_clients() <= MAX_DISTINCT_CLIENTS + 1,
        "distinct client buckets {} exceeded bound {}",
        queue.distinct_clients(),
        MAX_DISTINCT_CLIENTS + 1
    );

    for handle in handles {
        handle.abort();
    }
}
