//! An hour-long take with bounded forced-window ownership.

use std::time::Instant;

use base64::Engine as _;
use gateway_stt::test_fixtures::{
    HourSimulationProbe, RealtimeSessionFixture, RealtimeSessionRegistryFixture, hour_marker_input,
};

use super::{INPUT_SAMPLES_PER_STRIDE, LATER_FORCED_SAMPLES, WAIT};

const HOUR_STRIDES: usize = 360;
const OUTPUT_SAMPLES_PER_STRIDE: u64 = 16_000 * 10;

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
    reason = "the bounded hour probe is observed off the async executor"
)]
async fn wait_for_hour_decodes(probe: &HourSimulationProbe, count: usize) -> bool {
    let observer = probe.clone();
    tokio::task::spawn_blocking(move || observer.wait_for_final_decodes(count, WAIT))
        .await
        .expect("the hour decode observer joins")
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
        assert_eq!(
            session.pending_failure(),
            None,
            "stride {stride} keeps the take healthy"
        );
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
