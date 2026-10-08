//! Tests for final decodes gated on the speech the take's detector heard:
//! a commit over loud audio without detected speech decodes nothing,
//! detected speech still decodes, and a failed detector fails the commit.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use gateway_stt_engine::test_fixtures::ScriptedDetector;
use gateway_stt_engine::{DecodeRequest, EnginePolicy};
use tokio::sync::mpsc;

use super::{FINAL_SEGMENT_CAPACITY, FinalPipeline, run_final_pipeline};
use crate::take::{Take, TakeFailure};

const SECOND: usize = EnginePolicy::SAMPLE_RATE;
const LOUD: f32 = 0.5;

/// A take that hears speech through `detector` and whose final decodes
/// return `text`, reporting each request's sample count.
fn take_hearing(
    detector: ScriptedDetector,
    text: &'static str,
) -> (Take, mpsc::UnboundedReceiver<usize>) {
    let (commands, receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let mut take = Take::with_detector(Vec::new(), None, Box::new(detector));
    let (report, requests) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        Arc::from([]),
        Arc::clone(&take.state),
        Arc::clone(&take.whole_window),
        move |request: DecodeRequest| {
            let _ = report.send(request.samples().len());
            async move { Some(Ok(text.to_owned())) }
        },
    ));
    take.final_pipeline = Some(FinalPipeline {
        commands,
        task,
        pending_segments: Arc::new(AtomicUsize::new(0)),
    });
    (take, requests)
}

/// Appends three seconds of loud audio and commits the take.
async fn commit_loud_take(take: &Take) -> String {
    take.append(vec![LOUD; 3 * SECOND])
        .expect("the audio appends");
    take.submit_closed_segments();
    take.finalization()
        .expect("the take owns a final pipeline")
        .await
        .expect("completion succeeds")
}

fn decodes(requests: &mut mpsc::UnboundedReceiver<usize>) -> usize {
    std::iter::from_fn(|| requests.try_recv().ok()).count()
}

#[tokio::test]
async fn a_commit_over_a_take_its_detector_heard_no_speech_in_decodes_nothing() {
    let (take, mut requests) = take_hearing(ScriptedDetector::new([]), "Okay.");

    assert_eq!(
        commit_loud_take(&take).await,
        "",
        "the take completes without final text however loud its audio"
    );
    assert_eq!(decodes(&mut requests), 0, "the final model hears nothing");
}

#[tokio::test]
async fn a_commit_over_a_scripted_speech_run_still_decodes_it() {
    let (take, mut requests) =
        take_hearing(ScriptedDetector::new([(SECOND, 2 * SECOND)]), "Ask not.");

    assert_eq!(commit_loud_take(&take).await, "Ask not.");
    assert_eq!(decodes(&mut requests), 1);
}

#[tokio::test]
async fn a_commit_over_a_take_whose_detector_failed_fails_and_decodes_nothing() {
    let detector = ScriptedDetector::new([]).with_failure_at(2);
    let (take, mut requests) = take_hearing(detector, "Ask not.");
    take.append(vec![LOUD; 3 * SECOND])
        .expect("the audio appends");
    take.submit_closed_segments();

    let failure = take
        .finalization()
        .expect("the take owns a final pipeline")
        .await
        .expect_err("the detector failure fails the commit");

    assert!(
        matches!(failure.as_ref(), TakeFailure::Detector(_)),
        "{failure:?}"
    );
    assert_eq!(
        decodes(&mut requests),
        0,
        "loudness never stands in for the failed detector"
    );
}
