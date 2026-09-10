//! One-time publication and owned admission for the single speech runtime.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use gateway_config::Config;
use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
#[cfg(feature = "test-fixtures")]
use gateway_stt_engine::ModelFactory;
use gateway_stt_engine::{DecodeMode, EnginePolicy};
use shared_progress::ProgressHandle;
use tokio_util::sync::CancellationToken;

use crate::artifacts::{self, PreparedGeneration, SpeechError};
use crate::model::SpeechModelInfo;
use crate::status::SpeechStatus;

mod lease;
mod snapshot;

#[cfg(feature = "test-fixtures")]
pub(crate) use lease::GenerationJob;
pub(crate) use lease::GenerationLease;
use snapshot::{Backend, GenerationSpec, SpeechRuntime};

#[derive(Debug)]
struct Shared {
    publication: RwLock<Publication>,
    initial_load: AtomicBool,
}

#[derive(Debug, Default)]
struct Publication {
    active: Option<Arc<SpeechRuntime>>,
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
                initial_load: AtomicBool::new(false),
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
            .map(|prepared| whisper_spec(prepared).and_then(|spec| spec.build()))
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
        self.load_scripted_shared(Arc::new(factory), policy, cancel)
    }

    /// [`Self::load_scripted`] over a shared factory, for the facade's armed
    /// scripted load.
    #[cfg(feature = "test-fixtures")]
    pub(crate) fn load_scripted_shared(
        &self,
        factory: Arc<dyn ModelFactory>,
        policy: EnginePolicy,
        cancel: &CancellationToken,
    ) -> Result<(), SpeechError> {
        self.claim_initial_load()?;
        if cancel.is_cancelled() {
            return Err(SpeechError::InitialLoadCancelled);
        }
        let runtime =
            GenerationSpec::scripted_inferred(snapshot::SharedFactory(factory), policy).build()?;
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
        // The spent claim rejects every later initial load, so this is the
        // only publication the process can perform.
        let mut publication = self
            .shared
            .publication
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        publication.active = runtime.map(Arc::new);
        publication.configured = configured;
        Ok(())
    }

    /// Stops admitting work and waits for the published runtime to drain,
    /// then retires it.
    pub(crate) fn shutdown(&self) {
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
            publication
                .active
                .take_if(|active| Arc::ptr_eq(active, &generation))
        };
        drop(generation);
        if let Some(retired) = retired
            && let Err(error) = retired.shutdown()
        {
            tracing::error!(error = %error, "speech runtime shutdown failed");
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

    /// Closes admission on the published runtime the way
    /// [`Self::shutdown`] does - cancelling the session epoch - without
    /// draining or retiring it, so a test can observe cancelled work while
    /// worker ownership persists.
    #[cfg(feature = "test-fixtures")]
    pub(crate) fn shutdown_admission(&self) {
        let runtime = self
            .shared
            .publication
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .as_ref()
            .map(Arc::clone);
        if let Some(runtime) = runtime {
            runtime.admission.shutdown();
        }
    }
}

fn whisper_spec(prepared: PreparedGeneration) -> Result<GenerationSpec, SpeechError> {
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
    Ok(GenerationSpec::new(
        Backend::Whisper,
        factory,
        policy,
        prepared.names,
        prepared.guidance,
    ))
}
