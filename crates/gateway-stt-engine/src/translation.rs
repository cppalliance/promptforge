//! Backend failure translation into engine-owned errors.

use std::path::PathBuf;

use crate::TranscribeError;

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
