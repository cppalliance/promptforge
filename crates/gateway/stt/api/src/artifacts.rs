//! Verified speech artifacts and facade error vocabulary.

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use gateway_config::{Config, SttRole, WhisperBackend};
use gateway_local::artifacts::ArtifactStore;
use gateway_progress::Activity;
use tokio_util::sync::CancellationToken;

use crate::model::{ModelNames, REALTIME_TRANSCRIBE_MODEL};

#[path = "artifacts-silero.rs"]
mod silero;

#[cfg(feature = "test-fixtures")]
pub(crate) use silero::ScriptedSileroPin;
use silero::SileroPin;

/// Verified artifacts and policy for a runtime that has not started workers.
#[derive(Debug)]
pub(crate) struct PreparedSpeech {
    pub(crate) generation: Option<PreparedGeneration>,
}

#[derive(Debug)]
pub(crate) struct PreparedGeneration {
    pub(crate) library: PathBuf,
    pub(crate) interim_model: PathBuf,
    pub(crate) final_model: Option<PathBuf>,
    pub(crate) silero: PathBuf,
    pub(crate) names: ModelNames,
    pub(crate) guidance: Vec<String>,
    pub(crate) window_seconds: u64,
    pub(crate) interval_ms: u64,
    /// The load's activity, weakly held: the model factory this becomes
    /// lives for the process, while the activity ends with the load, so a
    /// later decoder rebuild finds nothing to report into.
    pub(crate) progress: Option<Weak<Activity>>,
}

#[derive(Debug, Default)]
struct ProvisionedModels {
    interim: Option<(String, PathBuf)>,
    final_model: Option<(String, PathBuf)>,
}

/// Provisions the whisper library, the speech models, and the Silero model;
/// a fired `cancel` stops their downloads at the next chunk and fails the
/// load as [`SpeechError::InitialLoadCancelled`].
pub(crate) fn prepare(
    config: &Config,
    progress: Option<&Arc<Activity>>,
    cancel: &CancellationToken,
) -> Result<PreparedSpeech, SpeechError> {
    prepare_impl(
        config,
        progress,
        cancel,
        ArtifactStore::provision_whisper_library_with_cancellation,
        SileroPin::PINNED,
    )
}

/// Body of [`prepare`] with the whisper library provision and the Silero
/// pin injectable, so a test can observe the backend `[stt]` and the load
/// token it hands over without probing the machine's GPUs or downloading a
/// runtime or the Silero model.
fn prepare_impl(
    config: &Config,
    progress: Option<&Arc<Activity>>,
    cancel: &CancellationToken,
    provision_library: impl FnOnce(
        &ArtifactStore,
        WhisperBackend,
        Option<&Activity>,
        Option<&CancellationToken>,
    ) -> Result<PathBuf, gateway_local::LocalError>,
    silero_pin: SileroPin<'_>,
) -> Result<PreparedSpeech, SpeechError> {
    if config.stt_models().is_empty() {
        return Ok(PreparedSpeech { generation: None });
    }
    if let Some(model) = config
        .stt_models()
        .iter()
        .find(|model| model.name() == REALTIME_TRANSCRIBE_MODEL)
    {
        return Err(SpeechError::ReservedModelName {
            model: model.name().to_owned(),
        });
    }

    let cache = gateway_local::resolve_cache_root(config.local().cache_dir())
        .map_err(SpeechError::Store)?;
    let store = ArtifactStore::new(cache).map_err(SpeechError::Store)?;
    let activity = progress.map(Arc::as_ref);
    if let Some(activity) = activity {
        activity.set_text("Provisioning whisper library");
    }
    // An absent `[stt]` section yields the defaults, whose backend is `auto`.
    let capture = config.stt().cloned().unwrap_or_default();
    let library = provision_library(&store, capture.whisper_backend(), activity, Some(cancel))
        .map_err(|source| cancelled_or(source, SpeechError::WhisperLibrary))?;
    tracing::info!(path = %library.display(), "provisioned whisper library");
    let models = provision_models(config, &store, activity, cancel)?;
    let Some((interim_name, interim_model)) = models.interim else {
        return Err(SpeechError::MissingInterim);
    };
    let (final_name, final_model) = models
        .final_model
        .map_or((None, None), |(name, path)| (Some(name), Some(path)));
    let silero = silero::provision(&store, silero_pin, activity, cancel)?;

    Ok(PreparedSpeech {
        generation: Some(PreparedGeneration {
            library,
            interim_model,
            final_model,
            silero,
            names: ModelNames::new(interim_name, final_name).map_err(|error| {
                SpeechError::ReservedModelName {
                    model: error.into_name(),
                }
            })?,
            guidance: capture.vocabulary().to_vec(),
            window_seconds: capture.window_seconds(),
            interval_ms: capture.interval_ms(),
            progress: progress.map(Arc::downgrade),
        }),
    })
}

fn provision_models(
    config: &Config,
    store: &ArtifactStore,
    activity: Option<&Activity>,
    cancel: &CancellationToken,
) -> Result<ProvisionedModels, SpeechError> {
    let mut provisioned = ProvisionedModels::default();
    for model in config.stt_models() {
        let path = store
            .ensure_model_with_cancellation(model.source(), model.sha256(), activity, Some(cancel))
            .map_err(|source| {
                cancelled_or(source, |source| SpeechError::Artifact {
                    model: model.name().to_owned(),
                    source,
                })
            })?;
        match model.role() {
            SttRole::Interim => provisioned.interim = Some((model.name().to_owned(), path)),
            SttRole::Final => provisioned.final_model = Some((model.name().to_owned(), path)),
            _ => {
                return Err(SpeechError::UnsupportedRole {
                    model: model.name().to_owned(),
                });
            }
        }
    }
    Ok(provisioned)
}

/// Maps a provision failure to the load's cancellation when the load token
/// stopped it, and through `stage` otherwise.
fn cancelled_or(
    source: gateway_local::LocalError,
    stage: impl FnOnce(gateway_local::LocalError) -> SpeechError,
) -> SpeechError {
    if matches!(source, gateway_local::LocalError::Cancelled) {
        SpeechError::InitialLoadCancelled
    } else {
        stage(source)
    }
}

/// A speech preparation, lifecycle, or request failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpeechError {
    /// The artifact store could not be opened.
    #[non_exhaustive]
    #[error("open STT artifact store")]
    Store(#[source] gateway_local::LocalError),

    /// The platform whisper.cpp runtime could not be provisioned.
    #[non_exhaustive]
    #[error("provision whisper library")]
    WhisperLibrary(#[source] gateway_local::LocalError),

    /// One model could not be provisioned.
    #[non_exhaustive]
    #[error("provision STT model {model}")]
    Artifact {
        /// Catalog name of the model that failed.
        model: String,
        /// Artifact download, confinement, or verification failure.
        #[source]
        source: gateway_local::LocalError,
    },

    /// The pinned Silero VAD model could not be provisioned.
    #[non_exhaustive]
    #[error("provision Silero VAD model")]
    Silero(#[source] gateway_local::LocalError),

    /// A final model was selected without its required interim partner.
    #[error("final STT model requires an interim model")]
    MissingInterim,

    /// The logical Realtime identity was used by one physical worker.
    #[non_exhaustive]
    #[error("model name {model} is reserved for the logical Realtime model")]
    ReservedModelName {
        /// Physical catalog name that collided with the logical identity.
        model: String,
    },

    /// A model role reached a service that does not implement it.
    #[non_exhaustive]
    #[error("model {model} has an unsupported role")]
    UnsupportedRole {
        /// Catalog name with the unsupported role.
        model: String,
    },

    /// The provisioned backend or worker pair could not be loaded.
    #[non_exhaustive]
    #[error("load STT engine")]
    Engine(#[source] gateway_stt_engine::TranscribeError),

    /// The provisioned Silero model did not open as a speech detector.
    #[non_exhaustive]
    #[error("open Silero speech detector")]
    SileroDetector(#[source] gateway_stt_engine::DetectorError),

    /// The one permitted initial speech load already ran.
    #[error("initial speech load was already attempted")]
    InitialLoadAttempted,

    /// The initial speech load was cancelled before publication.
    #[error("initial speech load was cancelled")]
    InitialLoadCancelled,

    /// Multipart framing could not be decoded.
    #[non_exhaustive]
    #[error("invalid multipart transcription request")]
    Multipart(#[source] axum::extract::multipart::MultipartError),

    /// A required form field was absent.
    #[non_exhaustive]
    #[error("missing multipart field {0}")]
    MissingField(&'static str),

    /// One form field had an unsupported value.
    #[non_exhaustive]
    #[error("invalid multipart field {field}: {value}")]
    InvalidField {
        /// Literal field name.
        field: &'static str,
        /// Refused field value.
        value: String,
    },

    /// The requested response format is unsupported.
    #[non_exhaustive]
    #[error("unsupported transcription response format {0}")]
    UnsupportedResponseFormat(String),

    /// The requested transcription language is not the one the speech
    /// engine transcribes.
    #[non_exhaustive]
    #[error("unsupported transcription language {0}; only en is transcribed")]
    UnsupportedLanguage(String),

    /// The audio file exceeded 25 MiB.
    #[error("audio file exceeds the 25 MiB limit")]
    FileTooLarge,

    /// The requested model is not loaded in the active generation.
    #[non_exhaustive]
    #[error("unknown model {0}")]
    ModelNotFound(String),

    /// WAV parsing failed.
    #[non_exhaustive]
    #[error("invalid WAV audio")]
    InvalidAudio(#[source] hound::Error),

    /// The WAV sample rate or channel count is unsupported.
    #[non_exhaustive]
    #[error("audio must be 16 kHz mono, got {sample_rate} Hz and {channels} channels")]
    UnsupportedAudio {
        /// Input sample rate.
        sample_rate: u32,
        /// Input channel count.
        channels: u16,
    },

    /// The active worker rejected otherwise valid audio.
    #[non_exhaustive]
    #[error("transcribe audio")]
    Inference(#[source] gateway_stt_engine::TranscribeError),
}

impl SpeechError {
    /// Returns the unknown physical model name for a selection failure.
    #[must_use]
    pub fn model_not_found(&self) -> Option<&str> {
        match self {
            Self::ModelNotFound(model) => Some(model),
            _ => None,
        }
    }

    /// Returns whether the caller exceeded the upload cap.
    #[must_use]
    pub fn is_file_too_large(&self) -> bool {
        matches!(self, Self::FileTooLarge)
    }

    /// Returns whether decoding failed after request validation.
    #[must_use]
    pub fn is_inference(&self) -> bool {
        matches!(self, Self::Inference(_))
    }
}

#[cfg(test)]
#[path = "artifacts-tests.rs"]
mod tests;
