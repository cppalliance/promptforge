//! Native characterization of the packaged Whisper runtime contract.

use gateway_stt_engine::{EngineConfig, SttEngine, fixtures};

const JFK_TRANSCRIPT: &str = "And so my fellow Americans ask not what your country can do for you, ask what you can do for your country.";
const UNPROMPTED_CLIP_TRANSCRIPT: &str = "country can do for you.";
const GLOSSARY_CLIP_TRANSCRIPT: &str = "One tree can do for you.";
const CONDITIONING_TRANSCRIPT: &str = "And so my fellow Americans asked";
const CONDITIONED_CLIP_TRANSCRIPT: &str = "what I can do for you.";
const SAMPLES_PER_TENTH: usize = 1_600;

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn packaged_runtime_preserves_native_transcription_contract() {
    let temp = tempfile::tempdir().expect("temporary packaged-runtime directory");
    let library = fixtures::require_library();
    let model = temp.path().join("ggml-tiny.en.bin");
    std::fs::copy(fixtures::require_model(), &model).expect("copy the exact tiny model fixture");
    let samples = fixtures::jfk_samples();
    let prompt_sensitive_clip = samples[60 * SAMPLES_PER_TENTH..80 * SAMPLES_PER_TENTH].to_vec();
    let conditioning_clip = samples[..40 * SAMPLES_PER_TENTH].to_vec();

    let unprompted = SttEngine::new(&EngineConfig {
        library: library.clone(),
        interim_model: model.clone(),
        final_model: Some(model.clone()),
        window_seconds: 12,
        interval_ms: 500,
    })
    .expect("packaged runtime and model load");

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
    assert_eq!(
        unprompted_clip, UNPROMPTED_CLIP_TRANSCRIPT,
        "the prompt-sensitive clip has a fixed unprompted control"
    );

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
    assert_eq!(
        conditioning_transcript, CONDITIONING_TRANSCRIPT,
        "the accumulated transcript that conditions the tail stays fixed"
    );
    assert_eq!(
        conditioned_clip, CONDITIONED_CLIP_TRANSCRIPT,
        "the accumulated transcript changes the prompt-sensitive tail"
    );
    assert_ne!(
        conditioned_clip, unprompted_clip,
        "removing accumulated-transcript conditioning must fail this target"
    );

    let glossary_prompted = SttEngine::new(&EngineConfig {
        library,
        interim_model: model.clone(),
        final_model: Some(model.clone()),
        window_seconds: 12,
        interval_ms: 500,
    })
    .expect("glossary-prompted engine loads");
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
    assert_eq!(
        glossary_clip, GLOSSARY_CLIP_TRANSCRIPT,
        "the glossary changes the prompt-sensitive segment"
    );
    assert_ne!(
        glossary_clip, unprompted_clip,
        "removing glossary conditioning must fail this target"
    );

    drop(glossary_prompted);
    drop(unprompted);
    std::fs::remove_file(model).expect("dropping the engine releases the model");
}
