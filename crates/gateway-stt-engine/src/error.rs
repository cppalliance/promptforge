//! Backend-neutral STT engine construction and transcription failures.

use std::path::PathBuf;

/// An STT engine construction or transcription failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TranscribeError {
    /// The selected transcription backend could not be initialized.
    #[non_exhaustive]
    #[error("initialize transcription backend")]
    InitializeBackend(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The transcription model file could not be loaded.
    #[non_exhaustive]
    #[error("load transcription model {}", path.display())]
    LoadModel {
        /// The model path that failed to load.
        path: PathBuf,
        /// The underlying backend error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The transcription worker thread could not be started.
    #[non_exhaustive]
    #[error("spawn transcription worker")]
    SpawnWorker(#[source] std::io::Error),

    /// The decoder rejected an audio window.
    #[non_exhaustive]
    #[error("transcribe audio window")]
    Inference(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The transcription worker exited while requests were in flight.
    #[non_exhaustive]
    #[error("transcription worker exited")]
    WorkerGone,

    /// The selected model worker has no free queue slot.
    #[non_exhaustive]
    #[error("transcription worker queue is full")]
    Overloaded,

    /// Model construction or decoding panicked on its worker thread.
    #[non_exhaustive]
    #[error("transcription worker panicked")]
    WorkerPanicked,

    /// The STT engine configuration is invalid.
    #[non_exhaustive]
    #[error("invalid STT configuration: {0}")]
    InvalidConfig(String),
}

impl TranscribeError {
    /// Translates a backend initialization source.
    pub fn initialize_backend(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::InitializeBackend(Box::new(source))
    }

    /// Translates a model construction source while preserving its path.
    pub fn load_model(
        path: PathBuf,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::LoadModel {
            path,
            source: Box::new(source),
        }
    }

    /// Translates a backend inference source.
    pub fn inference(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Inference(Box::new(source))
    }
}
