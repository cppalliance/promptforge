//! Silero detector loading, once at the generation's load and again per
//! take, and a take that fails when its detector does.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gateway_stt_engine::test_fixtures::ScriptedDetector;

use super::{Take, TakeFailure};
use crate::SpeechError;
use crate::generation::GenerationState;
use crate::segment::FRAME_SAMPLES;
use crate::test_fixtures::{
    SCRIPTED_SILERO_MODEL, ScriptedSilero, Warnings, scripted_silero_generation,
};

const FRAME: usize = FRAME_SAMPLES;
const LOUD: f32 = 0.05;

/// Asserts that `warnings` holds exactly one warning, naming `cause`.
fn assert_one_warning_naming(warnings: &[String], cause: &str) {
    assert_eq!(warnings.len(), 1, "one warning: {warnings:?}");
    assert!(
        warnings[0].contains(cause),
        "the warning names the cause: {warnings:?}"
    );
}

/// A take on a generation whose takes load Silero through `source`, and
/// the generation to shut down.
fn silero_take(source: Arc<ScriptedSilero>) -> (Take, GenerationState) {
    let (state, lease) = scripted_silero_generation(source);
    let take = Take::new(Vec::new(), lease).expect("the take's detector opens");
    (take, state)
}

fn frames(range: Range<usize>) -> Range<u64> {
    (range.start * FRAME) as u64..(range.end * FRAME) as u64
}

#[test]
fn a_generation_whose_silero_detector_does_not_open_fails_its_load_and_leaves_speech_unavailable() {
    let source = ScriptedSilero::failing_from(0, "scripted load failure");
    let state = GenerationState::default();

    let error = state
        .publish_scripted_silero(PathBuf::from(SCRIPTED_SILERO_MODEL), source.clone())
        .expect_err("a Silero detector that does not open fails the load");

    let SpeechError::SileroDetector(cause) = &error else {
        panic!("expected a Silero detector failure, got {error:?}");
    };
    assert!(
        cause.to_string().contains("scripted load failure"),
        "the failure names the cause: {cause}"
    );
    assert_eq!(
        source.loaded(),
        [PathBuf::from(SCRIPTED_SILERO_MODEL)],
        "the load opens the verified model"
    );
    assert!(state.active().is_none(), "speech stays unavailable");
    assert!(!state.status().ready());
}

#[test]
fn a_take_classifies_with_the_silero_detector_its_generation_loads() {
    let detector = ScriptedDetector::new([(0, 2 * FRAME)]);
    let source = ScriptedSilero::new(detector.clone());
    let (take, state) = silero_take(Arc::clone(&source));

    take.append(vec![LOUD; 4 * FRAME]).expect("audio appends");

    assert_eq!(
        source.loaded(),
        [
            PathBuf::from(SCRIPTED_SILERO_MODEL),
            PathBuf::from(SCRIPTED_SILERO_MODEL)
        ],
        "the generation's load proves the model, then the take opens its own"
    );
    assert_eq!(
        take.speech_runs(),
        [frames(0..2)],
        "Silero hears speech only where its script says, however loud the audio"
    );
    assert_eq!(detector.chunk_starts(), [0, FRAME, 2 * FRAME, 3 * FRAME]);
    assert!(take.pending_failure().is_none(), "nothing failed");
    drop(take);
    state.shutdown();
}

#[test]
fn a_take_whose_detector_does_not_open_does_not_start() {
    let source = ScriptedSilero::failing_from(1, "scripted load failure");
    let (state, lease) = scripted_silero_generation(Arc::clone(&source));
    let warnings = Warnings::default();

    let error =
        tracing::subscriber::with_default(warnings.clone(), || Take::new(Vec::new(), lease))
            .expect_err("the take does not start");

    assert!(
        error.to_string().contains("scripted load failure"),
        "the error names the cause: {error}"
    );
    assert_one_warning_naming(&warnings.take(), "scripted load failure");
    assert_eq!(
        source.loaded().len(),
        2,
        "the generation's load proved the model, then the take tried its own"
    );
    state.shutdown();
}

#[test]
fn a_mid_take_detector_error_fails_the_take_keeps_finalized_text_and_classifies_nothing_after() {
    let detector = ScriptedDetector::new([(0, 2 * FRAME)]).with_failure_at(2);
    let (take, state) = silero_take(ScriptedSilero::new(detector.clone()));
    let warnings = Warnings::default();
    let append = |samples: Vec<f32>| {
        tracing::subscriber::with_default(warnings.clone(), || take.append(samples))
    };
    append(vec![LOUD; 2 * FRAME]).expect("audio appends");
    take.record_finalized_through("Ask not.", Some(2 * FRAME as u64));
    assert!(take.pending_failure().is_none(), "nothing has failed yet");
    assert!(warnings.take().is_empty(), "nothing is logged yet");

    append(vec![LOUD; 4 * FRAME]).expect("the append the detector fails on still appends");
    append(vec![LOUD; 2 * FRAME]).expect("later audio appends");

    assert_one_warning_naming(&warnings.take(), "scripted detector failure at chunk 2");
    let failure = take
        .pending_failure()
        .expect("the detector error fails the take");
    assert!(
        matches!(failure.as_ref(), TakeFailure::Detector(_)),
        "{failure:?}"
    );
    assert!(
        failure
            .to_string()
            .contains("scripted detector failure at chunk 2"),
        "the failure names the cause: {failure}"
    );
    assert_eq!(
        take.finalized(),
        "Ask not.",
        "text finalized before the error stays"
    );
    assert_eq!(
        detector.chunk_starts(),
        [0, FRAME, 2 * FRAME],
        "the detector is never asked again once it fails"
    );
    assert_eq!(
        take.speech_runs(),
        [frames(0..2)],
        "no chunk after the error is classified by loudness, however loud"
    );
    drop(take);
    state.shutdown();
}
