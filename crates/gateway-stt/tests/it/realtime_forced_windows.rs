use std::time::{Duration, Instant};

use base64::Engine as _;
use gateway_stt::test_fixtures::{
    HourSimulationProbe, RealtimeSessionFixture, RealtimeSessionRegistryFixture, ScriptedDecoder,
    ScriptedModelFactory, hour_marker_input,
};

const WAIT: Duration = Duration::from_secs(2);
const INPUT_SAMPLES_PER_STRIDE: usize = 24_000 * 10;
const FIRST_FORCED_SAMPLES: usize = 16_000 * 10;
const LATER_FORCED_SAMPLES: usize = 16_000 * 18;
const HOUR_STRIDES: usize = 360;
const OUTPUT_SAMPLES_PER_STRIDE: u64 = 16_000 * 10;

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

fn encoded_marker_samples(start: u64, samples: usize) -> String {
    let bytes = hour_marker_input(start, samples)
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn timeline_text(start_second: usize, end_second: usize) -> String {
    (start_second..end_second)
        .map(|second| format!("word{second:04}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[expect(
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

#[expect(
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

#[expect(
    clippy::expect_used,
    reason = "the bounded hour probe is observed off the async executor"
)]
async fn wait_for_hour_decodes(probe: &HourSimulationProbe, count: usize) -> bool {
    let observer = probe.clone();
    tokio::task::spawn_blocking(move || observer.wait_for_final_decodes(count, WAIT))
        .await
        .expect("the hour decode observer joins")
}

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

#[expect(
    clippy::expect_used,
    reason = "the bounded fixture must observe one exact settled coverage frontier"
)]
async fn wait_for_coverage(
    session: &RealtimeSessionFixture,
    expected_end: u64,
) -> (u64, std::ops::Range<u64>) {
    let deadline = Instant::now() + WAIT;
    loop {
        let metrics = session
            .take_metrics()
            .expect("the hour simulation retains one input");
        if let (finalized, Some(unresolved)) = metrics.coverage()
            && unresolved.end == expected_end
            && metrics.work_counts().0 == 0
        {
            return (finalized, unresolved);
        }
        assert!(
            Instant::now() < deadline,
            "forced final coverage reaches {expected_end}"
        );
        tokio::task::yield_now().await;
    }
}

#[derive(Default)]
struct HourPeaks {
    retained: usize,
    pending: usize,
    outcomes: usize,
    accepted: usize,
}

impl HourPeaks {
    #[expect(
        clippy::expect_used,
        reason = "the bounded hour fixture keeps one input and a fixed stride count"
    )]
    fn observe(&mut self, session: &RealtimeSessionFixture, stride: usize) {
        let metrics = session
            .take_metrics()
            .expect("the same logical take remains");
        assert_eq!(
            metrics.input_samples(),
            u64::try_from(stride + 1).expect("stride fits") * INPUT_SAMPLES_PER_STRIDE as u64
        );
        self.retained = self.retained.max(metrics.retained_samples());
        let (pending, outcomes, accepted) = metrics.work_counts();
        self.pending = self.pending.max(pending);
        self.outcomes = self.outcomes.max(outcomes);
        self.accepted = self.accepted.max(accepted);
    }

    fn assert_bounded(&self) {
        assert!(self.retained <= 16_000 * 30);
        assert!(self.pending <= 1);
        assert!(self.outcomes <= 1);
        assert!(self.accepted <= 2);
    }
}

fn append_marked_rotated(
    session: &mut RealtimeSessionFixture,
    chunk_sizes: &[usize],
    stride: usize,
    stride_offset: usize,
    message: &str,
) {
    let mut consumed = 0_usize;
    for offset in 0..chunk_sizes.len() {
        let samples = chunk_sizes[(stride + offset) % chunk_sizes.len()];
        let start = u64::try_from(stride * INPUT_SAMPLES_PER_STRIDE + stride_offset + consumed)
            .unwrap_or(u64::MAX);
        session
            .append_base64(&encoded_marker_samples(start, samples))
            .unwrap_or_else(|error| panic!("{message}: {error}"));
        consumed += samples;
    }
}

#[expect(
    clippy::expect_used,
    reason = "the complete decoded hypothesis has one deterministic fixture shape"
)]
fn assert_hour_hypothesis(hypothesis: &serde_json::Value, item_id: &str, stride: usize) {
    assert_eq!(hypothesis["item_id"], item_id);
    assert_eq!(hypothesis["revision"], stride + 1);
    assert_eq!(hypothesis["audio_start_ms"], stride * 10_000);
    assert_eq!(hypothesis["audio_end_ms"], stride * 10_000 + 4_000);
    let finalized_end = stride.checked_sub(2).map_or(0, |index| index * 10 + 2);
    assert_eq!(hypothesis["finalized"], timeline_text(0, finalized_end));
    let current_live = format!("live region {stride:04}");
    let transcript = hypothesis["transcript"]
        .as_str()
        .expect("a complete replacement is text");
    let expected_prefix = timeline_text(0, stride * 10);
    let expected_visible = if expected_prefix.is_empty() {
        current_live
    } else {
        format!("{expected_prefix} {current_live}")
    };
    assert_eq!(
        transcript, expected_visible,
        "no forced stride may shrink or omit its revisable middle"
    );
    assert_eq!(
        transcript,
        format!(
            "{}{}{}",
            hypothesis["finalized"].as_str().unwrap_or_default(),
            hypothesis["agreed"].as_str().unwrap_or_default(),
            hypothesis["tentative"].as_str().unwrap_or_default()
        )
    );
}

#[tokio::test]
async fn one_take_runs_for_an_hour_with_bounded_absolute_ownership() {
    let probe = HourSimulationProbe::new();
    let mut session = RealtimeSessionRegistryFixture::default()
        .register_with_hour_simulation(&probe)
        .expect("the deterministic hour session starts");
    session
        .update_text(
            &serde_json::json!({
                "type": "session.update",
                "session": {
                    "type": "transcription",
                    "include": ["item.input_audio_transcription.hypothesis"]
                }
            })
            .to_string(),
        )
        .expect("complete replacement hypotheses are negotiated");
    let leading = [1, 23_999, 72_000];
    let trailing = [17, 47_983, 96_000];
    let mut provisional = None;
    let mut peaks = HourPeaks::default();

    for stride in 0..HOUR_STRIDES {
        append_marked_rotated(
            &mut session,
            &leading,
            stride,
            0,
            "varied leading audio appends",
        );
        let item_id = session
            .input_snapshot()
            .expect("one provisional input exists")
            .item_id()
            .to_owned();
        match &provisional {
            Some(expected) => assert_eq!(&item_id, expected),
            None => provisional = Some(item_id.clone()),
        }

        let hypothesis = session
            .run_interim()
            .await
            .expect("the production interim scheduler completes")
            .expect("four seconds of speech emits one complete hypothesis");
        assert_hour_hypothesis(&hypothesis, &item_id, stride);
        append_marked_rotated(
            &mut session,
            &trailing,
            stride,
            96_000,
            "varied trailing audio appends",
        );
        assert!(
            wait_for_hour_decodes(&probe, stride + 1).await,
            "forced decode {stride} completes without wall-clock capture: {:?}",
            session.take_metrics()
        );
        let expected_end =
            u64::try_from(stride + 1).expect("stride fits") * OUTPUT_SAMPLES_PER_STRIDE;
        let (finalized, unresolved) = wait_for_coverage(&session, expected_end).await;
        assert_eq!(unresolved.start, finalized);
        assert_eq!(unresolved.end, expected_end);
        peaks.observe(&session, stride);
    }

    assert_eq!(probe.final_decode_count(), HOUR_STRIDES);
    assert_eq!(probe.interim_decode_count(), HOUR_STRIDES);
    assert_eq!(probe.gap_free_coverage_samples(), 16_000_u64 * 3_600);
    assert!(probe.max_final_samples() <= LATER_FORCED_SAMPLES);
    peaks.assert_bounded();
    assert_eq!(
        session
            .take_metrics()
            .expect("the one input remains before commit")
            .input_samples(),
        24_000_u64 * 3_600
    );

    let provisional = provisional.expect("the hour owns one provisional item");
    let committed = session.commit().expect("the hour commits once");
    assert_eq!(committed.item_id(), provisional);
    assert_eq!(session.committed_count(), 1);
    session
        .finish_finalization(committed.item_id())
        .await
        .expect("the one final pipeline completes");

    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "completed");
    assert_eq!(results[0]["item_id"], provisional);
    assert_eq!(results[0]["seconds"], 3_600.0);
    assert_eq!(results[0]["transcript"], timeline_text(0, 3_600));
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
async fn five_unaligned_forced_overlaps_remain_one_healthy_item() {
    let final_decoder = ScriptedDecoder::new();
    let windows = (0..6)
        .map(|window| {
            (0..if window == 0 { 10 } else { 18 })
                .map(|token| format!("w{window}t{token}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>();
    for window in &windows {
        final_decoder.push_text(window);
    }
    let mut session = scripted_session(&final_decoder);
    let payload = encoded_speech();

    session
        .append_base64(&payload)
        .expect("the first continuous stride appends");
    wait_for_decodes(&final_decoder, 1).await;
    let provisional = session
        .input_snapshot()
        .expect("the live take remains one input")
        .item_id()
        .to_owned();
    for count in 2..=6 {
        append_after_retirement(&mut session, &payload).await;
        wait_for_decodes(&final_decoder, count).await;
        assert_eq!(
            session
                .input_snapshot()
                .expect("the recording accepts another stride")
                .item_id(),
            provisional
        );
        assert!(session.pending_failure().is_none());
    }

    let committed = session.commit().expect("the healthy take commits");
    assert_eq!(committed.item_id(), provisional);
    assert_eq!(session.committed_count(), 1);
    session
        .finish_finalization(committed.item_id())
        .await
        .expect("the pending current window flushes once");
    let results = session.drain_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "completed");
    assert_eq!(results[0]["item_id"], provisional);

    let mut expected = windows[0]
        .split_whitespace()
        .take(2)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for window in &windows[1..5] {
        expected.extend(window.split_whitespace().take(10).map(str::to_owned));
    }
    expected.extend(windows[5].split_whitespace().map(str::to_owned));
    assert_eq!(
        results[0]["transcript"],
        expected.join(" "),
        "estimated ownership is ordered, gap-free, and contains no whole duplicate window"
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
                assert_eq!(
                    session
                        .append_base64(&encoded_speech())
                        .expect_err("capture faster than decoding reaches retained ownership"),
                    "audio buffer exceeds 30 seconds"
                );
                assert_eq!(
                    session
                        .input_snapshot()
                        .expect("bounded overload preserves the valid input")
                        .item_id(),
                    provisional
                );
                assert!(
                    session.pending_failure().is_none(),
                    "retained-budget overload remains commit-recoverable"
                );
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
    final_decoder.push_text("alpha trusted OVERLAP");
    final_decoder.push_text("trusted overlap terminal");
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
        "alpha trusted overlap terminal"
    );
}

#[tokio::test]
async fn first_natural_boundary_after_a_cut_reconciles_the_forced_predecessor() {
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("alpha trusted OVERLAP");
    final_decoder.push_text("trusted overlap natural");
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
        "alpha trusted overlap natural"
    );
}
