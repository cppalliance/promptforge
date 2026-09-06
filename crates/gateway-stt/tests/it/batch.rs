//! Characterization tests for physical-model batch transcription.

use axum::http::StatusCode;
use gateway_transcribe::fixtures::jfk_samples;

use crate::common::{copy_model_replacing_token, fixture_runtime_with_models, transcribe_batch};

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn batch_selects_each_loaded_physical_model_by_name() {
    let interim_model = gateway_transcribe::fixtures::require_model();
    let fixture_dir = tempfile::tempdir().expect("distinct model tempdir");
    let final_model =
        copy_model_replacing_token(&interim_model, fixture_dir.path(), b"country", b"kingdom");
    let (state, runtime) = fixture_runtime_with_models(&interim_model, Some(final_model.as_path()));
    let samples = jfk_samples();

    let (interim_status, interim_response) =
        transcribe_batch(state.clone(), "speech", &samples).await;
    assert_eq!(
        interim_status,
        StatusCode::OK,
        "the interim physical model is directly selectable"
    );
    let interim_text = interim_response["text"]
        .as_str()
        .expect("interim batch response text is a string")
        .to_lowercase();
    assert!(
        interim_text.contains("country") && !interim_text.contains("kingdom"),
        "speech reaches the unmodified interim worker: {interim_text:?}"
    );

    let (final_status, final_response) =
        transcribe_batch(state.clone(), "speech-final", &samples).await;
    assert_eq!(
        final_status,
        StatusCode::OK,
        "the final physical model is directly selectable"
    );
    let final_text = final_response["text"]
        .as_str()
        .expect("final batch response text is a string")
        .to_lowercase();
    assert!(
        final_text.contains("kingdom") && !final_text.contains("country"),
        "speech-final reaches the vocabulary-distinguished final worker: {final_text:?}"
    );

    runtime.shutdown();
}
