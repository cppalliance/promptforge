//! One complete engine runtime and its immutable published facts.

use std::sync::Arc;
use std::time::Duration;

use gateway_stt_engine::{
    DecodeMode, DecodeRequest, EnginePolicy, ModelFactory, SttEngine, TranscribeError,
};

use crate::artifacts::SpeechError;
use crate::model::{ModelNames, SpeechModelInfo};
use crate::replacement::AdmissionGate;
use crate::status::SpeechStatus;

#[derive(Debug, Clone, Copy)]
pub(super) enum Backend {
    Whisper,
    #[cfg(feature = "test-fixtures")]
    Scripted,
}

#[derive(Debug)]
struct SharedFactory(Arc<dyn ModelFactory>);

impl ModelFactory for SharedFactory {
    fn create(
        &self,
        mode: DecodeMode,
    ) -> Result<Option<Box<dyn gateway_stt_engine::Decoder>>, TranscribeError> {
        self.0.create(mode)
    }
}

#[derive(Clone, Debug)]
pub(super) struct GenerationSpec {
    backend: Backend,
    factory: Arc<dyn ModelFactory>,
    policy: EnginePolicy,
    names: ModelNames,
    guidance: Vec<String>,
    infer_scripted_final: bool,
}

impl GenerationSpec {
    pub(super) fn new(
        backend: Backend,
        factory: impl ModelFactory,
        policy: EnginePolicy,
        names: ModelNames,
        guidance: Vec<String>,
    ) -> Self {
        Self {
            backend,
            factory: Arc::new(factory),
            policy,
            names,
            guidance,
            infer_scripted_final: false,
        }
    }

    #[cfg(feature = "test-fixtures")]
    pub(super) fn scripted_inferred(factory: impl ModelFactory, policy: EnginePolicy) -> Self {
        let mut spec = Self::new(
            Backend::Scripted,
            factory,
            policy,
            ModelNames::scripted(false),
            Vec::new(),
        );
        spec.infer_scripted_final = true;
        spec
    }

    pub(super) fn build(&self, id: u64) -> Result<SpeechRuntime, SpeechError> {
        let engine = SttEngine::new(SharedFactory(Arc::clone(&self.factory)), self.policy)
            .map_err(SpeechError::Engine)?;
        let names = if self.infer_scripted_final {
            ModelNames::scripted(engine.has_final_pass())
        } else {
            self.names.clone()
        };
        Ok(SpeechRuntime {
            id,
            backend: self.backend,
            engine,
            names,
            guidance: self.guidance.clone().into(),
            admission: Arc::new(AdmissionGate::default()),
        })
    }
}

/// The smallest immutable runtime handle: engine, identity, guidance, and
/// admission ownership shared by every admitted request and worker job.
#[derive(Debug)]
pub(crate) struct SpeechRuntime {
    id: u64,
    backend: Backend,
    engine: SttEngine,
    names: ModelNames,
    pub(super) guidance: Arc<[String]>,
    pub(super) admission: Arc<AdmissionGate>,
}

impl SpeechRuntime {
    pub(super) fn shutdown(&self) -> Result<(), SpeechError> {
        self.engine.shutdown().map_err(SpeechError::Engine)
    }

    pub(super) fn status(&self) -> SpeechStatus {
        let gpu = match self.backend {
            Backend::Whisper => self.engine.gpu_transcription_available(),
            #[cfg(feature = "test-fixtures")]
            Backend::Scripted => self.engine.gpu_transcription_available(),
        };
        SpeechStatus::active(gpu, self.id)
    }

    pub(super) fn models(&self) -> Vec<SpeechModelInfo> {
        self.names.infos()
    }

    pub(super) fn select(&self, name: &str) -> Option<DecodeMode> {
        self.names.select(name)
    }

    pub(super) fn has_final_pass(&self) -> bool {
        self.engine.has_final_pass()
    }

    pub(super) fn window_samples(&self) -> usize {
        self.engine.window_samples()
    }

    pub(super) fn interval(&self) -> Duration {
        self.engine.interval()
    }

    pub(super) async fn decode(&self, request: DecodeRequest) -> Result<String, TranscribeError> {
        self.engine.decode(request).await
    }
}

/// One staged generation and the specification needed to reconstruct the
/// runtime it replaces. Replacement compatibility only; the one-time initial
/// load publishes a bare [`SpeechRuntime`].
#[derive(Debug)]
pub(super) struct Generation {
    runtime: SpeechRuntime,
    restart: GenerationSpec,
}

impl Generation {
    pub(super) fn from_factory(
        id: u64,
        backend: Backend,
        factory: impl ModelFactory,
        policy: EnginePolicy,
        names: ModelNames,
        guidance: Vec<String>,
    ) -> Result<Self, SpeechError> {
        Self::from_spec(
            id,
            GenerationSpec::new(backend, factory, policy, names, guidance),
        )
    }

    pub(super) fn from_spec(id: u64, spec: GenerationSpec) -> Result<Self, SpeechError> {
        let runtime = spec.build(id)?;
        Ok(Self {
            runtime,
            restart: spec,
        })
    }

    pub(super) fn shutdown(&self) -> Result<(), SpeechError> {
        self.runtime.shutdown()
    }

    pub(super) fn into_parts(self) -> (SpeechRuntime, GenerationSpec) {
        (self.runtime, self.restart)
    }
}
