//! STT engine construction and transcription failures.

use std::path::PathBuf;

/// An STT engine construction or transcription failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TranscribeError {
    /// The whisper model file could not be loaded.
    #[non_exhaustive]
    #[error("load whisper model {}", path.display())]
    LoadModel {
        /// The model path that failed to load.
        path: PathBuf,
        /// The underlying whisper.cpp error, boxed to hide the dependency.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The transcription worker thread could not be started.
    #[non_exhaustive]
    #[error("spawn transcription worker")]
    SpawnWorker(#[source] std::io::Error),

    /// The model rejected an audio window.
    #[non_exhaustive]
    #[error("transcribe audio window")]
    Inference(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The transcription worker exited while requests were in flight.
    #[non_exhaustive]
    #[error("transcription worker exited")]
    WorkerGone,

    /// The STT engine configuration is invalid.
    #[non_exhaustive]
    #[error("invalid STT configuration: {0}")]
    InvalidConfig(String),
}
