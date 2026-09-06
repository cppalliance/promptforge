//! Atomic publication of one complete speech generation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};

use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::{DecodeMode, EnginePolicy, SttEngine};

use crate::artifacts::{PreparedSpeech, SpeechError};
use crate::model::{ModelNames, SpeechModelInfo};
use crate::status::SpeechStatus;

#[derive(Debug, Clone, Copy)]
enum Backend {
    Whisper,
    #[cfg(feature = "test-fixtures")]
    Scripted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admission {
    Open,
}

/// One staged generation token whose internals remain service-owned.
#[derive(Debug)]
pub struct SpeechReplacement {
    owner: Weak<Shared>,
    generation: Option<Generation>,
}

#[derive(Debug)]
pub(crate) struct Generation {
    id: u64,
    backend: Backend,
    engine: Arc<SttEngine>,
    names: ModelNames,
    guidance: Arc<[String]>,
    admission: Admission,
}

impl Generation {
    pub(crate) fn engine(&self) -> &SttEngine {
        &self.engine
    }

    pub(crate) fn engine_handle(&self) -> Arc<SttEngine> {
        Arc::clone(&self.engine)
    }

    pub(crate) fn guidance(&self) -> &[String] {
        &self.guidance
    }

    fn status(&self) -> SpeechStatus {
        let gpu = match self.backend {
            Backend::Whisper => self.engine.gpu_transcription_available(),
            #[cfg(feature = "test-fixtures")]
            Backend::Scripted => self.engine.gpu_transcription_available(),
        };
        SpeechStatus::active(gpu, self.id)
    }

    fn models(&self) -> Vec<SpeechModelInfo> {
        self.names.infos()
    }

    fn select(&self, name: &str) -> Option<DecodeMode> {
        (self.admission == Admission::Open)
            .then(|| self.names.select(name))
            .flatten()
    }
}

#[derive(Debug)]
struct Shared {
    active: RwLock<Option<Arc<Generation>>>,
    next_generation: AtomicU64,
    changes: tokio::sync::watch::Sender<u64>,
}

/// Cloneable internal state used by service methods and private handlers.
#[derive(Debug, Clone)]
pub(crate) struct GenerationState {
    shared: Arc<Shared>,
}

impl Default for GenerationState {
    fn default() -> Self {
        let (changes, _receiver) = tokio::sync::watch::channel(0);
        Self {
            shared: Arc::new(Shared {
                active: RwLock::new(None),
                next_generation: AtomicU64::new(1),
                changes,
            }),
        }
    }
}

impl GenerationState {
    pub(crate) fn stage(&self, prepared: PreparedSpeech) -> Result<SpeechReplacement, SpeechError> {
        let generation = prepared
            .generation
            .map(|prepared| {
                let backend_config = WhisperConfig::new(
                    prepared.library,
                    prepared.interim_model,
                    prepared.final_model,
                    prepared.progress,
                );
                let factory =
                    WhisperModelFactory::new(backend_config).map_err(SpeechError::Engine)?;
                let policy = EnginePolicy::new(
                    prepared.window_seconds,
                    prepared.interval_ms,
                    factory.gpu_available(),
                )
                .map_err(SpeechError::Engine)?;
                let engine = SttEngine::new(factory, policy).map_err(SpeechError::Engine)?;
                Ok(Generation {
                    id: self.next_id(),
                    backend: Backend::Whisper,
                    engine: Arc::new(engine),
                    names: prepared.names,
                    guidance: prepared.guidance.into(),
                    admission: Admission::Open,
                })
            })
            .transpose()?;
        Ok(SpeechReplacement {
            owner: Arc::downgrade(&self.shared),
            generation,
        })
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn stage_scripted(
        &self,
        engine: SttEngine,
        interim: String,
        final_model: Option<String>,
        guidance: Vec<String>,
    ) -> SpeechReplacement {
        SpeechReplacement {
            owner: Arc::downgrade(&self.shared),
            generation: Some(Generation {
                id: self.next_id(),
                backend: Backend::Scripted,
                engine: Arc::new(engine),
                names: ModelNames::new(interim, final_model),
                guidance: guidance.into(),
                admission: Admission::Open,
            }),
        }
    }

    pub(crate) fn commit(&self, replacement: SpeechReplacement) -> Result<(), SpeechError> {
        let Some(owner) = replacement.owner.upgrade() else {
            return Err(SpeechError::ReplacementOwner);
        };
        if !Arc::ptr_eq(&owner, &self.shared) {
            return Err(SpeechError::ReplacementOwner);
        }

        let published = replacement.generation.map(Arc::new);
        let revision = published
            .as_ref()
            .map_or_else(|| self.next_id(), |generation| generation.id);
        let mut active = self
            .shared
            .active
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if active.is_some() {
            return Err(SpeechError::GenerationActive);
        }
        *active = published;
        drop(active);
        self.shared.changes.send_replace(revision);
        Ok(())
    }

    pub(crate) fn shutdown(&self) {
        let generation = self
            .shared
            .active
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if generation.is_some() {
            self.shared.changes.send_replace(self.next_id());
        }
        unload(generation);
    }

    pub(crate) fn active(&self) -> Option<Arc<Generation>> {
        self.shared
            .active
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|generation| generation.admission == Admission::Open)
            .cloned()
    }

    pub(crate) fn select(&self, name: &str) -> Option<(Arc<Generation>, DecodeMode)> {
        let generation = self.active()?;
        let mode = generation.select(name)?;
        Some((generation, mode))
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.shared.changes.subscribe()
    }

    pub(crate) fn status(&self) -> SpeechStatus {
        self.active()
            .as_deref()
            .map_or_else(SpeechStatus::inactive, Generation::status)
    }

    pub(crate) fn models(&self) -> Vec<SpeechModelInfo> {
        self.active()
            .as_deref()
            .map_or_else(Vec::new, Generation::models)
    }

    fn next_id(&self) -> u64 {
        self.shared.next_generation.fetch_add(1, Ordering::Relaxed)
    }
}

fn unload(generation: Option<Arc<Generation>>) {
    let Some(generation) = generation else {
        return;
    };
    let engine = Arc::clone(&generation.engine);
    while Arc::strong_count(&generation) > 1 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    drop(generation);
    while Arc::strong_count(&engine) > 1 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    drop(engine);
}
