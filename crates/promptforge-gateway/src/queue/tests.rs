use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Deterministically wait until exactly `n` requests are enqueued as waiters,
/// yielding to the runtime so spawned admits can register (no sleeps).
async fn await_waiters(lane: &EndpointLane, n: usize) {
    while lane.waiter_count() != n {
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
async fn unlimited_lane_admits_immediately() {
    let lane = EndpointLane::unlimited();
    let permit = lane.admit("default").await.unwrap();
    assert!(permit.limited.is_none());
}

#[tokio::test]
async fn concurrency_one_serializes_two_admits() {
    let lane = EndpointLane::new(1, &QueueConfig::default());
    let phase = Arc::new(AtomicUsize::new(0));

    let first = lane.admit("a").await.unwrap();
    phase.store(1, Ordering::SeqCst);

    let lane2 = lane.clone();
    let phase2 = Arc::clone(&phase);
    let second = tokio::spawn(async move {
        phase2.store(2, Ordering::SeqCst);
        let permit = lane2.admit("a").await.unwrap();
        phase2.store(3, Ordering::SeqCst);
        permit
    });

    await_waiters(&lane, 1).await;
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
    let lane = EndpointLane::new(
        1,
        &QueueConfig {
            max_depth: 1,
            fair_scheduling: true,
        },
    );
    let inflight = lane.admit("a").await.unwrap();

    let lane_wait = lane.clone();
    let waiting = tokio::spawn(async move { lane_wait.admit("b").await });
    await_waiters(&lane, 1).await;

    let err = lane.admit("c").await.unwrap_err();
    assert_eq!(err, AdmitError::QueueFull);

    drop(inflight);
    let _waiting_permit = waiting.await.unwrap().unwrap();
}

#[tokio::test]
async fn fair_scheduling_interleaves_clients() {
    // concurrency=1; enqueue A, A, B while one A holds. Fair wake order
    // is A, B, A (not FIFO A, A, B).
    let lane = EndpointLane::new(
        1,
        &QueueConfig {
            max_depth: 10,
            fair_scheduling: true,
        },
    );
    let held = lane.admit("A").await.unwrap();

    let order = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for (registered, key) in ["A", "A", "B"].into_iter().enumerate() {
        let lane_task = lane.clone();
        let order = Arc::clone(&order);
        let key = key.to_string();
        handles.push(tokio::spawn(async move {
            let permit = lane_task.admit(&key).await.unwrap();
            order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(key);
            // Release immediately so the next fair waiter can run.
            drop(permit);
        }));
        // Deterministically wait for this waiter to enqueue before the next
        // one, so fair-scheduling order is well defined (no sleeps).
        await_waiters(&lane, registered + 1).await;
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
async fn cancel_after_wake_does_not_leak_slot() {
    // concurrency=1: queue a waiter, wake it by dropping the holder, then
    // cancel the waiter future before it returns a Permit. The granted slot
    // must be released so a later admit can still acquire.
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    let lane = EndpointLane::new(
        1,
        &QueueConfig {
            max_depth: 10,
            fair_scheduling: true,
        },
    );
    let held = lane.admit("a").await.unwrap();

    let mut waiting = Box::pin(lane.admit("b"));
    poll_fn(|cx| match waiting.as_mut().poll(cx) {
        Poll::Ready(_) => panic!("waiter should be queued, not ready"),
        Poll::Pending => Poll::Ready(()),
    })
    .await;

    drop(held);
    // Cancel while CancelOnDrop is still armed after the oneshot transfer.
    drop(waiting);

    let permit = tokio::time::timeout(Duration::from_millis(500), lane.admit("c"))
        .await
        .expect("slot leaked: subsequent admit timed out")
        .unwrap();
    drop(permit);
}
