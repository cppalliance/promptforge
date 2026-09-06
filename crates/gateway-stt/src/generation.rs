//! Atomic publication and owned admission for one speech generation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::{DecodeMode, EnginePolicy, ModelFactory, SttEngine};

use crate::artifacts::{PreparedSpeech, SpeechError};
use crate::model::{ModelNames, SpeechModelInfo};
use crate::replacement::{DrainOutcome, ReplacementCoordinator, ReplacementPermit};
use crate::status::SpeechStatus;

mod lease;
mod snapshot;

#[cfg(feature = "test-fixtures")]
pub(crate) use lease::GenerationJob;
pub(crate) use lease::GenerationLease;
use snapshot::{Backend, Generation};

const GENERATION_QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(30);

/// One staged generation token whose internals remain service-owned.
#[derive(Debug)]
pub struct SpeechReplacement {
    owner: Weak<Shared>,
    generation: Option<Generation>,
    permit: ReplacementPermit,
}

#[derive(Debug)]
struct Shared {
    active: RwLock<Option<Arc<Generation>>>,
    next_generation: AtomicU64,
    changes: tokio::sync::watch::Sender<u64>,
    replacements: Arc<ReplacementCoordinator>,
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
                replacements: Arc::new(ReplacementCoordinator::default()),
            }),
        }
    }
}

impl GenerationState {
    pub(crate) fn stage(&self, prepared: PreparedSpeech) -> Result<SpeechReplacement, SpeechError> {
        self.replace_with(GENERATION_QUIESCENCE_TIMEOUT, move |id| {
            prepared
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
                    Generation::from_factory(
                        id,
                        Backend::Whisper,
                        factory,
                        policy,
                        prepared.names,
                        prepared.guidance,
                    )
                })
                .transpose()
        })
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn stage_scripted(
        &self,
        factory: impl ModelFactory,
        final_model: Option<String>,
        gpu_available: bool,
        timeout: Duration,
    ) -> Result<SpeechReplacement, SpeechError> {
        let policy = EnginePolicy::new(15, 500, gpu_available).map_err(SpeechError::Engine)?;
        self.replace_with(timeout, move |id| {
            Generation::from_factory(
                id,
                Backend::Scripted,
                factory,
                policy,
                ModelNames::new("scripted-interim".to_owned(), final_model),
                Vec::new(),
            )
            .map(Some)
        })
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn stage_loaded_scripted(
        &self,
        engine: SttEngine,
        interim: String,
        final_model: Option<String>,
        guidance: Vec<String>,
        timeout: Duration,
    ) -> Result<SpeechReplacement, SpeechError> {
        self.replace_with(timeout, move |id| {
            Ok(Some(Generation::from_engine(
                id,
                Backend::Scripted,
                engine,
                ModelNames::new(interim, final_model),
                guidance,
            )))
        })
    }

    pub(crate) fn commit(&self, replacement: SpeechReplacement) -> Result<(), SpeechError> {
        let Some(owner) = replacement.owner.upgrade() else {
            return Err(SpeechError::ReplacementOwner);
        };
        if !Arc::ptr_eq(&owner, &self.shared) {
            return Err(SpeechError::ReplacementOwner);
        }

        let mut replacement = replacement;
        let published = replacement.generation.take().map(Arc::new);
        let revision = published
            .as_ref()
            .map_or_else(|| self.next_id(), |generation| generation.id);
        let committed = replacement.permit.with_current(|| {
            let mut active = self
                .shared
                .active
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if active.is_some() {
                return false;
            }
            *active = published;
            drop(active);
            self.shared.changes.send_replace(revision);
            true
        });
        match committed {
            Some(true) => {}
            Some(false) => return Err(SpeechError::GenerationActive),
            None => return Err(SpeechError::ReplacementInvalidated),
        }
        Ok(())
    }

    pub(crate) fn shutdown(&self) {
        let _shutdown = self.shared.replacements.begin_shutdown();
        let generation = self
            .shared
            .active
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(Arc::clone);
        let Some(generation) = generation else {
            return;
        };
        generation.admission.shutdown();
        self.shared.changes.send_replace(self.next_id());
        generation.admission.wait_until_idle();
        let retired = self
            .shared
            .active
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take_if(|active| Arc::ptr_eq(active, &generation));
        drop(generation);
        if let Some(retired) = retired
            && let Err(error) = retired.shutdown()
        {
            tracing::error!(error = %error, "speech generation shutdown failed");
        }
    }

    pub(crate) fn active(&self) -> Option<GenerationLease> {
        let active = self
            .shared
            .active
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let generation = active.as_ref()?;
        let admission = generation.admission.admit()?;
        Some(GenerationLease::new(Arc::clone(generation), admission))
    }

    pub(crate) fn select(&self, name: &str) -> Option<(GenerationLease, DecodeMode)> {
        let generation = self.active()?;
        let mode = generation.select(name)?;
        Some((generation, mode))
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.shared.changes.subscribe()
    }

    pub(crate) fn status(&self) -> SpeechStatus {
        let active = self
            .shared
            .active
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        active
            .as_deref()
            .filter(|generation| generation.admission.is_open())
            .map_or_else(SpeechStatus::inactive, Generation::status)
    }

    pub(crate) fn models(&self) -> Vec<SpeechModelInfo> {
        let active = self
            .shared
            .active
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        active
            .as_deref()
            .filter(|generation| generation.admission.is_open())
            .map_or_else(Vec::new, Generation::models)
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn counts(&self) -> Option<(usize, usize)> {
        self.shared
            .active
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|generation| generation.admission.counts())
    }

    fn replace_with(
        &self,
        timeout: Duration,
        build: impl FnOnce(u64) -> Result<Option<Generation>, SpeechError>,
    ) -> Result<SpeechReplacement, SpeechError> {
        let permit = self.shared.replacements.acquire();
        self.quiesce(&permit, timeout)?;
        if !permit.is_current() {
            return Err(SpeechError::ReplacementInvalidated);
        }
        let generation = build(self.next_id())?;
        if !permit.is_current() {
            return Err(SpeechError::ReplacementInvalidated);
        }
        Ok(SpeechReplacement {
            owner: Arc::downgrade(&self.shared),
            generation,
            permit,
        })
    }

    fn quiesce(&self, permit: &ReplacementPermit, timeout: Duration) -> Result<(), SpeechError> {
        let generation = self
            .shared
            .active
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(Arc::clone);
        let Some(generation) = generation else {
            return Ok(());
        };
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(SpeechError::QuiescenceDeadline)?;
        let close = permit
            .with_current(|| {
                let close = generation.admission.close()?;
                self.shared.changes.send_replace(self.next_id());
                Some(close)
            })
            .flatten()
            .ok_or(SpeechError::ReplacementInvalidated)?;
        match generation.admission.wait_for_idle(&close, deadline) {
            DrainOutcome::TimedOut => {
                let reopened = permit
                    .with_current(|| {
                        let reopened = generation.admission.reopen(&close);
                        if reopened {
                            self.shared.changes.send_replace(self.next_id());
                        }
                        reopened
                    })
                    .unwrap_or(false);
                if reopened {
                    Err(SpeechError::QuiescenceDeadline)
                } else {
                    Err(SpeechError::ReplacementInvalidated)
                }
            }
            DrainOutcome::Invalidated => Err(SpeechError::ReplacementInvalidated),
            DrainOutcome::Idle => {
                let retired = permit
                    .with_current(|| {
                        self.shared
                            .active
                            .write()
                            .unwrap_or_else(PoisonError::into_inner)
                            .take_if(|active| Arc::ptr_eq(active, &generation))
                    })
                    .flatten()
                    .ok_or(SpeechError::ReplacementInvalidated)?;
                drop(generation);
                retired.shutdown()
            }
        }
    }

    fn next_id(&self) -> u64 {
        self.shared.next_generation.fetch_add(1, Ordering::Relaxed)
    }
}
