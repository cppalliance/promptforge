//! Backend-neutral speech decoding on dedicated worker threads.
//!
//! [`SttEngine`] owns an interim decoder and an optional final decoder.
//! Backends implement [`ModelFactory`] and [`Decoder`], while callers retain
//! session, prompt input, transcript, and publication state.

mod decoder;
mod engine;
mod error;
mod policy;
mod worker;

pub use decoder::{Decoder, ModelFactory};
pub use engine::SttEngine;
pub use error::TranscribeError;
pub use policy::{MIN_WINDOW_SAMPLES, SAMPLE_RATE, is_silence};
