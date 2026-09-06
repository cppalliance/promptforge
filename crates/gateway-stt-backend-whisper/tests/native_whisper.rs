//! Native characterization of the packaged Whisper backend contract.

#![expect(
    clippy::expect_used,
    reason = "native fixture setup fails by panicking with the missing invariant named"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::SttEngine;
use shared_progress::{ProgressHandle, ProgressHub};

const JFK_TRANSCRIPT: &str = "And so my fellow Americans ask not what your country can do for you, ask what you can do for your country.";
const UNPROMPTED_CLIP_TRANSCRIPT: &str = "country can do for you.";
const GLOSSARY_CLIP_TRANSCRIPT: &str = "One tree can do for you.";
const CONDITIONING_TRANSCRIPT: &str = "And so my fellow Americans asked";
const CONDITIONED_CLIP_TRANSCRIPT: &str = "what I can do for you.";
const SAMPLES_PER_TENTH: usize = 1_600;
static NATIVE_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn require_fixture(variable: &str, fallback: &str) -> PathBuf {
    let path =
        std::env::var_os(variable).map_or_else(|| fixture_dir().join(fallback), PathBuf::from);
    assert!(
        path.is_file(),
        "native test fixture is missing: {}",
        path.display()
    );
    path
}

fn jfk_samples() -> Vec<f32> {
    let path = require_fixture("PROMPTFORGE_WHISPER_AUDIO", "jfk.wav");
    let mut reader = hound::WavReader::open(path).expect("JFK fixture opens");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000, "fixture must be 16 kHz");
    assert_eq!(spec.channels, 1, "fixture must be mono");
    assert_eq!(spec.bits_per_sample, 16, "fixture must be 16-bit PCM");
    reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.expect("fixture sample decodes")) / 32_768.0)
        .collect()
}

fn engine(library: PathBuf, interim: PathBuf, final_model: Option<PathBuf>) -> SttEngine {
    engine_with_progress(library, interim, final_model, None)
}

fn engine_with_progress(
    library: PathBuf,
    interim: PathBuf,
    final_model: Option<PathBuf>,
    progress: Option<ProgressHandle>,
) -> SttEngine {
    let config = WhisperConfig::new(library, interim, final_model, progress);
    let factory = WhisperModelFactory::new(config).expect("packaged runtime loads");
    SttEngine::new(factory, 12, 500).expect("backend models load")
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn packaged_runtime_preserves_native_transcription_contract() {
    let _guard = NATIVE_TEST.lock().await;
    let temp = tempfile::tempdir().expect("temporary packaged-runtime directory");
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", "whisper.dll");
    let model = temp.path().join("ggml-tiny.en.bin");
    std::fs::copy(
        require_fixture("PROMPTFORGE_WHISPER_MODEL", "ggml-tiny.en.bin"),
        &model,
    )
    .expect("copy the exact tiny model fixture");
    let samples = jfk_samples();
    let prompt_sensitive_clip = samples[60 * SAMPLES_PER_TENTH..80 * SAMPLES_PER_TENTH].to_vec();
    let conditioning_clip = samples[..40 * SAMPLES_PER_TENTH].to_vec();

    let unprompted = engine(library.clone(), model.clone(), Some(model.clone()));
    let interim = unprompted
        .transcribe(samples.clone(), Vec::new())
        .await
        .expect("interim decode succeeds");
    assert_eq!(interim, JFK_TRANSCRIPT, "interim decode policy stays fixed");

    let unprompted_clip = unprompted
        .transcribe_final(prompt_sensitive_clip.clone(), Vec::new(), String::new())
        .await
        .expect("a final model is configured")
        .expect("unprompted final decode succeeds");
    assert_eq!(unprompted_clip, UNPROMPTED_CLIP_TRANSCRIPT);

    let conditioning_transcript = unprompted
        .transcribe_final(conditioning_clip, Vec::new(), String::new())
        .await
        .expect("a final model is configured")
        .expect("conditioning decode succeeds");
    let conditioned_clip = unprompted
        .transcribe_final(
            prompt_sensitive_clip.clone(),
            Vec::new(),
            conditioning_transcript.clone(),
        )
        .await
        .expect("a final model is configured")
        .expect("transcript-conditioned final decode succeeds");
    assert_eq!(conditioning_transcript, CONDITIONING_TRANSCRIPT);
    assert_eq!(conditioned_clip, CONDITIONED_CLIP_TRANSCRIPT);
    assert_ne!(conditioned_clip, unprompted_clip);

    let glossary_prompted = engine(library, model.clone(), Some(model.clone()));
    let glossary_clip = glossary_prompted
        .transcribe_final(
            prompt_sensitive_clip,
            vec!["one tree".to_string()],
            String::new(),
        )
        .await
        .expect("a final model is configured")
        .expect("the glossary-conditioned segment decodes");
    let silent_tail = glossary_prompted
        .transcribe_final(
            vec![0.0; 16_000],
            vec!["one tree".to_string()],
            glossary_clip.clone(),
        )
        .await
        .expect("a final model is configured")
        .expect("the silent tail decodes");
    assert!(silent_tail.is_empty(), "silence remains gated");
    assert_eq!(glossary_clip, GLOSSARY_CLIP_TRANSCRIPT);
    assert_ne!(glossary_clip, unprompted_clip);

    drop(glossary_prompted);
    drop(unprompted);
    std::fs::remove_file(model).expect("dropping the engine releases the model");
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn independent_final_jobs_do_not_require_a_reset() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", "whisper.dll");
    let model = require_fixture("PROMPTFORGE_WHISPER_MODEL", "ggml-tiny.en.bin");
    let engine = engine(library, model.clone(), Some(model));
    let samples = jfk_samples();

    let first = engine
        .transcribe_final(samples.clone(), Vec::new(), String::new())
        .await
        .expect("a final model is configured")
        .expect("first job succeeds");
    let second = engine
        .transcribe_final(samples, Vec::new(), String::new())
        .await
        .expect("a final model is configured")
        .expect("second job succeeds");
    assert_eq!(second, first, "equal stateless jobs remain independent");
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn one_final_job_cannot_change_another_jobs_history() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", "whisper.dll");
    let model = require_fixture("PROMPTFORGE_WHISPER_MODEL", "ggml-tiny.en.bin");
    let engine = engine(library, model.clone(), Some(model));
    let samples = jfk_samples();
    let prompt_sensitive = samples[6 * 16_000..8 * 16_000].to_vec();

    let control = engine
        .transcribe_final(prompt_sensitive.clone(), Vec::new(), String::new())
        .await
        .expect("a final model is configured")
        .expect("control job succeeds");
    let history = engine
        .transcribe_final(samples[..4 * 16_000].to_vec(), Vec::new(), String::new())
        .await
        .expect("a final model is configured")
        .expect("history source succeeds");
    let conditioned = engine
        .transcribe_final(prompt_sensitive.clone(), Vec::new(), history)
        .await
        .expect("a final model is configured")
        .expect("conditioned job succeeds");
    assert_ne!(conditioned, control, "fixture detects conditioning");

    let standalone = engine
        .transcribe_final(prompt_sensitive, Vec::new(), String::new())
        .await
        .expect("a final model is configured")
        .expect("standalone job succeeds");
    assert_eq!(
        standalone, control,
        "prior job history cannot leak into a stateless decode"
    );
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn final_decode_is_absent_without_a_final_model() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", "whisper.dll");
    let model = require_fixture("PROMPTFORGE_WHISPER_MODEL", "ggml-tiny.en.bin");
    let engine = engine(library, model, None);
    assert!(
        engine
            .transcribe_final(jfk_samples(), Vec::new(), String::new())
            .await
            .is_none(),
        "an omitted final model leaves no final decoder"
    );
}

#[tokio::test]
#[ignore = "requires packaged whisper and model fixtures"]
async fn configured_model_branches_finish_prewarm_and_init_progress() {
    let _guard = NATIVE_TEST.lock().await;
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", "whisper.dll");
    let model = require_fixture("PROMPTFORGE_WHISPER_MODEL", "ggml-tiny.en.bin");
    let hub = Arc::new(ProgressHub::new());
    let tree = hub.operation();
    let models = tree.register("models", 1.0);

    let engine = engine_with_progress(library, model.clone(), Some(model), Some(models));
    assert!(engine.has_final_pass(), "the final branch is configured");

    let snapshot = hub.snapshot();
    let nodes = &snapshot[0].nodes;
    for branch in ["interim", "final"] {
        let branch_path = format!("models/{branch}");
        assert!(
            nodes.iter().any(|node| node.path == branch_path),
            "{branch} model progress branch is present: {nodes:?}"
        );
        for stage in ["prewarm", "init"] {
            let path = format!("{branch_path}/{stage}");
            let node = nodes
                .iter()
                .find(|node| node.path == path)
                .unwrap_or_else(|| panic!("{path} progress is present: {nodes:?}"));
            assert!(
                node.finished && node.ok,
                "{path} reaches a successful terminal state: {node:?}"
            );
            assert!(
                (node.fraction - 1.0).abs() < f64::EPSILON,
                "{path} completes all work: {node:?}"
            );
        }
    }
}
