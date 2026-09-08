use std::time::{Duration, Instant};

use base64::Engine as _;
use gateway_stt::test_fixtures::{
    RealtimeSessionFixture, RealtimeSessionRegistryFixture, ScriptedDecoder, ScriptedModelFactory,
};

const WAIT: Duration = Duration::from_secs(2);
const INPUT_SAMPLES_PER_STRIDE: usize = 24_000 * 10;
const FIRST_FORCED_SAMPLES: usize = 16_000 * 10;
const LATER_FORCED_SAMPLES: usize = 16_000 * 18;

fn encoded_samples(value: i16, samples: usize) -> String {
    let bytes = vec![value; samples]
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn encoded_speech_samples(samples: usize) -> String {
    encoded_samples(8_192, samples)
}

fn encoded_speech() -> String {
    encoded_speech_samples(INPUT_SAMPLES_PER_STRIDE)
}

#[allow(
    clippy::expect_used,
    reason = "the fixture creates one deterministic session and generation"
)]
fn scripted_session(final_decoder: &ScriptedDecoder) -> RealtimeSessionFixture {
    RealtimeSessionRegistryFixture::default()
        .register_with_scripted_engine(
            ScriptedModelFactory::new(ScriptedDecoder::new()).with_final(final_decoder.clone()),
        )
        .expect("the scripted Realtime session starts")
}

#[allow(
    clippy::expect_used,
    reason = "the bounded decoder observation is a deterministic fixture assertion"
)]
async fn wait_for_decodes(decoder: &ScriptedDecoder, count: usize) {
    let observer = decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || observer.wait_for_completed(count, WAIT))
            .await
            .expect("the decode observer joins"),
        "the requested final decodes complete"
    );
}

#[allow(
    clippy::expect_used,
    reason = "the retry waits only for worker-owned PCM retirement"
)]
async fn append_after_retirement(session: &mut RealtimeSessionFixture, payload: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        match session.append_base64(payload) {
            Ok(()) => return,
            Err(error) if error.contains("audio buffer exceeds") && Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            Err(error) => panic!("continuous append must succeed after retirement: {error}"),
        }
    }
}

#[tokio::test]
async fn one_item_reconciles_bounded_forced_windows() {
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("alpha beta ECHO, now");
    final_decoder.push_text("echo now revised ending");
    final_decoder.push_text("revised ending final words");
    let mut session = scripted_session(&final_decoder);
    let payload = encoded_speech();

    session
        .append_base64(&payload)
        .expect("the first continuous stride appends");
    let provisional = session
        .input_snapshot()
        .expect("one provisional input exists")
        .item_id()
        .to_owned();
    wait_for_decodes(&final_decoder, 1).await;
    append_after_retirement(&mut session, &payload).await;
    assert_eq!(
        session
            .input_snapshot()
            .expect("the same input remains")
            .item_id(),
        provisional
    );
    wait_for_decodes(&final_decoder, 2).await;
    append_after_retirement(&mut session, &payload).await;
    wait_for_decodes(&final_decoder, 3).await;

    let committed = session.commit().expect("the one input commits");
    assert_eq!(committed.item_id(), provisional);
    assert_eq!(session.committed_count(), 1);
    session
        .finish_finalization(committed.item_id())
        .await
        .expect("forced windows complete through the existing item");
    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "completed");
    assert_eq!(results[0]["item_id"], provisional);
    assert_eq!(
        results[0]["transcript"],
        "alpha beta echo now revised ending final words"
    );
    assert_eq!(results[0]["seconds"], 30.0);

    let requests = final_decoder.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].samples().len(), FIRST_FORCED_SAMPLES);
    assert_eq!(requests[1].samples().len(), LATER_FORCED_SAMPLES);
    assert_eq!(requests[2].samples().len(), LATER_FORCED_SAMPLES);
    assert_eq!(requests[0].finalized(), "");
    assert_eq!(requests[1].finalized(), "");
    assert_eq!(requests[2].finalized(), "alpha beta");
}

#[tokio::test]
async fn unaligned_forced_overlap_becomes_one_item_failure() {
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("old overlap");
    final_decoder.push_text("unrelated revision");
    let mut session = scripted_session(&final_decoder);
    let payload = encoded_speech();

    session
        .append_base64(&payload)
        .expect("the first continuous stride appends");
    wait_for_decodes(&final_decoder, 1).await;
    append_after_retirement(&mut session, &payload).await;
    wait_for_decodes(&final_decoder, 2).await;

    let provisional = session
        .input_snapshot()
        .expect("the failed take remains one input")
        .item_id()
        .to_owned();
    let deadline = Instant::now() + WAIT;
    while session.pending_failure().is_none() && Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        session.pending_failure().as_deref(),
        Some("forced final overlap could not be aligned")
    );

    let committed = session
        .commit()
        .expect("the failed take still promotes its sole item");
    assert_eq!(committed.item_id(), provisional);
    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "failed");
    assert_eq!(results[0]["item_id"], provisional);
    assert_eq!(
        results[0]["message"],
        "forced final overlap could not be aligned"
    );
}

#[tokio::test]
async fn blocked_forced_decode_enforces_the_thirty_second_aggregate_budget() {
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("first overlap");
    final_decoder.push_text("overlap second");
    let mut session = scripted_session(&final_decoder);
    session
        .append_base64(&encoded_speech())
        .expect("the first forced stride appends");
    wait_for_decodes(&final_decoder, 1).await;
    let provisional = final_decoder
        .with_next_decode_blocked(
            WAIT,
            || async {
                session
                    .append_base64(&encoded_speech())
                    .expect("the later overlapping stride appends");
                let provisional = session
                    .input_snapshot()
                    .expect("the one provisional input exists")
                    .item_id()
                    .to_owned();
                (&mut session, provisional)
            },
            |(session, provisional)| async {
                session
                    .append_base64(&encoded_speech())
                    .expect("an 18-second decode leaves room for the next ten-second stride");
                assert_eq!(
                    session
                        .input_snapshot()
                        .expect("bounded overload retains the same input")
                        .item_id(),
                    provisional
                );
                assert_eq!(session.pending_final_segments(), Some(2));
                provisional
            },
        )
        .await
        .expect("the forced decode reaches the bounded scenario");

    assert_eq!(
        session
            .input_snapshot()
            .expect("the one input survives worker retirement")
            .item_id(),
        provisional
    );
}

#[tokio::test]
async fn stop_before_the_first_cut_decodes_only_the_terminal_tail() {
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("before cut");
    let mut session = scripted_session(&final_decoder);
    session
        .append_base64(&encoded_speech_samples(24_000 * 9))
        .expect("nine seconds remain before the forced cut");

    let committed = session.commit().expect("the take stops before the cut");
    session
        .finish_finalization(committed.item_id())
        .await
        .expect("the terminal tail completes");

    assert_eq!(final_decoder.requests().len(), 1);
    assert_eq!(final_decoder.requests()[0].samples().len(), 16_000 * 9);
    assert_eq!(session.drain_results()[0]["transcript"], "before cut");
}

#[tokio::test]
async fn stop_exactly_on_the_cut_does_not_decode_the_overlap_twice() {
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("exact cut");
    let mut session = scripted_session(&final_decoder);
    session
        .append_base64(&encoded_speech())
        .expect("ten seconds reach the forced cut");

    let committed = session.commit().expect("the take stops on the cut");
    session
        .finish_finalization(committed.item_id())
        .await
        .expect("the cut completes");

    assert_eq!(final_decoder.requests().len(), 1);
    assert_eq!(session.drain_results()[0]["transcript"], "exact cut");
}

#[tokio::test]
async fn stop_after_the_cut_reconciles_the_overlapping_terminal_tail() {
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("alpha OVERLAP");
    final_decoder.push_text("overlap terminal");
    let mut session = scripted_session(&final_decoder);
    session
        .append_base64(&encoded_speech())
        .expect("ten seconds reach the forced cut");
    session
        .append_base64(&encoded_speech_samples(24_000 * 2))
        .expect("two seconds follow the forced cut");

    let committed = session.commit().expect("the take stops after the cut");
    session
        .finish_finalization(committed.item_id())
        .await
        .expect("the overlapping terminal tail completes");

    let requests = final_decoder.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].samples().len(), 16_000 * 10);
    assert_eq!(
        session.drain_results()[0]["transcript"],
        "alpha overlap terminal"
    );
}

#[tokio::test]
async fn first_natural_boundary_after_a_cut_reconciles_the_forced_predecessor() {
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("alpha OVERLAP");
    final_decoder.push_text("overlap natural");
    let mut session = scripted_session(&final_decoder);
    session
        .append_base64(&encoded_speech())
        .expect("ten seconds reach the forced cut");
    session
        .append_base64(&encoded_speech_samples(24_000))
        .expect("one speech second follows the cut");
    session
        .append_base64(&encoded_samples(0, 24_000 * 3))
        .expect("three silence seconds close a natural boundary");
    wait_for_decodes(&final_decoder, 2).await;

    let committed = session
        .commit()
        .expect("the take stops after the natural boundary");
    session
        .finish_finalization(committed.item_id())
        .await
        .expect("the natural successor completes");

    let requests = final_decoder.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].samples().len() > 16_000 * 8 && requests[1].samples().len() < 16_000 * 10);
    assert_eq!(
        session.drain_results()[0]["transcript"],
        "alpha overlap natural"
    );
}
