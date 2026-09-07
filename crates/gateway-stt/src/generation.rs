//! Atomic publication and owned admission for one speech generation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::{DecodeMode, EnginePolicy, ModelFactory};

use crate::artifacts::{PreparedSpeech, SpeechError};
use crate::model::{ModelNames, SpeechModelInfo};
use crate::replacement::{DrainOutcome, ReplacementCoordinator, ReplacementPermit};
use crate::status::SpeechStatus;

mod lease;
mod snapshot;

#[cfg(feature = "test-fixtures")]
pub(crate) use lease::GenerationJob;
pub(crate) use lease::GenerationLease;
use snapshot::{Backend, Generation, GenerationSpec};

const GENERATION_QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(30);

/// One staged generation token whose internals remain service-owned.
#[derive(Debug)]
pub struct SpeechReplacement {
    owner: Weak<Shared>,
    generation: Option<Generation>,
    rollback: Option<GenerationSpec>,
    permit: ReplacementPermit,
}

#[derive(Debug)]
struct Shared {
    publication: RwLock<Publication>,
    next_generation: AtomicU64,
    replacements: Arc<ReplacementCoordinator>,
}

#[derive(Debug, Default)]
struct Publication {
    active: Option<Arc<Generation>>,
    configured: bool,
}

/// Cloneable internal state used by service methods and private handlers.
#[derive(Debug, Clone)]
pub(crate) struct GenerationState {
    shared: Arc<Shared>,
}

impl Default for GenerationState {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                publication: RwLock::new(Publication::default()),
                next_generation: AtomicU64::new(1),
                replacements: Arc::new(ReplacementCoordinator::default()),
            }),
        }
    }
}

impl GenerationState {
    pub(crate) fn stage(&self, prepared: PreparedSpeech) -> Result<SpeechReplacement, SpeechError> {
        let deadline = Instant::now()
            .checked_add(GENERATION_QUIESCENCE_TIMEOUT)
            .ok_or(SpeechError::QuiescenceDeadline)?;
        self.stage_until(prepared, deadline)
    }

    pub(crate) fn stage_until(
        &self,
        prepared: PreparedSpeech,
        deadline: Instant,
    ) -> Result<SpeechReplacement, SpeechError> {
        self.replace_with_until(deadline, move |id, startup_timeout| {
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
                    .map_err(SpeechError::Engine)?
                    .with_startup_timeout(startup_timeout);
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
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(SpeechError::QuiescenceDeadline)?;
        let policy = EnginePolicy::new(15, 500, gpu_available).map_err(SpeechError::Engine)?;
        self.replace_with_until(deadline, move |id, startup_timeout| {
            Generation::from_factory(
                id,
                Backend::Scripted,
                factory,
                policy.with_startup_timeout(startup_timeout),
                ModelNames::scripted(final_model.is_some()),
                Vec::new(),
            )
            .map(Some)
        })
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn stage_scripted_with_policy(
        &self,
        factory: impl ModelFactory,
        policy: EnginePolicy,
    ) -> Result<SpeechReplacement, SpeechError> {
        self.replace_with(GENERATION_QUIESCENCE_TIMEOUT, move |id| {
            GenerationSpec::scripted_inferred(factory, policy)
                .build(id)
                .map(Some)
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
        let configured = published.is_some();
        let committed = replacement.permit.with_current(|| {
            let mut publication = self
                .shared
                .publication
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if publication.active.is_some() {
                return false;
            }
            publication.active = published;
            publication.configured = configured;
            true
        });
        match committed {
            Some(true) => replacement.rollback = None,
            Some(false) => return Err(SpeechError::GenerationActive),
            None => return Err(SpeechError::ReplacementInvalidated),
        }
        Ok(())
    }

    pub(crate) fn abort(&self, mut replacement: SpeechReplacement) -> Result<(), SpeechError> {
        let Some(owner) = replacement.owner.upgrade() else {
            return Err(SpeechError::ReplacementOwner);
        };
        if !Arc::ptr_eq(&owner, &self.shared) {
            return Err(SpeechError::ReplacementOwner);
        }
        replacement.rollback()
    }

    pub(crate) fn shutdown(&self) {
        let _shutdown = self.shared.replacements.begin_shutdown();
        let generation = self
            .shared
            .publication
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .as_ref()
            .map(Arc::clone);
        let Some(generation) = generation else {
            return;
        };
        generation.admission.shutdown();
        generation.admission.wait_until_idle();
        let retired = self
            .shared
            .publication
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .take_if(|active| Arc::ptr_eq(active, &generation));
        drop(generation);
        if let Some(retired) = retired
            && let Err(error) = retired.shutdown()
        {
            tracing::error!(error = %error, "speech generation shutdown failed");
        }
    }

    pub(crate) fn active(&self) -> Option<GenerationLease> {
        let publication = self
            .shared
            .publication
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let generation = publication.active.as_ref()?;
        let admission = generation.admission.admit()?;
        Some(GenerationLease::new(Arc::clone(generation), admission))
    }

    pub(crate) fn select(&self, name: &str) -> Option<(GenerationLease, DecodeMode)> {
        let generation = self.active()?;
        let mode = generation.select(name)?;
        Some((generation, mode))
    }

    pub(crate) fn status(&self) -> SpeechStatus {
        let publication = self
            .shared
            .publication
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        publication
            .active
            .as_deref()
            .filter(|generation| generation.admission.is_open())
            .map_or_else(
                || SpeechStatus::unready(publication.configured),
                Generation::status,
            )
    }

    pub(crate) fn models(&self) -> Vec<SpeechModelInfo> {
        let publication = self
            .shared
            .publication
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        publication
            .active
            .as_deref()
            .filter(|generation| generation.admission.is_open())
            .map_or_else(Vec::new, Generation::models)
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn counts(&self) -> Option<(usize, usize)> {
        self.shared
            .publication
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .as_ref()
            .map(|generation| generation.admission.counts())
    }

    fn replace_with(
        &self,
        timeout: Duration,
        build: impl FnOnce(u64) -> Result<Option<Generation>, SpeechError>,
    ) -> Result<SpeechReplacement, SpeechError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(SpeechError::QuiescenceDeadline)?;
        self.replace_with_until(deadline, move |id, _startup_timeout| build(id))
    }

    fn replace_with_until(
        &self,
        deadline: Instant,
        build: impl FnOnce(u64, Duration) -> Result<Option<Generation>, SpeechError>,
    ) -> Result<SpeechReplacement, SpeechError> {
        let permit = self.shared.replacements.acquire();
        let rollback = self.quiesce(&permit, deadline)?;
        if !permit.is_current() {
            return Err(SpeechError::ReplacementInvalidated);
        }
        let startup_timeout = deadline.saturating_duration_since(Instant::now());
        let generation = match build(self.next_id(), startup_timeout) {
            Ok(generation) => generation,
            Err(failure) => {
                if failure.is_non_preemptible_startup_timeout() {
                    return Err(failure);
                }
                if let Some(rollback) = rollback
                    && let Err(rollback) = restore_generation(&self.shared, &permit, &rollback)
                {
                    return Err(SpeechError::Rollback {
                        failure: Box::new(failure),
                        rollback: Box::new(rollback),
                    });
                }
                return Err(failure);
            }
        };
        if !permit.is_current() {
            return Err(SpeechError::ReplacementInvalidated);
        }
        Ok(SpeechReplacement {
            owner: Arc::downgrade(&self.shared),
            generation,
            rollback,
            permit,
        })
    }

    fn quiesce(
        &self,
        permit: &ReplacementPermit,
        deadline: Instant,
    ) -> Result<Option<GenerationSpec>, SpeechError> {
        let generation = self
            .shared
            .publication
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .as_ref()
            .map(Arc::clone);
        let Some(generation) = generation else {
            return Ok(None);
        };
        let close = permit
            .with_current(|| {
                let close = generation.admission.close()?;
                Some(close)
            })
            .flatten()
            .ok_or(SpeechError::ReplacementInvalidated)?;
        match generation.admission.wait_for_idle(&close, deadline) {
            DrainOutcome::TimedOut => {
                let reopened = permit
                    .with_current(|| generation.admission.reopen(&close))
                    .unwrap_or(false);
                if reopened {
                    Err(SpeechError::QuiescenceDeadline)
                } else {
                    Err(SpeechError::ReplacementInvalidated)
                }
            }
            DrainOutcome::Invalidated => Err(SpeechError::ReplacementInvalidated),
            DrainOutcome::Idle => {
                let restart = generation.restart_spec();
                let retired = permit
                    .with_current(|| {
                        self.shared
                            .publication
                            .write()
                            .unwrap_or_else(PoisonError::into_inner)
                            .active
                            .take_if(|active| Arc::ptr_eq(active, &generation))
                    })
                    .flatten()
                    .ok_or(SpeechError::ReplacementInvalidated)?;
                drop(generation);
                retired.shutdown()?;
                Ok(Some(restart))
            }
        }
    }

    fn next_id(&self) -> u64 {
        self.shared.next_generation.fetch_add(1, Ordering::Relaxed)
    }
}

fn restore_generation(
    shared: &Arc<Shared>,
    permit: &ReplacementPermit,
    rollback: &GenerationSpec,
) -> Result<(), SpeechError> {
    if !permit.is_current() {
        return Err(SpeechError::ReplacementInvalidated);
    }
    let id = shared.next_generation.fetch_add(1, Ordering::Relaxed);
    let generation = Arc::new(rollback.build(id)?);
    let restored = permit
        .with_current(|| {
            let mut publication = shared
                .publication
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if publication.active.is_some() {
                return false;
            }
            publication.active = Some(generation);
            true
        })
        .unwrap_or(false);
    if !restored {
        return Err(SpeechError::ReplacementInvalidated);
    }
    Ok(())
}

impl SpeechReplacement {
    fn rollback(&mut self) -> Result<(), SpeechError> {
        let Some(owner) = self.owner.upgrade() else {
            return Err(SpeechError::ReplacementOwner);
        };
        let cleanup = self
            .generation
            .take()
            .map_or(Ok(()), |generation| generation.shutdown());
        let reconstruction = self.rollback.take().map_or(Ok(()), |rollback| {
            restore_generation(&owner, &self.permit, &rollback)
        });
        match (cleanup, reconstruction) {
            (Err(failure), Err(rollback)) => Err(SpeechError::Rollback {
                failure: Box::new(failure),
                rollback: Box::new(rollback),
            }),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

impl Drop for SpeechReplacement {
    fn drop(&mut self) {
        if (self.generation.is_some() || self.rollback.is_some())
            && let Err(error) = self.rollback()
        {
            tracing::error!(error = %error, "speech replacement rollback failed");
        }
    }
}
