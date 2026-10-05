//! Session admission, first-append configuration, and retirement cleanup.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures_util::FutureExt as _;
use gateway_stt::test_fixtures::RealtimeSessionRegistryFixture;

use super::{
    BLOCKING_TASK_TEST, BlockingFinalization, BlockingPoll, SESSION_CAPACITY, append_committable,
    encoded, session, source_message, update, wait_until_started,
};

#[test]
fn session_registration_has_no_wait_queue_at_capacity() {
    let registry = RealtimeSessionRegistryFixture::default();
    let sessions = (0..SESSION_CAPACITY)
        .map(|_| registry.register().expect("session is admitted"))
        .collect::<Vec<_>>();

    let error = registry.register().expect_err("ninth session is rejected");
    assert_eq!(error.to_string(), "register fixture session");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the realtime transcription session limit is reached")
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
#[expect(
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
    let error = registry.register().expect_err("capacity remains occupied");
    assert_eq!(error.to_string(), "register fixture session");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the realtime transcription session limit is reached")
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
#[expect(
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
    let error = registry.register().expect_err("capacity remains occupied");
    assert_eq!(error.to_string(), "register fixture session");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the realtime transcription session limit is reached")
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
