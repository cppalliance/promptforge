//! Per-take Silero detector selection and its fall back to loudness.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gateway_progress::ProgressHub;
use gateway_stt_engine::test_fixtures::ScriptedDetector;
use gateway_stt_engine::{DetectorError, SpeechDetector};

use super::Take;
use crate::generation::{GenerationState, SileroSource};
use crate::segment::FRAME_SAMPLES;
use crate::test_fixtures::Warnings;

const FRAME: usize = FRAME_SAMPLES;
const LOUD: f32 = 0.05;
const MODEL: &str = "ggml-silero.bin";

/// Loads `detector`, or fails with the scripted message, recording each
/// model path it is asked to load.
#[derive(Debug)]
struct ScriptedSilero {
    detector: Result<ScriptedDetector, String>,
    loaded: Mutex<Vec<PathBuf>>,
}

impl ScriptedSilero {
    fn new(detector: Result<ScriptedDetector, String>) -> Arc<Self> {
        Arc::new(Self {
            detector,
            loaded: Mutex::default(),
        })
    }

    fn loaded(&self) -> Vec<PathBuf> {
        self.loaded.lock().expect("load record lock").clone()
    }
}

impl SileroSource for ScriptedSilero {
    fn load(&self, model: &Path) -> Result<Box<dyn SpeechDetector>, DetectorError> {
        self.loaded
            .lock()
            .expect("load record lock")
            .push(model.to_path_buf());
        match &self.detector {
            Ok(detector) => Ok(Box::new(detector.clone())),
            Err(message) => Err(DetectorError::load(message.clone())),
        }
    }
}

/// A take on a generation whose takes load Silero through `source`, the
/// generation to shut down, every warning the take's start logged, and the
/// hub the take reports progress into.
fn silero_take(source: Arc<ScriptedSilero>) -> (Take, GenerationState, Warnings, Arc<ProgressHub>) {
    let hub = Arc::new(ProgressHub::new());
    let state = GenerationState::default();
    state.publish_scripted_silero(Ok(PathBuf::from(MODEL)), source, Some(Arc::clone(&hub)));
    let lease = state.active().expect("the published runtime admits");
    let warnings = Warnings::default();
    let take =
        tracing::subscriber::with_default(warnings.clone(), || Take::new(Vec::new(), Some(lease)));
    (take, state, warnings, hub)
}

/// Asserts `hub` shows a live activity naming `cause`.
fn assert_progress_names(hub: &ProgressHub, cause: &str) {
    let progress = hub.current();
    assert!(progress.busy, "the report is live while the take is");
    assert!(
        progress.text.contains("loudness") && progress.text.contains(cause),
        "the progress text names the fall back and its cause: {:?}",
        progress.text
    );
}

/// Appends `blocks` in turn while recording warnings.
fn append(take: &Take, warnings: &Warnings, blocks: &[Vec<f32>]) {
    tracing::subscriber::with_default(warnings.clone(), || {
        for block in blocks {
            take.append(block.clone()).expect("audio appends");
        }
    });
}

fn frames(range: Range<usize>) -> Range<u64> {
    (range.start * FRAME) as u64..(range.end * FRAME) as u64
}

#[test]
fn a_take_classifies_with_the_silero_detector_its_generation_loads() {
    let detector = ScriptedDetector::new([(0, 2 * FRAME)]);
    let source = ScriptedSilero::new(Ok(detector.clone()));
    let (take, state, warnings, hub) = silero_take(Arc::clone(&source));

    append(&take, &warnings, &[vec![LOUD; 4 * FRAME]]);

    assert_eq!(source.loaded(), [PathBuf::from(MODEL)]);
    assert_eq!(
        take.speech_runs(),
        [frames(0..2)],
        "Silero hears speech only where its script says, however loud the audio"
    );
    assert_eq!(detector.chunk_starts(), [0, FRAME, 2 * FRAME, 3 * FRAME]);
    assert!(warnings.take().is_empty(), "nothing failed");
    assert!(!hub.current().busy, "nothing is reported through progress");
    drop(take);
    state.shutdown();
}

#[test]
fn a_silero_load_failure_falls_back_to_loudness_and_reports_once() {
    let source = ScriptedSilero::new(Err("scripted load failure".to_owned()));
    let (take, state, warnings, hub) = silero_take(source);
    let started = warnings.take();
    assert_progress_names(&hub, "scripted load failure");

    append(
        &take,
        &warnings,
        &[
            vec![LOUD; 2 * FRAME],
            vec![0.0; 2 * FRAME],
            vec![LOUD; FRAME],
        ],
    );

    assert_eq!(
        started.len(),
        1,
        "the take reports its load failure: {started:?}"
    );
    assert!(
        started[0].contains("scripted load failure"),
        "the report names the cause: {started:?}"
    );
    assert_eq!(
        take.speech_runs(),
        [frames(0..2), frames(4..5)],
        "loudness decides every chunk in order"
    );
    assert!(warnings.take().is_empty(), "appends report nothing more");
    assert_progress_names(&hub, "scripted load failure");
    drop(take);
    assert!(!hub.current().busy, "the report ends with the take");
    state.shutdown();
}

#[test]
fn a_mid_take_inference_error_falls_back_to_loudness_reports_once_and_keeps_every_chunk_in_order() {
    let detector = ScriptedDetector::new([(0, 100 * FRAME)]).with_failure_at(2);
    let (take, state, warnings, hub) = silero_take(ScriptedSilero::new(Ok(detector.clone())));

    let mut audio = vec![0.0; 2 * FRAME];
    audio.extend(vec![LOUD; FRAME]);
    audio.extend(vec![0.0; FRAME]);
    audio.extend(vec![LOUD; FRAME]);
    audio.extend(vec![0.0; 3 * FRAME]);
    let (first, rest) = audio.split_at(FRAME + 100);
    let (second, third) = rest.split_at(3 * FRAME);
    append(&take, &warnings, &[first.to_vec()]);
    assert!(!hub.current().busy, "nothing has failed yet");
    append(&take, &warnings, &[second.to_vec()]);
    let reported = warnings.take();
    assert_progress_names(&hub, "scripted detector failure at chunk 2");
    append(&take, &warnings, &[third.to_vec()]);

    assert_eq!(
        detector.chunk_starts(),
        [0, FRAME, 2 * FRAME],
        "Silero decides chunks in order through the one it fails on, and never again"
    );
    assert_eq!(
        take.speech_runs(),
        [frames(0..3), frames(4..5)],
        "Silero's two chunks, then loudness for the failed chunk and every later one"
    );
    assert_eq!(
        reported.len(),
        1,
        "the failure is reported once: {reported:?}"
    );
    assert!(
        reported[0].contains("scripted detector failure at chunk 2"),
        "the report names the cause: {reported:?}"
    );
    assert!(
        warnings.take().is_empty(),
        "later appends report nothing more"
    );
    assert_progress_names(&hub, "scripted detector failure at chunk 2");
    drop(take);
    assert!(!hub.current().busy, "the report ends with the take");
    state.shutdown();
}
