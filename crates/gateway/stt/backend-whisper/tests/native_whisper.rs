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
    let config = WhisperConfig::new(library, interim, final_model, progress);
    let factory = WhisperModelFactory::new(config).expect("packaged runtime loads");
    let policy =
        EnginePolicy::new(12, 500, factory.gpu_available()).expect("capture policy is valid");
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
        .expect("interim decode succeeds");
    assert_eq!(interim, JFK_TRANSCRIPT, "interim decode policy stays fixed");

    let unprompted_clip = unprompted
        .decode(request(
            DecodeMode::Final,
            prompt_sensitive_clip.clone(),
            Vec::new(),
            "",
        ))
        .await
        .expect("unprompted final decode succeeds");
    assert_eq!(unprompted_clip, UNPROMPTED_CLIP_TRANSCRIPT);

    let conditioning_transcript = unprompted
        .decode(request(
            DecodeMode::Final,
            conditioning_clip,
            Vec::new(),
            "",
        ))
        .await
        .expect("conditioning decode succeeds");
    let conditioned_clip = unprompted
        .decode(request(
            DecodeMode::Final,
            prompt_sensitive_clip.clone(),
            Vec::new(),
            conditioning_transcript.clone(),
        ))
        .await
        .expect("transcript-conditioned final decode succeeds");
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
        .expect("the glossary-conditioned segment decodes");
    let silent_tail = glossary_prompted
        .decode(request(
            DecodeMode::Final,
            vec![0.0; 16_000],
            vec!["one tree".to_string()],
            glossary_clip.clone(),
        ))
        .await
        .expect("the silent tail decodes");
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
        .expect("history source succeeds");
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
        .expect("a decode whose flag stays unset transcribes");
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
