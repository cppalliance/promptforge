//! The native production session that the replay capture and the long-speech
//! test both drive: the Whisper models the environment names, one registered
//! Realtime session, and the 24 kHz stream a client sends.
//!
//! The models come from `PROMPTFORGE_WHISPER_LIBRARY`,
//! `PROMPTFORGE_WHISPER_MODEL` for the interim role, and
//! `PROMPTFORGE_WHISPER_FINAL_MODEL` for the final role, which falls back to
//! the interim model when unset. Each caller owns its own clock: the capture
//! measures one on audio position plus decode time, and the long-speech test
//! paces its appends on the wall clock.

#![expect(
    clippy::expect_used,
    reason = "a native fixture that fails to load fails the test with the step named"
)]

pub(crate) mod audio;

use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::native::fixture_final_model;
use gateway_stt::test_fixtures::{
    RealtimeSessionFixture, RealtimeSessionRegistryFixture, load_scripted_initial_with_cancellation,
};
use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::ModelFactory;
use tokio_util::sync::CancellationToken;

use crate::common;

/// The window of the fixed policy the fixture load publishes, which is the
/// gateway default.
pub(crate) const WINDOW_SECONDS: u64 = 15;
pub(crate) const SAMPLES_PER_MS: u64 = 16;
/// 100 ms of 16 kHz audio, the chunk the client streams.
pub(crate) const CHUNK_SAMPLES: u64 = 1_600;
/// An interim tick follows every 500 ms of appended audio.
pub(crate) const TICK_SAMPLES: u64 = 8_000;
const HYPOTHESIS_UPDATE: &str = r#"{"type":"session.update","session":{"type":"transcription","include":["item.input_audio_transcription.hypothesis"]}}"#;

/// Builds the Whisper factory from the library and models the environment
/// names, rather than loading a gateway config: a config load builds its
/// factory internally, leaving no place for a recording wrapper, and
/// provisions the pinned whisper build from the artifact store instead of the
/// named library.
pub(crate) fn whisper_factory() -> WhisperModelFactory {
    let model = common::require_model();
    let config = WhisperConfig::new(
        common::require_library(),
        model.clone(),
        Some(fixture_final_model(&model)),
        WINDOW_SECONDS,
        None,
    );
    WhisperModelFactory::new(config).expect("the packaged runtime loads")
}

/// Loads `factory` as the service's initial generation. The returned service
/// must outlive every session registered on it.
pub(crate) fn load(factory: impl ModelFactory) -> SpeechService {
    let service = SpeechService::new();
    load_scripted_initial_with_cancellation(&service, factory, &CancellationToken::new())
        .expect("the native interim and final models load");
    service
}

/// Registers one production session on `service`, with the hypothesis
/// events enabled.
pub(crate) fn register(service: &SpeechService) -> RealtimeSessionFixture {
    let mut session = RealtimeSessionRegistryFixture::default()
        .register_with_service(service)
        .expect("a native session registers");
    session
        .update_text(HYPOTHESIS_UPDATE)
        .expect("the hypothesis include applies");
    session
}
