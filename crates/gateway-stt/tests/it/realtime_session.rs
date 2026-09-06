use std::future::{Future, pending};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use base64::Engine as _;
use futures_util::FutureExt as _;
use gateway_stt::test_fixtures::{RealtimeSessionFixture, RealtimeSessionRegistryFixture};

const SESSION_CAPACITY: usize = 8;
const CANCEL_JOIN_CAPACITY: usize = 8;

fn encoded(samples: &[i16]) -> String {
    let bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn update(prompt: &str, include: bool) -> String {
    serde_json::json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {"input": {"transcription": {"prompt": prompt}}},
            "include": if include {
                vec!["item.input_audio_transcription.hypothesis"]
            } else {
                Vec::<&str>::new()
            }
        }
    })
    .to_string()
}

#[allow(
    clippy::expect_used,
    reason = "a fixture registry has no prior session that could consume capacity"
)]
fn session() -> RealtimeSessionFixture {
    RealtimeSessionRegistryFixture::default()
        .register()
        .expect("session registers")
}

struct BlockingDrop {
    dropping: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

impl Future for BlockingDrop {
    type Output = String;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl Drop for BlockingDrop {
    fn drop(&mut self) {
        self.dropping.store(true, Ordering::Release);
        while !self.release.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    }
}

async fn wait_until(predicate: impl Fn() -> bool) {
    for _ in 0..1_000 {
        if predicate() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition did not become true within the bounded yield budget");
}

#[test]
fn session_registration_has_no_wait_queue_at_capacity() {
    let registry = RealtimeSessionRegistryFixture::default();
    let sessions = (0..SESSION_CAPACITY)
        .map(|_| registry.register().expect("session is admitted"))
        .collect::<Vec<_>>();

    assert_eq!(
        registry.register().expect_err("ninth session is rejected"),
        "the realtime transcription session limit is reached"
    );
    drop(sessions);
    assert!(
        registry.register().is_ok(),
        "release immediately reopens admission"
    );
}

#[test]
fn first_append_freezes_configuration_and_clear_resets_audio_state() {
    let mut reused = session();
    reused
        .update_text(&update("first", true))
        .expect("first update applies");
    reused
        .append_base64(&base64::engine::general_purpose::STANDARD.encode([0x7f]))
        .expect("odd byte appends");
    let first = reused.input_snapshot().expect("first snapshot exists");

    reused
        .update_text(&update("second", false))
        .expect("second update applies");
    assert_eq!(
        reused.input_snapshot().expect("snapshot remains").prompt(),
        "first"
    );
    reused.clear().expect("input clears");
    reused
        .append_base64(&encoded(&vec![123; 2_400]))
        .expect("replacement input appends");
    let second = reused.input_snapshot().expect("second snapshot exists");
    assert_ne!(first.item_id(), second.item_id());
    assert_eq!(second.prompt(), "second");
    assert!(!second.include_hypothesis());

    let mut fresh = session();
    fresh
        .update_text(&update("second", false))
        .expect("fresh update applies");
    fresh
        .append_base64(&encoded(&vec![123; 2_400]))
        .expect("fresh input appends");
    assert_eq!(reused.resampled_audio(), fresh.resampled_audio());
}

#[test]
fn failed_first_append_does_not_capture_configuration() {
    let mut session = session();
    session
        .update_text(&update("before", false))
        .expect("first update applies");
    assert!(session.append_base64("not base64").is_err());
    assert!(session.input_snapshot().is_none());

    session
        .update_text(&update("after", true))
        .expect("replacement update applies");
    session
        .append_base64(&encoded(&[0, 1]))
        .expect("valid append succeeds");
    let snapshot = session.input_snapshot().expect("snapshot exists");
    assert_eq!(snapshot.prompt(), "after");
    assert!(snapshot.include_hypothesis());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_session_retains_admission_until_interim_cleanup_joins() {
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry.register().expect("session registers");
    session
        .append_base64(&encoded(&[0, 0]))
        .expect("input appends");
    let dropping = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    session
        .spawn_interim(BlockingDrop {
            dropping: Arc::clone(&dropping),
            release: Arc::clone(&release),
        })
        .expect("interim starts");

    drop(session);
    wait_until(|| dropping.load(Ordering::Acquire)).await;
    assert_eq!(registry.active(), 1, "retiring work keeps admission owned");
    release.store(true, Ordering::Release);
    wait_until(|| registry.active() == 0).await;
}

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
async fn canceling_join_keeps_capacity_owned_until_retry_completes() {
    let mut session = session();
    session
        .append_base64(&encoded(&[0, 0]))
        .expect("input appends");
    let dropping = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    session
        .spawn_interim(BlockingDrop {
            dropping: Arc::clone(&dropping),
            release: Arc::clone(&release),
        })
        .expect("interim starts");
    session.clear().expect("interim retires");
    wait_until(|| dropping.load(Ordering::Acquire)).await;

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
    assert_eq!(
        session.clear().expect_err("next retirement is rejected"),
        "the canceled interim task join capacity is reached"
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
    let stale = session.begin_interim().expect("epoch begins");
    let current_event = session
        .accept_interim(stale, "current".to_owned())
        .expect("event serializes")
        .expect("current epoch is accepted");
    assert_eq!(current_event["delta"], "current");
    assert_eq!(session.allocated_event_count(), 1);

    session.clear().expect("input clears");
    assert!(
        session
            .accept_interim(stale, "stale".to_owned())
            .expect("rejection does not serialize")
            .is_none()
    );
    assert_eq!(
        session.allocated_event_count(),
        1,
        "stale completion consumes no event ID"
    );
}
