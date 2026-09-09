//! Verified speech artifacts and facade error vocabulary.

use std::path::PathBuf;

use gateway_config::{Config, SttRole};
use gateway_local::artifacts::ArtifactStore;
use shared_progress::ProgressHandle;

use crate::model::{ModelNames, REALTIME_TRANSCRIBE_MODEL};

/// Verified artifacts and policy for a generation that has not started workers.
#[derive(Debug)]
pub struct PreparedSpeech {
    pub(crate) generation: Option<PreparedGeneration>,
}

#[derive(Debug)]
pub(crate) struct PreparedGeneration {
    pub(crate) library: PathBuf,
    pub(crate) interim_model: PathBuf,
    pub(crate) final_model: Option<PathBuf>,
    pub(crate) names: ModelNames,
    pub(crate) guidance: Vec<String>,
    pub(crate) window_seconds: u64,
    pub(crate) interval_ms: u64,
    pub(crate) progress: Option<ProgressHandle>,
}

#[derive(Debug, Default)]
struct ProvisionedModels {
    interim: Option<(String, PathBuf)>,
    final_model: Option<(String, PathBuf)>,
}

pub(crate) fn prepare(
    config: &Config,
    progress: Option<&ProgressHandle>,
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
    let library_progress = progress.map(|handle| handle.child("whisper-library", 1.0));
    let library = store
        .provision_whisper_library(library_progress.as_ref())
        .map_err(SpeechError::WhisperLibrary)?;
    let models = provision_models(config, &store, progress)?;
    let Some((interim_name, interim_model)) = models.interim else {
        return Err(SpeechError::MissingInterim);
    };
    let capture = config.stt().cloned().unwrap_or_default();
    let (final_name, final_model) = models
        .final_model
        .map_or((None, None), |(name, path)| (Some(name), Some(path)));

    Ok(PreparedSpeech {
        generation: Some(PreparedGeneration {
            library,
            interim_model,
            final_model,
            names: ModelNames::new(interim_name, final_name).map_err(|error| {
                SpeechError::ReservedModelName {
                    model: error.into_name(),
                }
            })?,
            guidance: capture.vocabulary().to_vec(),
            window_seconds: capture.window_seconds(),
            interval_ms: capture.interval_ms(),
            progress: progress.map(|handle| handle.child("engine", 1.0)),
        }),
    })
}

fn provision_models(
    config: &Config,
    store: &ArtifactStore,
    progress: Option<&ProgressHandle>,
) -> Result<ProvisionedModels, SpeechError> {
    let mut provisioned = ProvisionedModels::default();
    for model in config.stt_models() {
        let model_progress = progress.map(|handle| handle.child(model.name(), 4.0));
        let path = store
            .ensure_model_with_progress(model.source(), model.sha256(), model_progress.as_ref())
            .map_err(|source| SpeechError::Artifact {
                model: model.name().to_owned(),
                source,
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

    /// A final model was selected without its required interim partner.
    #[error("final STT model requires an interim model")]
    MissingInterim,

    /// The logical Realtime identity was used by one physical worker.
    #[non_exhaustive]
    #[error("STT model name {model} is reserved for the logical Realtime model")]
    ReservedModelName {
        /// Physical catalog name that collided with the logical identity.
        model: String,
    },

    /// A future role reached a service that does not implement it.
    #[non_exhaustive]
    #[error("STT model {model} has an unsupported role")]
    UnsupportedRole {
        /// Catalog name carrying the unsupported role.
        model: String,
    },

    /// The provisioned backend or worker pair could not be loaded.
    #[non_exhaustive]
    #[error("load STT engine")]
    Engine(#[source] gateway_stt_engine::TranscribeError),

    /// The one permitted initial speech load already ran.
    #[error("initial speech load was already attempted")]
    InitialLoadAttempted,

    /// The initial speech load was cancelled before publication.
    #[error("initial speech load was cancelled")]
    InitialLoadCancelled,

    /// A replacement token belongs to another service.
    #[error("speech replacement belongs to another service")]
    ReplacementOwner,

    /// A replacement was committed while a generation was still active.
    #[error("an active speech generation must be shut down before replacement")]
    GenerationActive,

    /// Old-generation ownership did not drain before replacement's deadline.
    #[error("speech generation quiescence deadline expired")]
    QuiescenceDeadline,

    /// Shutdown invalidated a replacement before it could publish.
    #[error("speech replacement was invalidated by shutdown")]
    ReplacementInvalidated,

    /// Reconstructing the old generation failed after a determinate replacement failure.
    #[error("speech replacement failed ({failure}); reconstruct old generation ({rollback})")]
    Rollback {
        /// The determinate failure that required reconstruction.
        failure: Box<SpeechError>,
        /// The failure returned while reconstructing the old specification.
        rollback: Box<SpeechError>,
    },

    /// Multipart framing could not be decoded.
    #[non_exhaustive]
    #[error("invalid multipart transcription request")]
    Multipart(#[source] axum::extract::multipart::MultipartError),

    /// A required form field was absent.
    #[non_exhaustive]
    #[error("missing multipart field {0}")]
    MissingField(&'static str),

    /// One form field carried an unsupported value.
    #[non_exhaustive]
    #[error("invalid multipart field {field}: {value}")]
    InvalidField {
        /// Literal field name.
        field: &'static str,
        /// Refused field value.
        value: String,
    },

    /// The requested response format is not implemented.
    #[non_exhaustive]
    #[error("unsupported transcription response format {0}")]
    UnsupportedResponseFormat(String),

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
    /// Returns whether worker construction exceeded a deadline and left a
    /// non-preemptible native call running.
    #[must_use]
    pub fn is_non_preemptible_startup_timeout(&self) -> bool {
        match self {
            Self::Engine(error) => error.is_non_preemptible_startup_timeout(),
            Self::Rollback { failure, rollback } => {
                failure.is_non_preemptible_startup_timeout()
                    || rollback.is_non_preemptible_startup_timeout()
            }
            _ => false,
        }
    }

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
mod tests {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    use super::*;

    fn selected(source: &str, sha256: Option<&str>) -> Config {
        let pin = sha256.map_or_else(String::new, |pin| format!("sha256 = \"{pin}\"\n"));
        let catalog = Config::from_toml_str(&format!(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
             [workshop]\n\
             [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {source:?}\n\
             {pin}vram_gb = 1.0\n\
             [[profile]]\nname = \"work\"\nmodels = [\"speech\"]\n"
        ))
        .expect("catalog parses");
        catalog
            .select_profile(&gateway_config::ProfileName::parse("work").expect("name"))
            .expect("profile selects")
    }

    #[test]
    fn a_pinned_model_rejects_the_wrong_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"model bytes").expect("fixture writes");
        let config = selected(&model.display().to_string(), Some(&"0".repeat(64)));
        let store = ArtifactStore::new(dir.path().join("cache")).expect("store builds");
        let error = provision_models(&config, &store, None).expect_err("bad pin must fail");
        assert!(matches!(error, SpeechError::Artifact { .. }));
    }

    #[test]
    fn an_unpinned_local_model_provisions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"model bytes").expect("fixture writes");
        let config = selected(&model.display().to_string(), None);
        let store = ArtifactStore::new(dir.path().join("cache")).expect("store builds");
        let provisioned = provision_models(&config, &store, None).expect("unpinned path works");
        assert_eq!(
            provisioned.interim.as_ref().map(|(_, path)| path),
            Some(&model)
        );
    }

    #[test]
    fn a_pinned_model_accepts_the_matching_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"model bytes").expect("fixture writes");
        let mut pin = String::with_capacity(64);
        for byte in Sha256::digest(b"model bytes") {
            write!(&mut pin, "{byte:02x}").expect("writing to String is infallible");
        }
        let config = selected(&model.display().to_string(), Some(&pin));
        let store = ArtifactStore::new(dir.path().join("cache")).expect("store builds");
        let provisioned = provision_models(&config, &store, None).expect("matching pin works");
        assert_eq!(
            provisioned.interim.as_ref().map(|(_, path)| path),
            Some(&model)
        );
    }
}
