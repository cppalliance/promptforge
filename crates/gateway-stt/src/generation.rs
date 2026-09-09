//! One-time publication and owned admission for the single speech runtime.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use gateway_config::Config;
use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
#[cfg(feature = "test-fixtures")]
use gateway_stt_engine::ModelFactory;
use gateway_stt_engine::{DecodeMode, EnginePolicy};
use shared_progress::ProgressHandle;
use tokio_util::sync::CancellationToken;

use crate::artifacts::{self, PreparedGeneration, PreparedSpeech, SpeechError};
#[cfg(feature = "test-fixtures")]
use crate::model::ModelNames;
use crate::model::SpeechModelInfo;
use crate::replacement::{DrainOutcome, ReplacementCoordinator, ReplacementPermit};
use crate::status::SpeechStatus;

mod lease;
mod snapshot;

#[cfg(feature = "test-fixtures")]
pub(crate) use lease::GenerationJob;
pub(crate) use lease::GenerationLease;
use snapshot::{Backend, Generation, GenerationSpec, SpeechRuntime};

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
    initial_load: AtomicBool,
    replacements: Arc<ReplacementCoordinator>,
}

#[derive(Debug, Default)]
struct Publication {
    active: Option<Arc<SpeechRuntime>>,
    /// Reconstruction specification for `active`; only the compatibility
    /// replacement path records one, so a one-time initial publication has
    /// none.
    restart: Option<GenerationSpec>,
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
                initial_load: AtomicBool::new(false),
                replacements: Arc::new(ReplacementCoordinator::default()),
            }),
        }
    }
}

impl GenerationState {
    /// Claims the one initial load, builds the boot runtime, and publishes it.
    ///
    /// The attempt is spent whether it publishes, fails, or is cancelled, so
    /// speech remains unavailable until process restart after any outcome
    /// other than success.
    pub(crate) fn load_initial(
        &self,
        config: &Config,
        progress: Option<&ProgressHandle>,
        cancel: &CancellationToken,
    ) -> Result<(), SpeechError> {
        self.claim_initial_load()?;
        if cancel.is_cancelled() {
            return Err(SpeechError::InitialLoadCancelled);
        }
        let prepared = artifacts::prepare(config, progress)?;
        if cancel.is_cancelled() {
            return Err(SpeechError::InitialLoadCancelled);
        }
        let runtime = prepared
            .generation
            .map(|prepared| {
                whisper_spec(prepared, None).and_then(|spec| spec.build(self.next_id()))
            })
            .transpose()?;
        self.publish_initial(runtime, cancel)
    }

    /// Claims the one initial load and publishes deterministic scripted workers.
    #[cfg(feature = "test-fixtures")]
    pub(crate) fn load_scripted(
        &self,
        factory: impl ModelFactory,
        policy: EnginePolicy,
        cancel: &CancellationToken,
    ) -> Result<(), SpeechError> {
        self.claim_initial_load()?;
        if cancel.is_cancelled() {
            return Err(SpeechError::InitialLoadCancelled);
        }
        let runtime = GenerationSpec::scripted_inferred(factory, policy).build(self.next_id())?;
        self.publish_initial(Some(runtime), cancel)
    }

    fn claim_initial_load(&self) -> Result<(), SpeechError> {
        if self.shared.initial_load.swap(true, Ordering::AcqRel) {
            return Err(SpeechError::InitialLoadAttempted);
        }
        Ok(())
    }

    fn publish_initial(
        &self,
        runtime: Option<SpeechRuntime>,
        cancel: &CancellationToken,
    ) -> Result<(), SpeechError> {
        if cancel.is_cancelled() {
            if let Some(runtime) = &runtime
                && let Err(error) = runtime.shutdown()
            {
                tracing::error!(error = %error, "cancelled initial speech load cleanup failed");
            }
            return Err(SpeechError::InitialLoadCancelled);
        }
        let configured = runtime.is_some();
        // The facade is still empty here: the spent claim rejects every later
        // initial load and the staging guard rejects the compatibility
        // replacement path, so this is the only publication the process can
        // perform.
        let mut publication = self
            .shared
            .publication
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        publication.active = runtime.map(Arc::new);
        publication.configured = configured;
        Ok(())
    }

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
                    whisper_spec(prepared, Some(startup_timeout))
                        .and_then(|spec| Generation::from_spec(id, spec))
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
            Generation::from_spec(id, GenerationSpec::scripted_inferred(factory, policy)).map(Some)
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
        let published = replacement.generation.take().map(Generation::into_parts);
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
            if let Some((runtime, restart)) = published {
                publication.active = Some(Arc::new(runtime));
                publication.restart = Some(restart);
            } else {
                publication.active = None;
                publication.restart = None;
            }
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
        let retired = {
            let mut publication = self
                .shared
                .publication
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            let retired = publication
                .active
                .take_if(|active| Arc::ptr_eq(active, &generation));
            if retired.is_some() {
                publication.restart = None;
            }
            retired
        };
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
        let runtime = publication.active.as_ref()?;
        let admission = runtime.admission.admit()?;
        Some(GenerationLease::new(Arc::clone(runtime), admission))
    }

    pub(crate) fn select(&self, name: &str) -> Option<(GenerationLease, DecodeMode)> {
        let runtime = self.active()?;
        let mode = runtime.select(name)?;
        Some((runtime, mode))
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
            .filter(|runtime| runtime.admission.is_open())
            .map_or_else(
                || SpeechStatus::unready(publication.configured),
                SpeechRuntime::status,
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
            .filter(|runtime| runtime.admission.is_open())
            .map_or_else(Vec::new, SpeechRuntime::models)
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn counts(&self) -> Option<(usize, usize)> {
        self.shared
            .publication
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .as_ref()
            .map(|runtime| runtime.admission.counts())
    }

    #[cfg(feature = "test-fixtures")]
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
        if self.shared.initial_load.load(Ordering::Acquire) {
            return Err(SpeechError::InitialLoadAttempted);
        }
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
                let retired = permit
                    .with_current(|| {
                        let mut publication = self
                            .shared
                            .publication
                            .write()
                            .unwrap_or_else(PoisonError::into_inner);
                        publication
                            .active
                            .take_if(|active| Arc::ptr_eq(active, &generation))
                            .map(|retired| (retired, publication.restart.take()))
                    })
                    .flatten()
                    .ok_or(SpeechError::ReplacementInvalidated)?;
                let (retired, restart) = retired;
                drop(generation);
                retired.shutdown()?;
                let Some(restart) = restart else {
                    // The staging guard refuses replacement once the one-time
                    // initial load ran, so a compatibility-published runtime
                    // always carries its reconstruction specification here.
                    return Err(SpeechError::ReplacementInvalidated);
                };
                Ok(Some(restart))
            }
        }
    }

    fn next_id(&self) -> u64 {
        self.shared.next_generation.fetch_add(1, Ordering::Relaxed)
    }
}

fn whisper_spec(
    prepared: PreparedGeneration,
    startup_timeout: Option<Duration>,
) -> Result<GenerationSpec, SpeechError> {
    let backend_config = WhisperConfig::new(
        prepared.library,
        prepared.interim_model,
        prepared.final_model,
        prepared.progress,
    );
    let factory = WhisperModelFactory::new(backend_config).map_err(SpeechError::Engine)?;
    let policy = EnginePolicy::new(
        prepared.window_seconds,
        prepared.interval_ms,
        factory.gpu_available(),
    )
    .map_err(SpeechError::Engine)?;
    let policy = match startup_timeout {
        Some(timeout) => policy.with_startup_timeout(timeout),
        None => policy,
    };
    Ok(GenerationSpec::new(
        Backend::Whisper,
        factory,
        policy,
        prepared.names,
        prepared.guidance,
    ))
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
            publication.restart = Some(rollback.clone());
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
