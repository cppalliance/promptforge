//! Tests for session configuration snapshots, interim epochs, commit
//! capacity, canceled task joins, and clear.

use std::future::pending;

use base64::Engine as _;

use super::{MAX_COMMITTED_ITEMS_PER_SESSION, SESSION_CANCEL_JOIN_CAPACITY, Session, SessionError};
use crate::realtime::registry::SessionRegistry;

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

fn update_with_ranges(prompt: &str) -> String {
    serde_json::json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {"input": {"transcription": {"prompt": prompt}}},
            "include": [
                "item.input_audio_transcription.hypothesis",
                "item.input_audio_transcription.hypothesis.ranges"
            ]
        }
    })
    .to_string()
}

fn session() -> Session {
    let registration = SessionRegistry::default()
        .register()
        .expect("session registers");
    Session::new(registration, None)
}

#[test]
fn first_successful_append_freezes_configuration_until_clear() {
    let mut session = session();
    session
        .update_text(&update("first", true))
        .expect("first update applies");
    session
        .append_base64(&encoded(&[1, 2, 3]))
        .expect("first append succeeds");
    let first_item = session.input().expect("input exists").item_id().to_owned();

    session
        .update_text(&update("second", false))
        .expect("second update applies");
    let input = session.input().expect("input remains");
    assert_eq!(input.item_id(), first_item);
    assert_eq!(input.snapshot().prompt(), "first");
    assert!(input.snapshot().include_hypothesis());

    session.clear().expect("clear succeeds");
    session
        .append_base64(&encoded(&[4, 5, 6]))
        .expect("next input appends");
    let input = session.input().expect("replacement input exists");
    assert_ne!(input.item_id(), first_item);
    assert_eq!(input.snapshot().prompt(), "second");
    assert!(!input.snapshot().include_hypothesis());
}

#[test]
fn first_successful_append_freezes_the_ranges_include_until_clear() {
    let mut session = session();
    session
        .update_text(&update_with_ranges("first"))
        .expect("ranges update applies");
    session
        .append_base64(&encoded(&[1, 2, 3]))
        .expect("first append succeeds");

    session
        .update_text(&update("second", true))
        .expect("base hypothesis update applies");
    let input = session.input().expect("input remains");
    assert!(input.snapshot().include_hypothesis());
    assert!(input.snapshot().include_ranges());

    session.clear().expect("clear succeeds");
    session
        .append_base64(&encoded(&[4, 5, 6]))
        .expect("next input appends");
    let input = session.input().expect("replacement input exists");
    assert!(input.snapshot().include_hypothesis());
    assert!(!input.snapshot().include_ranges());
}

#[test]
fn failed_first_append_does_not_capture_a_snapshot() {
    let mut session = session();
    session
        .update_text(&update("before", false))
        .expect("update applies");
    assert!(session.append_base64("not base64").is_err());
    assert!(session.input().is_none());

    session
        .update_text(&update("after", true))
        .expect("replacement update applies");
    session
        .append_base64(&encoded(&[0, 1]))
        .expect("valid append succeeds");
    assert_eq!(
        session.input().expect("input exists").snapshot().prompt(),
        "after"
    );
}

#[test]
fn clear_retires_only_input_and_rejects_stale_interim_epochs() {
    let mut session = session();
    session
        .append_base64(&encoded(&vec![0; 2_400]))
        .expect("audio appends");
    let epoch = session.begin_interim().expect("epoch begins");
    assert!(
        session
            .accept_interim(epoch, "current".to_owned())
            .is_some()
    );

    session.clear().expect("clear succeeds");
    assert!(session.input().is_none());
    assert!(session.accept_interim(epoch, "stale".to_owned()).is_none());

    session
        .append_base64(&encoded(&vec![0; 2_400]))
        .expect("replacement audio appends");
    let next = session.begin_interim().expect("new epoch begins");
    assert_ne!(next, epoch);
    assert!(session.accept_interim(epoch, "stale".to_owned()).is_none());
    assert!(session.accept_interim(next, "fresh".to_owned()).is_some());
}

#[test]
fn miri_interim_epoch_rejects_results_after_clear_and_reuse() {
    let mut session = session();
    session
        .append_base64(&encoded(&[0, 0]))
        .expect("input appends");
    let stale = session.begin_interim().expect("first epoch begins");
    session.clear().expect("input clears");
    session
        .append_base64(&encoded(&[0, 0]))
        .expect("replacement input appends");
    let current = session.begin_interim().expect("next epoch begins");

    assert!(session.accept_interim(stale, "stale".to_owned()).is_none());
    assert!(
        session
            .accept_interim(current, "current".to_owned())
            .is_some()
    );
}

#[test]
fn miri_commit_reserves_capacity_promotes_ids_and_keeps_lineage() {
    let mut session = session();
    let mut previous = None;
    let mut committed = Vec::new();
    for _ in 0..MAX_COMMITTED_ITEMS_PER_SESSION {
        session
            .append_base64(&encoded(&vec![0; 2_400]))
            .expect("committable input appends");
        let provisional = session.input().expect("input exists").item_id().to_owned();
        let receipt = session.commit().expect("item commits within capacity");
        assert_eq!(receipt.item_id(), provisional);
        assert_eq!(receipt.previous_item_id(), previous.as_deref());
        previous = Some(provisional.clone());
        committed.push(provisional);
    }

    session
        .append_base64(&encoded(&vec![0; 2_400]))
        .expect("retry input appends");
    let retry_id = session
        .input()
        .expect("retry input exists")
        .item_id()
        .to_owned();
    assert!(matches!(
        session.commit(),
        Err(SessionError::CommittedItemsAtCapacity)
    ));
    assert_eq!(session.input().expect("input remains").item_id(), retry_id);

    session
        .finalize_completed(&committed[0], "done".to_owned())
        .expect("item finalizes");
    session.drain_results();
    assert_eq!(session.commit().expect("retry commits").item_id(), retry_id);
}

#[tokio::test]
async fn canceled_task_joins_accept_exact_capacity_and_reject_next() {
    assert_eq!(SESSION_CANCEL_JOIN_CAPACITY, 8);
    let mut session = session();
    for _ in 0..SESSION_CANCEL_JOIN_CAPACITY {
        session
            .append_base64(&encoded(&[0, 0]))
            .expect("input appends");
        session
            .spawn_interim(pending())
            .expect("task starts within join capacity");
        session.clear().expect("task is retained for joining");
    }
    assert_eq!(session.canceled_join_count(), SESSION_CANCEL_JOIN_CAPACITY);

    session
        .append_base64(&encoded(&[0, 0]))
        .expect("capacity-plus-one input appends");
    session
        .spawn_interim(pending())
        .expect("capacity-plus-one task starts");
    assert!(matches!(
        session.clear(),
        Err(SessionError::CancelJoinAtCapacity)
    ));
    assert!(
        session.input().is_some(),
        "recoverable error preserves input"
    );

    session.join_canceled().await.expect("canceled tasks join");
    session.clear().expect("retry succeeds after joins drain");
}

#[test]
fn clear_resets_partial_pcm_and_resampler_state() {
    let mut reused = session();
    reused
        .append_base64(&base64::engine::general_purpose::STANDARD.encode([0x7f]))
        .expect("odd byte appends");
    reused.clear().expect("partial input clears");
    reused
        .append_base64(&encoded(&vec![123; 2_400]))
        .expect("clean input appends");

    let mut fresh = session();
    fresh
        .append_base64(&encoded(&vec![123; 2_400]))
        .expect("fresh input appends");
    assert_eq!(
        reused
            .input()
            .expect("reused input")
            .take()
            .uncommitted_snapshot(usize::MAX),
        fresh
            .input()
            .expect("fresh input")
            .take()
            .uncommitted_snapshot(usize::MAX)
    );
}
