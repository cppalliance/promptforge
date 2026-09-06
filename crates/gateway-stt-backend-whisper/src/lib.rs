//! Safe Whisper backend for the backend-neutral STT engine.

mod config;
mod model;
mod prompt;

pub use config::WhisperConfig;
pub use model::WhisperModelFactory;
