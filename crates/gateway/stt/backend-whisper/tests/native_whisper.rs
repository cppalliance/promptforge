//! Native characterization of the packaged Whisper backend contract.
//! Miri excludes native model loading and decode; packaged-runtime CI owns them.

#![expect(
    clippy::expect_used,
    reason = "native fixture setup fails by panicking with the missing invariant named"
)]

mod common;

use std::error::Error as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use common::{fixture_dir, jfk_samples};
use gateway_progress::{Activity, ProgressHub};
use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::test_fixtures::native::require_fixture;
use gateway_stt_engine::{DecodeMode, DecodeRequest, EnginePolicy, SttEngine, TranscribeError};
use gateway_whisper_ffi::WhisperError;

const JFK_TRANSCRIPT: &str = "And so my fellow Americans ask not what your country can do for you, ask what you can do for your country.";
/// The interim role decodes with timestamp tokens, which drop the comma
/// after "for you".
const JFK_INTERIM_TRANSCRIPT: &str = "And so my fellow Americans ask not what your country can do for you ask what you can do for your country.";
const UNPROMPTED_CLIP_TRANSCRIPT: &str = "country can do for you.";
const GLOSSARY_CLIP_TRANSCRIPT: &str = "One tree can do for you.";
const CONDITIONING_TRANSCRIPT: &str = "And so my fellow Americans asked";
const CONDITIONED_CLIP_TRANSCRIPT: &str = "what I can do for you.";
const SAMPLES_PER_TENTH: usize = 1_600;
/// Repeats of the 11 s clip in the long decode: about ten minutes, which the
/// fastest build still takes seconds to transcribe.
const LONG_CLIP_REPEATS: usize = 55;
/// How far into the long decode its flag is set.
const MID_PASS: Duration = Duration::from_millis(300);
/// The longest an aborted decode may run on after its flag is set: one
/// encoder pass or decoder step, which tiny.en finishes well within it.
const ABORT_BOUND: Duration = Duration::from_secs(1);
/// The capture policy's interim window.
const WINDOW_SECONDS: u64 = 12;
static NATIVE_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[test]
fn native_backend_suite_keeps_its_backend_fixture_root() {
    assert_eq!(
        fixture_dir(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    );
}

fn engine(library: PathBuf, interim: PathBuf, final_model: Option<PathBuf>) -> SttEngine {
    engine_with_progress(library, interim, final_model, None)
}

fn engine_with_progress(
    library: PathBuf,
    interim: PathBuf,
    final_model: Option<PathBuf>,
    progress: Option<Weak<Activity>>,
) -> SttEngine {
    let config = WhisperConfig::new(library, interim, final_model, WINDOW_SECONDS, progress);
    let factory = WhisperModelFactory::new(config).expect("packaged runtime loads");
    let policy = EnginePolicy::new(WINDOW_SECONDS, 500, factory.gpu_available())
        .expect("capture policy is valid");
    SttEngine::new(factory, policy).expect("backend models load")
}

fn request(
    mode: DecodeMode,
    samples: Vec<f32>,
    guidance: Vec<String>,
    finalized: impl Into<String>,
) -> DecodeRequest {
    DecodeRequest::new(mode, samples, guidance, finalized.into())
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn packaged_runtime_preserves_native_transcription_contract() {
    let _guard = NATIVE_TEST.lock().await;
    let temp = tempfile::tempdir().expect("temporary packaged-runtime directory");
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", &fixture_dir(), "whisper.dll");
    let model = temp.path().join("ggml-tiny.en.bin");
    std::fs::copy(
        require_fixture(
            "PROMPTFORGE_WHISPER_MODEL",
            &fixture_dir(),
            "ggml-tiny.en.bin",
        ),
        &model,
    )
    .expect("copy the exact tiny model fixture");
    let samples = jfk_samples();
    let prompt_sensitive_clip = samples[60 * SAMPLES_PER_TENTH..80 * SAMPLES_PER_TENTH].to_vec();
    let conditioning_clip = samples[..40 * SAMPLES_PER_TENTH].to_vec();

    let unprompted = engine(library.clone(), model.clone(), Some(model.clone()));
    let interim = unprompted
        .decode(request(
            DecodeMode::Interim,
            samples.clone(),
            Vec::new(),
            "",
        ))
        .await
        .expect("interim decode succeeds")
        .into_text();
    assert_eq!(
        interim, JFK_INTERIM_TRANSCRIPT,
        "interim decode policy stays fixed"
    );

    let unprompted_clip = unprompted
        .decode(request(
            DecodeMode::Final,
            prompt_sensitive_clip.clone(),
            Vec::new(),
            "",
        ))
        .await
        .expect("unprompted final decode succeeds")
        .into_text();
    assert_eq!(unprompted_clip, UNPROMPTED_CLIP_TRANSCRIPT);

    let conditioning_transcript = unprompted
        .decode(request(
            DecodeMode::Final,
            conditioning_clip,
            Vec::new(),
            "",
        ))
        .await
        .expect("conditioning decode succeeds")
        .into_text();
    let conditioned_clip = unprompted
        .decode(request(
            DecodeMode::Final,
            prompt_sensitive_clip.clone(),
            Vec::new(),
            conditioning_transcript.clone(),
        ))
        .await
        .expect("transcript-conditioned final decode succeeds")
        .into_text();
    assert_eq!(conditioning_transcript, CONDITIONING_TRANSCRIPT);
    assert_eq!(conditioned_clip, CONDITIONED_CLIP_TRANSCRIPT);
    assert_ne!(conditioned_clip, unprompted_clip);

    let glossary_prompted = engine(library, model.clone(), Some(model.clone()));
    let glossary_clip = glossary_prompted
        .decode(request(
            DecodeMode::Final,
            prompt_sensitive_clip,
            vec!["one tree".to_string()],
            "",
        ))
        .await
        .expect("the glossary-conditioned segment decodes")
        .into_text();
    let silent_tail = glossary_prompted
        .decode(request(
            DecodeMode::Final,
            vec![0.0; 16_000],
            vec!["one tree".to_string()],
            glossary_clip.clone(),
        ))
        .await
        .expect("the silent tail decodes")
        .into_text();
    assert!(silent_tail.is_empty(), "silence remains gated");
    assert_eq!(glossary_clip, GLOSSARY_CLIP_TRANSCRIPT);
    assert_ne!(glossary_clip, unprompted_clip);

    glossary_prompted
        .shutdown()
        .expect("the glossary-prompted engine shuts down");
    unprompted
        .shutdown()
        .expect("the unprompted engine shuts down");
    std::fs::remove_file(model).expect("shutting down the engines releases the model");
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn independent_final_jobs_do_not_require_a_reset() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", &fixture_dir(), "whisper.dll");
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &fixture_dir(),
        "ggml-tiny.en.bin",
    );
    let engine = engine(library, model.clone(), Some(model));
    let samples = jfk_samples();

    let first = engine
        .decode(request(DecodeMode::Final, samples.clone(), Vec::new(), ""))
        .await
        .expect("first job succeeds");
    let second = engine
        .decode(request(DecodeMode::Final, samples, Vec::new(), ""))
        .await
        .expect("second job succeeds");
    assert_eq!(second, first, "equal stateless jobs remain independent");
    engine.shutdown().expect("the engine shuts down");
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn one_final_job_cannot_change_another_jobs_history() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", &fixture_dir(), "whisper.dll");
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &fixture_dir(),
        "ggml-tiny.en.bin",
    );
    let engine = engine(library, model.clone(), Some(model));
    let samples = jfk_samples();
    let prompt_sensitive = samples[6 * 16_000..8 * 16_000].to_vec();

    let control = engine
        .decode(request(
            DecodeMode::Final,
            prompt_sensitive.clone(),
            Vec::new(),
            "",
        ))
        .await
        .expect("control job succeeds");
    let history = engine
        .decode(request(
            DecodeMode::Final,
            samples[..4 * 16_000].to_vec(),
            Vec::new(),
            "",
        ))
        .await
        .expect("history source succeeds")
        .into_text();
    let conditioned = engine
        .decode(request(
            DecodeMode::Final,
            prompt_sensitive.clone(),
            Vec::new(),
            history,
        ))
        .await
        .expect("conditioned job succeeds");
    assert_ne!(conditioned, control, "fixture detects conditioning");

    let standalone = engine
        .decode(request(DecodeMode::Final, prompt_sensitive, Vec::new(), ""))
        .await
        .expect("standalone job succeeds");
    assert_eq!(
        standalone, control,
        "prior job history cannot leak into a stateless decode"
    );
    engine.shutdown().expect("the engine shuts down");
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn final_decode_is_absent_without_a_final_model() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", &fixture_dir(), "whisper.dll");
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &fixture_dir(),
        "ggml-tiny.en.bin",
    );
    let engine = engine(library, model, None);
    let error = engine
        .decode(request(DecodeMode::Final, jfk_samples(), Vec::new(), ""))
        .await
        .expect_err("an omitted final model leaves no final decoder");
    assert!(
        error
            .to_string()
            .contains("final decoder is not configured"),
        "the missing final worker is classified explicitly: {error}"
    );
    engine.shutdown().expect("the engine shuts down");
}

#[tokio::test]
#[ignore = "requires packaged whisper and model fixtures"]
async fn configured_model_branches_write_their_load_text_then_release_the_activity() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", &fixture_dir(), "whisper.dll");
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &fixture_dir(),
        "ggml-tiny.en.bin",
    );
    let hub = ProgressHub::new();
    let activity = Arc::new(hub.begin("loading-speech"));

    let engine = engine_with_progress(
        library,
        model.clone(),
        Some(model),
        Some(Arc::downgrade(&activity)),
    );
    assert!(engine.has_final_pass(), "the final branch is configured");
    let text = hub.current().text;
    assert!(
        text.starts_with("Initializing ") && text.ends_with(" speech model"),
        "the last stage written is a branch's context initialization: {text:?}"
    );

    // The factory holds the activity weakly: the load's guard alone keeps
    // the hub busy, and dropping it leaves the factory nothing to write to.
    drop(activity);
    assert!(!hub.current().busy, "the load's guard ended the activity");
    engine.shutdown().expect("the engine shuts down");
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn interim_word_ends_rise_inside_their_window_one_per_word() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", &fixture_dir(), "whisper.dll");
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &fixture_dir(),
        "ggml-tiny.en.bin",
    );
    let engine = engine(library, model.clone(), Some(model));
    let samples = jfk_samples();
    let windows = [
        ("the whole clip", 0..samples.len()),
        ("the opening words", 0..40 * SAMPLES_PER_TENTH),
        (
            "a window opening mid-speech",
            30 * SAMPLES_PER_TENTH..samples.len(),
        ),
    ];
    for (name, range) in windows {
        let window = u64::try_from(range.len()).expect("the window length fits u64");
        let output = engine
            .decode(request(
                DecodeMode::Interim,
                samples[range].to_vec(),
                Vec::new(),
                "",
            ))
            .await
            .expect("interim decode succeeds");
        let ends = output.word_ends();
        assert_eq!(
            ends.len(),
            output.text().split_whitespace().count(),
            "{name} has one end per word of {:?}: {ends:?}",
            output.text()
        );
        assert!(!ends.is_empty(), "{name} decodes words");
        assert!(
            ends.is_sorted(),
            "{name}'s word ends never fall back: {ends:?}"
        );
        assert!(
            ends.iter().all(|&end| end > 0 && end <= window),
            "{name}'s word ends lie inside its {window} samples: {ends:?}"
        );
        assert!(
            ends.first().is_some_and(|&first| first < window / 2),
            "{name}'s first word ends in the first half of its window: {ends:?}"
        );
        assert!(
            ends.last().is_some_and(|&last| last > window / 2),
            "{name}'s last word ends in the second half of its window: {ends:?}"
        );
    }

    let accurate = engine
        .decode(request(DecodeMode::Final, samples, Vec::new(), ""))
        .await
        .expect("final decode succeeds");
    assert_eq!(accurate.text(), JFK_TRANSCRIPT);
    assert!(
        accurate.word_ends().is_empty(),
        "the final role requests no timestamps"
    );
    engine.shutdown().expect("the engine shuts down");
}

fn whisper_source(error: &TranscribeError) -> Option<&WhisperError> {
    error.source()?.downcast_ref::<WhisperError>()
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn a_decode_ends_when_its_cancellation_flag_is_set() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", &fixture_dir(), "whisper.dll");
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &fixture_dir(),
        "ggml-tiny.en.bin",
    );
    let engine = engine(library, model.clone(), Some(model));
    let samples = jfk_samples();

    let preset = Arc::new(AtomicBool::new(true));
    let error = engine
        .decode(
            request(DecodeMode::Final, samples.clone(), Vec::new(), "").with_cancellation(preset),
        )
        .await
        .expect_err("a decode whose flag is already set fails");
    assert!(
        matches!(error, TranscribeError::Inference { .. }),
        "a preset flag fails as inference: {error}"
    );
    assert!(
        whisper_source(&error).is_none(),
        "a preset flag fails before whisper runs a pass: {:?}",
        error.source()
    );

    let unset = Arc::new(AtomicBool::new(false));
    let text = engine
        .decode(
            request(DecodeMode::Final, samples.clone(), Vec::new(), "")
                .with_cancellation(Arc::clone(&unset)),
        )
        .await
        .expect("a decode whose flag stays unset transcribes")
        .into_text();
    assert_eq!(
        text, JFK_TRANSCRIPT,
        "an unset flag leaves the decode whole"
    );

    let long: Vec<f32> = samples
        .iter()
        .copied()
        .cycle()
        .take(samples.len() * LONG_CLIP_REPEATS)
        .collect();
    let flag = Arc::new(AtomicBool::new(false));
    let decode = async {
        let result = engine
            .decode(
                request(DecodeMode::Final, long, Vec::new(), "")
                    .with_cancellation(Arc::clone(&flag)),
            )
            .await;
        (result, Instant::now())
    };
    let cancel = async {
        tokio::time::sleep(MID_PASS).await;
        flag.store(true, Ordering::Release);
        Instant::now()
    };
    let ((result, finished_at), cancelled_at) = tokio::join!(decode, cancel);
    let Some(ran_on) = finished_at.checked_duration_since(cancelled_at) else {
        let early = cancelled_at.duration_since(finished_at);
        panic!(
            "the long decode ended {early:?} before its flag was set {MID_PASS:?} in: {result:?}"
        );
    };
    let Err(error) = result else {
        panic!("the long decode transcribed after its flag was set {MID_PASS:?} in");
    };
    assert!(
        matches!(whisper_source(&error), Some(WhisperError::Inference { .. })),
        "whisper's abort ends the pass as an inference failure: {error}: {:?}",
        error.source()
    );
    assert!(
        ran_on < ABORT_BOUND,
        "the decode ended {ran_on:?} after its flag was set, past {ABORT_BOUND:?}"
    );
    engine.shutdown().expect("the engine shuts down");
}
