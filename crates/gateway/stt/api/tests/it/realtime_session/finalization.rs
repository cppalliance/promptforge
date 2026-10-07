//! Accurate finalization ownership, retry, and asynchronous final failures.

use std::sync::atomic::Ordering;
use std::time::Duration;

use futures_util::FutureExt as _;
use gateway_stt::test_fixtures::{
    RealtimeSessionRegistryFixture, ScriptedDecoder, ScriptedModelFactory,
};

use super::{
    COMMITTED_ITEM_CAPACITY, append_committable, append_decodable, closed_segment, encoded,
    session, source_message, update, wait_until,
};

#[tokio::test]
async fn four_items_finalize_in_reverse_order_without_crossing_ownership() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
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
    final_decoder
        .with_next_decode_blocked(
            Duration::from_secs(1),
            || async {
                for index in 0..COMMITTED_ITEM_CAPACITY {
                    session
                        .update_text(&update(&format!("prompt-{index}"), true))
                        .expect("item prompt updates");
                    append_decodable(&mut session);
                    ids.push(session.commit().expect("item commits").item_id().to_owned());
                }
                &session
            },
            |session| async {
                assert_eq!(
                    session.finalizing_count(),
                    COMMITTED_ITEM_CAPACITY,
                    "every committed take owns an asynchronous finalization"
                );
            },
        )
        .await
        .expect("one accurate final decode blocks while all item tasks remain owned");

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
async fn realtime_interim_and_final_decodes_carry_a_cancellation_flag() {
    let interim = ScriptedDecoder::new();
    interim.push_text("provisional");
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("authoritative");
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry
        .register_with_scripted_engine(
            ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone()),
        )
        .expect("scripted session starts");
    session
        .append_base64(&encoded(&vec![16_384; 24_000]))
        .expect("one second of speech appends");
    session
        .run_interim()
        .await
        .expect("the production interim decode completes");
    let item_id = session.commit().expect("item commits").item_id().to_owned();
    session
        .finish_finalization(&item_id)
        .await
        .expect("the committed item finalizes");

    let interim_requests = interim.requests();
    let final_requests = final_decoder.requests();
    assert_eq!(interim_requests.len(), 1, "one interim decode ran");
    assert!(!final_requests.is_empty(), "the committed item decoded");
    for request in interim_requests.iter().chain(&final_requests) {
        let flag = request
            .cancellation()
            .expect("every Realtime decode carries the epoch's flag");
        assert!(
            !flag.load(Ordering::Acquire),
            "an open epoch's flag is unset"
        );
    }
}

#[tokio::test]
async fn canceling_item_finish_keeps_finalization_owned_for_retry() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("authoritative");
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry
        .register_with_scripted_engine(
            ScriptedModelFactory::new(interim).with_final(final_decoder.clone()),
        )
        .expect("scripted session starts");
    let item_id = final_decoder
        .with_next_decode_blocked(
            Duration::from_secs(1),
            || async {
                append_decodable(&mut session);
                let item_id = session.commit().expect("item commits").item_id().to_owned();
                (&mut session, item_id)
            },
            |(session, item_id)| async {
                assert!(
                    session
                        .finish_finalization(&item_id)
                        .now_or_never()
                        .is_none(),
                    "canceling the first join poll cannot detach finalization"
                );
                assert_eq!(session.finalizing_count(), 1);
                item_id
            },
        )
        .await
        .expect("the accurate decode reaches the blocked scenario");
    session
        .finish_finalization(&item_id)
        .await
        .expect("retry joins the same finalization");

    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "completed");
    assert_eq!(results[0]["transcript"], "authoritative");
}

#[tokio::test]
async fn fixture_finalization_errors_report_their_operation_and_source() {
    let mut session = session();
    let error = session
        .finish_finalization("missing")
        .await
        .expect_err("an unknown item is rejected");
    assert_eq!(error.to_string(), "finish fixture finalization");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the committed item is not active")
    );

    let item_id = append_committable(&mut session);
    session.commit().expect("item commits");
    let error = session
        .finish_finalization(&item_id)
        .await
        .expect_err("an item without finalization work is rejected");
    assert_eq!(error.to_string(), "finish fixture finalization");
    assert_eq!(
        source_message(&error).as_deref(),
        Some("the committed item has no active finalization")
    );
}

#[tokio::test]
async fn asynchronous_final_failure_is_observed_before_the_next_append_and_at_commit() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_error("late accurate failure");
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry
        .register_with_scripted_engine(
            ScriptedModelFactory::new(interim).with_final(final_decoder.clone()),
        )
        .expect("scripted session starts");

    final_decoder
        .with_next_decode_blocked(
            Duration::from_secs(1),
            || async {
                session
                    .append_base64(&closed_segment())
                    .expect("closed segment enters the production take");
            },
            |()| async {},
        )
        .await
        .expect("the accurate segment blocks between appends");
    wait_until(|| session.pending_failure().is_some()).await;
    let failure = session
        .pending_failure()
        .expect("the take owns the asynchronous failure");

    let error = session
        .append_base64(&encoded(&[1, 2]))
        .expect_err("the next append is rejected before mutating audio");
    assert_eq!(error.to_string(), "append fixture audio");
    assert_eq!(source_message(&error).as_deref(), Some(failure.as_str()));
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
async fn production_final_segment_saturation_holds_the_next_segment_until_the_queue_has_room() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    for index in 1..=5 {
        final_decoder.push_text(format!("segment-{index}"));
    }
    let registry = RealtimeSessionRegistryFixture::default();
    let mut session = registry
        .register_with_scripted_engine(
            ScriptedModelFactory::new(interim).with_final(final_decoder.clone()),
        )
        .expect("scripted session starts");

    let item_id = final_decoder
        .with_next_decode_blocked(
            Duration::from_secs(1),
            || async {
                session
                    .append_base64(&closed_segment())
                    .expect("first closed segment is admitted");
                &mut session
            },
            |session| async {
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
                    .expect("a segment past capacity is held rather than rejected");
                assert_eq!(session.pending_final_segments(), Some(4));
                assert!(
                    session.pending_failure().is_none(),
                    "a full final queue does not fail the take"
                );
                let item_id = session
                    .commit()
                    .expect("the saturated take commits")
                    .item_id()
                    .to_owned();
                assert_eq!(session.finalizing_count(), 1);
                item_id
            },
        )
        .await
        .expect("the first production segment blocks in final decoding");

    session
        .finish_finalization(&item_id)
        .await
        .expect("the held segment decodes once the queue has room");
    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["item_id"], item_id.as_str());
    assert_eq!(results[0]["type"], "completed");
    assert_eq!(
        results[0]["transcript"],
        "segment-1 segment-2 segment-3 segment-4 segment-5"
    );
}
