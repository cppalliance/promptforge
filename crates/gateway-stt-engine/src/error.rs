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
    /// Interim model construction did not report an outcome before its deadline.
    #[non_exhaustive]
    #[error("interim transcription worker startup timed out")]
    InterimStartupTimedOut,
    /// Final model construction did not report an outcome before its deadline.
    #[non_exhaustive]
    #[error("final transcription worker startup timed out")]
    FinalStartupTimedOut,
    /// Multiple worker startup outcomes failed during the shared deadline.
    #[non_exhaustive]
    #[error("multiple transcription workers failed during startup")]
    StartupFailures {
        /// Every observed startup failure, ordered interim then final.
        failures: Vec<TranscribeError>,
    },
    /// A worker thread panicked while shutdown joined it.
    #[non_exhaustive]
    #[error("transcription worker panicked during shutdown")]
    ShutdownPanicked,
    /// Multiple worker threads panicked while shutdown joined them.
    #[non_exhaustive]
    #[error("multiple transcription workers panicked during shutdown")]
    ShutdownFailures {
        /// Every failure observed while joining the workers.
        cleanup: Vec<TranscribeError>,
    },
    /// Worker startup failed and partial-startup cleanup also failed.
    #[non_exhaustive]
    #[error("transcription worker startup failed and cleanup also failed")]
    StartupCleanup {
        /// The startup failure that caused construction to stop.
        #[source]
        startup: Box<TranscribeError>,
        /// Every failure observed while joining partially started workers.
        cleanup: Vec<TranscribeError>,
    },
    /// The STT engine configuration is invalid.
    #[non_exhaustive]
    #[error("invalid STT configuration: {0}")]
    InvalidConfig(String),
}
