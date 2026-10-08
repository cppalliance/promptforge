//! Backend-neutral speech decoding on dedicated worker threads.
//! [`SttEngine`] owns an interim decoder and an optional final decoder.
//! Backends implement [`ModelFactory`] and [`Decoder`], while callers retain
//! session, prompt input, transcript, and publication state.
//! [`SpeechDetector`] classifies streaming audio as speech chunk by chunk.
mod decoder;
mod detector;
mod engine;
mod error;
mod policy;
mod startup;
#[cfg(feature = "test-fixtures")]
pub mod test_fixtures;
mod translation;
mod worker;

pub use decoder::{DecodeMode, DecodeOutput, DecodeRequest, Decoder, ModelFactory};
#[cfg(any(test, feature = "test-fixtures"))]
pub use detector::EnergyDetector;
pub use detector::SpeechDetector;
pub use engine::SttEngine;
pub use error::{DetectorError, TranscribeError};
pub use policy::EnginePolicy;
