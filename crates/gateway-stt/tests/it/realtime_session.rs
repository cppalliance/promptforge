use std::future::{Future, pending};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use futures_util::FutureExt as _;
use gateway_stt::test_fixtures::{
    RealtimeSessionFixture, RealtimeSessionRegistryFixture, ScriptedDecoder, ScriptedModelFactory,
};

const SESSION_CAPACITY: usize = 8;
const CANCEL_JOIN_CAPACITY: usize = 8;
const COMMITTED_ITEM_CAPACITY: usize = 4;
const RESULT_CAPACITY: usize = 16;
static BLOCKING_TASK_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn encoded(samples: &[i16]) -> String {
    let bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn closed_segment() -> String {
    let mut samples = vec![16_384; 24_000];
    samples.extend(vec![0; 72_000]);
    encoded(&samples)
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

struct BlockingPoll {
    started: Arc<(Mutex<bool>, Condvar)>,
    release: Arc<AtomicBool>,
}

impl Future for BlockingPoll {
    type Output = String;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut started = self
            .started
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *started = true;
        self.started.1.notify_all();
        drop(started);
        while !self.release.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        Poll::Ready("released".to_owned())
    }
}

struct BlockingFinalization(BlockingPoll);

impl Future for BlockingFinalization {
    type Output = Result<String, String>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(context).map(Ok)
    }
}

fn wait_until_started(started: &Arc<(Mutex<bool>, Condvar)>) {
    let state = started
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (state, timeout) = started
        .1
        .wait_timeout_while(state, Duration::from_secs(1), |started| !*started)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(!timeout.timed_out() && *state, "blocked task starts");
}

async fn wait_until(predicate: impl Fn() -> bool) {
    assert!(
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if predicate() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok(),
        "condition reaches its wall-clock deadline"
    );
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
#[allow(
    clippy::await_holding_lock,
    reason = "the process-wide test lock serializes deliberately blocked runtime workers"
)]
async fn dropping_session_retains_admission_until_interim_cleanup_joins() {
    let _serial = BLOCKING_TASK_TEST
        .lock()
        .expect("test lock is not poisoned");
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry.register().expect("session registers");
    let other_sessions = (1..SESSION_CAPACITY)
        .map(|_| registry.register().expect("capacity is admitted"))
        .collect::<Vec<_>>();
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
    let cleanup_events = registry.cleanup_event_count();
    drop(session);
    let cleanup = registry.cleanup_notified();
    tokio::pin!(cleanup);
    assert!(
        cleanup.as_mut().now_or_never().is_none(),
        "cleanup waiter starts before task release"
    );
    assert_eq!(
        registry.active(),
        SESSION_CAPACITY,
        "retiring work keeps admission owned"
    );
    assert_eq!(
        registry.register().expect_err("capacity remains occupied"),
        "the realtime transcription session limit is reached"
    );

    release.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(1), cleanup)
        .await
        .expect("registry cleanup reaches its wall-clock deadline");
    assert_eq!(
        registry.cleanup_event_count(),
        cleanup_events + 1,
        "one retirement emits exactly one cleanup event"
    );
    assert_eq!(registry.active(), SESSION_CAPACITY - 1);
    let replacement = registry
        .register()
        .expect("completed cleanup immediately reopens admission");
    drop(replacement);
    drop(other_sessions);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(
    clippy::await_holding_lock,
    reason = "the process-wide test lock serializes deliberately blocked runtime workers"
)]
async fn dropping_session_retains_admission_until_finalization_cleanup_joins() {
    let _serial = BLOCKING_TASK_TEST
        .lock()
        .expect("test lock is not poisoned");
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry.register().expect("session registers");
    let other_sessions = (1..SESSION_CAPACITY)
        .map(|_| registry.register().expect("capacity is admitted"))
        .collect::<Vec<_>>();
    let item = {
        append_committable(&mut session);
        session.commit().expect("item commits")
    };
    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Arc::new(AtomicBool::new(false));
    session
        .replace_finalization(
            item.item_id(),
            BlockingFinalization(BlockingPoll {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
            }),
        )
        .expect("controlled finalization starts");

    wait_until_started(&started);
    let cleanup_events = registry.cleanup_event_count();
    let cleanup = registry.cleanup_notified();
    tokio::pin!(cleanup);
    assert!(
        cleanup.as_mut().now_or_never().is_none(),
        "cleanup waiter starts before session retirement"
    );
    drop(session);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), cleanup.as_mut())
            .await
            .is_err(),
        "parked finalization keeps cleanup pending"
    );
    assert_eq!(
        registry.active(),
        SESSION_CAPACITY,
        "retiring finalization keeps admission owned"
    );
    assert_eq!(
        registry.register().expect_err("capacity remains occupied"),
        "the realtime transcription session limit is reached"
    );

    release.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(1), cleanup.as_mut())
        .await
        .expect("finalization cleanup reaches its wall-clock deadline");
    assert_eq!(
        registry.cleanup_event_count(),
        cleanup_events + 1,
        "one finalization retirement emits exactly one cleanup event"
    );
    assert_eq!(registry.active(), SESSION_CAPACITY - 1);
    let replacement = registry
        .register()
        .expect("completed finalization cleanup immediately reopens admission");
    drop(replacement);
    drop(other_sessions);
}

#[tokio::test]
async fn retired_task_join_failures_are_preserved() {
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry.register().expect("session registers");
    session
        .append_base64(&encoded(&[0, 0]))
        .expect("input appends");
    let (started, started_rx) = tokio::sync::oneshot::channel();
    session
        .spawn_interim(async move {
            let _ = started.send(());
            panic!("retired interim task panic")
        })
        .expect("interim starts");
    tokio::time::timeout(Duration::from_secs(1), started_rx)
        .await
        .expect("panicking task starts before retirement")
        .expect("panicking task reports startup");
    let cleanup = registry.cleanup_notified();
    tokio::pin!(cleanup);
    assert!(
        cleanup.as_mut().now_or_never().is_none(),
        "cleanup waiter starts before retirement"
    );

    drop(session);
    tokio::time::timeout(Duration::from_secs(1), cleanup)
        .await
        .expect("failed task cleanup reaches its wall-clock deadline");
    assert_eq!(
        registry.retired_task_failures(),
        1,
        "retired task panic remains observable after admission release"
    );
}

#[tokio::test]
async fn missing_cleanup_notification_reaches_wall_clock_deadline() {
    let registry = RealtimeSessionRegistryFixture::default();

    assert!(
        tokio::time::timeout(Duration::from_millis(25), registry.cleanup_notified())
            .await
            .is_err(),
        "missing cleanup reaches the bounded wall-clock timeout"
    );
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
#[allow(
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

#[allow(
    clippy::expect_used,
    reason = "the helper establishes valid canonical fixture audio and input"
)]
fn append_committable(session: &mut RealtimeSessionFixture) -> String {
    session
        .append_base64(&encoded(&vec![0; 2_400]))
        .expect("committable input appends");
    session
        .input_snapshot()
        .expect("provisional input exists")
        .item_id()
        .to_owned()
}

#[test]
fn commit_promotes_the_provisional_id_and_preserves_durable_lineage() {
    let mut session = session();
    let first_provisional = append_committable(&mut session);
    let first = session.commit().expect("first item commits");
    assert_eq!(first.item_id(), first_provisional);
    assert_eq!(first.previous_item_id(), None);

    session
        .finalize_completed(first.item_id(), "first")
        .expect("first item finalizes");
    let second_provisional = append_committable(&mut session);
    let second = session.commit().expect("second item commits");
    assert_eq!(second.item_id(), second_provisional);
    assert_eq!(second.previous_item_id(), Some(first.item_id()));
}

#[test]
fn committed_capacity_is_reserved_before_input_detach_and_retryable() {
    let mut session = session();
    let mut committed = Vec::new();
    for _ in 0..COMMITTED_ITEM_CAPACITY {
        append_committable(&mut session);
        committed.push(session.commit().expect("item commits within capacity"));
    }
    assert_eq!(session.committed_count(), COMMITTED_ITEM_CAPACITY);

    let retry_id = append_committable(&mut session);
    assert_eq!(
        session.commit().expect_err("fifth item is rejected"),
        "the committed realtime item limit is reached"
    );
    assert_eq!(
        session
            .input_snapshot()
            .expect("rejected commit preserves input")
            .item_id(),
        retry_id
    );

    session
        .finalize_completed(committed[0].item_id(), "done")
        .expect("one item releases capacity");
    session.drain_results();
    let retried = session.commit().expect("same input retries");
    assert_eq!(retried.item_id(), retry_id);
}

#[tokio::test]
async fn four_items_finalize_in_reverse_order_without_crossing_ownership() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.park_next();
    for index in 0..COMMITTED_ITEM_CAPACITY {
        final_decoder.push_text(format!("result-{index}"));
    }
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry
        .register_with_scripted_engine(
            ScriptedModelFactory::new(interim).with_final(final_decoder.clone()),
        )
        .expect("scripted session starts");
    let mut ids = Vec::new();
    for index in 0..COMMITTED_ITEM_CAPACITY {
        session
            .update_text(&update(&format!("prompt-{index}"), true))
            .expect("item prompt updates");
        append_committable(&mut session);
        ids.push(session.commit().expect("item commits").item_id().to_owned());
    }
    assert_eq!(
        session.finalizing_count(),
        COMMITTED_ITEM_CAPACITY,
        "every committed take owns an asynchronous finalization"
    );
    tokio::task::yield_now().await;
    assert!(
        final_decoder.wait_until_parked(Duration::from_secs(1)),
        "one accurate final decode parks while all item tasks remain owned"
    );
    final_decoder.release();

    for (index, item_id) in ids.iter().enumerate().rev() {
        assert_eq!(
            session
                .committed_prompt_and_guidance(item_id)
                .expect("item retains its immutable take state"),
            (format!("prompt-{index}"), vec![format!("prompt-{index}")])
        );
        session
            .finish_finalization(item_id)
            .await
            .expect("item finalizes independently through its take");
    }
    assert_eq!(session.finalizing_count(), 0);
    let terminals = session.drain_results();
    assert_eq!(
        terminals
            .iter()
            .filter_map(|event| event["item_id"].as_str())
            .collect::<Vec<_>>(),
        ids.iter().rev().map(String::as_str).collect::<Vec<_>>()
    );
    let requests = final_decoder.requests();
    assert_eq!(requests.len(), COMMITTED_ITEM_CAPACITY);
    for (index, event) in terminals.iter().enumerate() {
        let item_index = COMMITTED_ITEM_CAPACITY - index - 1;
        let prompt = format!("prompt-{item_index}");
        let request_index = requests
            .iter()
            .position(|request| request.guidance() == [prompt.as_str()])
            .expect("each item guidance reaches one final request");
        assert_eq!(event["transcript"], format!("result-{request_index}"));
    }
}

#[tokio::test]
async fn canceling_item_finish_keeps_finalization_owned_for_retry() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("authoritative");
    final_decoder.park_next();
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry
        .register_with_scripted_engine(
            ScriptedModelFactory::new(interim).with_final(final_decoder.clone()),
        )
        .expect("scripted session starts");
    append_committable(&mut session);
    let item = session.commit().expect("item commits");
    let item_id = item.item_id().to_owned();

    tokio::task::yield_now().await;
    assert!(
        final_decoder.wait_until_parked(Duration::from_secs(1)),
        "the accurate decode remains parked"
    );
    assert!(
        session
            .finish_finalization(&item_id)
            .now_or_never()
            .is_none(),
        "canceling the first join poll cannot detach finalization"
    );
    assert_eq!(session.finalizing_count(), 1);
    final_decoder.release();
    session
        .finish_finalization(&item_id)
        .await
        .expect("retry joins the same finalization");

    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "completed");
    assert_eq!(results[0]["transcript"], "authoritative");
}

#[test]
fn result_capacity_hypothesis_replacement_and_terminal_reservation_are_independent() {
    let mut session = session();
    append_committable(&mut session);
    let item = session.commit().expect("item commits");
    for index in 0..RESULT_CAPACITY {
        session
            .push_delta(item.item_id(), &format!("delta-{index}"))
            .expect("result enters bounded capacity");
    }
    assert_eq!(
        session
            .push_delta(item.item_id(), "overflow")
            .expect_err("capacity-plus-one is rejected"),
        "the realtime session result capacity is reached"
    );

    session
        .replace_hypothesis(item.item_id(), 1, "old")
        .expect("first hypothesis enters its slot");
    session
        .replace_hypothesis(item.item_id(), 2, "new")
        .expect("new hypothesis replaces old");
    session
        .finalize_completed(item.item_id(), "authoritative")
        .expect("terminal uses its reserved slot despite saturation");

    let results = session.drain_results();
    assert_eq!(
        results
            .iter()
            .filter(|event| event["type"] == "delta")
            .count(),
        RESULT_CAPACITY
    );
    let hypothesis = results
        .iter()
        .find(|event| event["type"] == "hypothesis")
        .expect("one replaceable hypothesis remains");
    assert_eq!(hypothesis["revision"], 2);
    assert_eq!(hypothesis["transcript"], "new");
    assert_eq!(
        results
            .iter()
            .filter(|event| event["type"] == "completed")
            .count(),
        1
    );
}

#[test]
fn pending_precommit_failure_blocks_append_but_commits_one_item_failure() {
    let mut session = session();
    let item_id = append_committable(&mut session);
    session
        .fail_precommit("accurate segment failed")
        .expect("failure is retained by the input");
    assert_eq!(
        session
            .append_base64(&encoded(&[0, 0]))
            .expect_err("failed input rejects later audio"),
        "accurate segment failed"
    );

    let committed = session
        .commit()
        .expect("failed input still establishes item");
    assert_eq!(committed.item_id(), item_id);
    assert_eq!(
        session
            .finalize_failed(committed.item_id(), "duplicate")
            .expect_err("a second terminal is rejected"),
        "the committed item already reached a terminal outcome"
    );
    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "failed");
    assert_eq!(results[0]["message"], "accurate segment failed");
}

#[test]
fn clear_discards_pending_precommit_failure_without_creating_an_item() {
    let mut session = session();
    append_committable(&mut session);
    session
        .fail_precommit("discard me")
        .expect("failure is retained");
    session.clear().expect("failed uncommitted input clears");
    assert_eq!(session.committed_count(), 0);
    assert!(session.drain_results().is_empty());
}

#[tokio::test]
async fn asynchronous_final_failure_is_observed_before_the_next_append_and_at_commit() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_error("late accurate failure");
    final_decoder.park_next();
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry
        .register_with_scripted_engine(
            ScriptedModelFactory::new(interim).with_final(final_decoder.clone()),
        )
        .expect("scripted session starts");

    session
        .append_base64(&closed_segment())
        .expect("closed segment enters the production take");
    tokio::task::yield_now().await;
    assert!(
        final_decoder.wait_until_parked(Duration::from_secs(1)),
        "the accurate segment is running between appends"
    );
    final_decoder.release();
    wait_until(|| session.pending_failure().is_some()).await;
    let failure = session
        .pending_failure()
        .expect("the take owns the asynchronous failure");

    assert_eq!(
        session
            .append_base64(&encoded(&[1, 2]))
            .expect_err("the next append is rejected before mutating audio"),
        failure
    );
    let item = session
        .commit()
        .expect("commit still establishes the failed item");
    assert_eq!(session.finalizing_count(), 0);
    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["item_id"], item.item_id());
    assert_eq!(results[0]["type"], "failed");
    assert_eq!(results[0]["message"], failure);
}

#[tokio::test]
async fn production_final_segment_admission_is_exact_and_fails_atomically() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.park_next();
    final_decoder.push_text("first");
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry
        .register_with_scripted_engine(
            ScriptedModelFactory::new(interim).with_final(final_decoder.clone()),
        )
        .expect("scripted session starts");

    session
        .append_base64(&closed_segment())
        .expect("first closed segment is admitted");
    tokio::task::yield_now().await;
    assert!(
        final_decoder.wait_until_parked(Duration::from_secs(1)),
        "the first production segment parks in final decoding"
    );
    assert_eq!(session.pending_final_segments(), Some(1));

    for expected in 2..=4 {
        session
            .append_base64(&closed_segment())
            .expect("segment is accepted through exact capacity");
        assert_eq!(session.pending_final_segments(), Some(expected));
        assert!(session.pending_failure().is_none());
    }

    session
        .append_base64(&closed_segment())
        .expect("audio ingestion remains recoverable at segment saturation");
    assert_eq!(session.pending_final_segments(), Some(4));
    assert_eq!(
        session.pending_failure().as_deref(),
        Some("final segment capacity is reached")
    );
    let item = session
        .commit()
        .expect("capacity failure atomically becomes an item failure");
    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["item_id"], item.item_id());
    assert_eq!(results[0]["message"], "final segment capacity is reached");
    final_decoder.release();
}

#[tokio::test]
async fn commit_reserves_interim_join_capacity_before_detaching_input() {
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

    let provisional = append_committable(&mut session);
    session
        .spawn_interim(pending())
        .expect("current interim starts");
    assert_eq!(
        session
            .commit()
            .expect_err("commit cannot detach an unowned task"),
        "the canceled interim task join capacity is reached"
    );
    assert_eq!(
        session
            .input_snapshot()
            .expect("rejected commit preserves input")
            .item_id(),
        provisional
    );
    session.join_canceled().await.expect("retired tasks join");
    assert_eq!(
        session.commit().expect("retry commits").item_id(),
        provisional
    );
}
