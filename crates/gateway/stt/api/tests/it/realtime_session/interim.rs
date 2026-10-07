//! Interim task ownership, retry, canceled-join capacity, the discard of
//! empty interim transcripts, and plain-session deltas during the take.

use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use futures_util::FutureExt as _;
use gateway_stt::test_fixtures::{
    RealtimeSessionFixture, RealtimeSessionRegistryFixture, ScriptedDecoder, ScriptedModelFactory,
};
use serde_json::Value;

use super::{
    BLOCKING_TASK_TEST, BlockingPoll, CANCEL_JOIN_CAPACITY, encoded, session, source_message,
    update, wait_until_started,
};

const SNAPSHOT_FIELDS: [&str; 7] = [
    "revision",
    "transcript",
    "finalized",
    "agreed",
    "tentative",
    "audio_start_ms",
    "audio_end_ms",
];

#[expect(
    clippy::expect_used,
    reason = "a fresh registry has capacity and the update is a valid hypothesis include"
)]
fn hypothesis_session(interim: &ScriptedDecoder) -> RealtimeSessionFixture {
    let mut session = RealtimeSessionRegistryFixture::default()
        .register_with_scripted_engine(ScriptedModelFactory::new(interim.clone()))
        .expect("scripted session starts");
    session
        .update_text(&update("", true))
        .expect("hypothesis include applies");
    session
}

/// Appends `seconds` of 24 kHz speech-level audio, runs one production
/// interim, and returns its hypothesis snapshot fields.
#[expect(
    clippy::expect_used,
    reason = "speech-level audio appends and its scripted interim decodes"
)]
async fn speak_then_decode(session: &mut RealtimeSessionFixture, seconds: usize) -> Option<Value> {
    let second = encoded(&vec![16_384; 24_000]);
    for _ in 0..seconds {
        session.append_base64(&second).expect("speech appends");
    }
    let event = session.run_interim().await.expect("the interim runs")?;
    Some(
        SNAPSHOT_FIELDS
            .iter()
            .map(|field| ((*field).to_owned(), event[*field].clone()))
            .collect(),
    )
}

#[tokio::test]
async fn an_empty_interim_transcript_leaves_the_snapshot_and_agreement_unchanged() {
    let interim = ScriptedDecoder::new();
    for text in ["alpha beta", "", "alpha beta"] {
        interim.push_text(text);
    }
    let mut session = hypothesis_session(&interim);
    let first = speak_then_decode(&mut session, 1)
        .await
        .expect("the first interim emits a hypothesis");
    assert_eq!(
        speak_then_decode(&mut session, 1).await,
        None,
        "the empty interim emits nothing"
    );
    assert_eq!(
        interim.requests().len(),
        2,
        "the empty interim decoded before the session discarded it"
    );
    let after = speak_then_decode(&mut session, 1)
        .await
        .expect("the next interim emits a hypothesis");

    let control_interim = ScriptedDecoder::new();
    for text in ["alpha beta", "alpha beta"] {
        control_interim.push_text(text);
    }
    let mut control = hypothesis_session(&control_interim);
    assert_eq!(speak_then_decode(&mut control, 1).await, Some(first));
    let expected = speak_then_decode(&mut control, 2)
        .await
        .expect("the control's second interim emits a hypothesis");
    assert_eq!(
        after, expected,
        "the take advances as if the empty interim never arrived"
    );
    assert_eq!(after["agreed"], "alpha beta");
}

#[tokio::test]
async fn a_plain_session_receives_agreed_deltas_before_commit() {
    let interim = ScriptedDecoder::new();
    for text in ["alpha beta gamma", "alpha beta gamma"] {
        interim.push_text(text);
    }
    let mut session = RealtimeSessionRegistryFixture::default()
        .register_with_scripted_engine(ScriptedModelFactory::new(interim.clone()))
        .expect("scripted session starts");
    let second = encoded(&vec![16_384; 24_000]);
    session.append_base64(&second).expect("speech appends");
    assert_eq!(
        session.run_interim().await.expect("the interim runs"),
        None,
        "tentative text sends nothing"
    );
    session.append_base64(&second).expect("speech appends");
    let delta = session
        .run_interim()
        .await
        .expect("the interim runs")
        .expect("agreed text streams before commit");
    let input = session
        .input_snapshot()
        .expect("the input is still uncommitted");
    assert!(!input.include_hypothesis());
    assert_eq!(
        delta["type"],
        "conversation.item.input_audio_transcription.delta"
    );
    assert_eq!(delta["item_id"], input.item_id());
    assert_eq!(
        delta["delta"], "alpha",
        "the last two agreed words wait for later agreement or commit"
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
