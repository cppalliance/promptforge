//! Safe Whisper backend for the backend-neutral STT engine.

mod config;
mod guard;
mod model;
mod profile;
mod prompt;
mod silero;
mod words;

pub use config::WhisperConfig;
pub use model::WhisperModelFactory;
pub use silero::SileroDetector;
