//! One complete engine generation and its immutable published facts.

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
            ModelNames::new("scripted-interim".to_owned(), None),
            Vec::new(),
        );
        spec.infer_scripted_final = true;
        spec
    }

    pub(super) fn build(&self, id: u64) -> Result<Generation, SpeechError> {
        let engine = SttEngine::new(SharedFactory(Arc::clone(&self.factory)), self.policy)
            .map_err(SpeechError::Engine)?;
        let names = if self.infer_scripted_final {
            ModelNames::new(
                "scripted-interim".to_owned(),
                engine.has_final_pass().then(|| "scripted-final".to_owned()),
            )
        } else {
            self.names.clone()
        };
        Ok(Generation {
            id,
            backend: self.backend,
            engine,
            names,
            guidance: self.guidance.clone().into(),
            admission: Arc::new(AdmissionGate::default()),
            restart: self.clone(),
        })
    }
}

#[derive(Debug)]
pub(super) struct Generation {
    pub(super) id: u64,
    backend: Backend,
    engine: SttEngine,
    names: ModelNames,
    pub(super) guidance: Arc<[String]>,
    pub(super) admission: Arc<AdmissionGate>,
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
        GenerationSpec::new(backend, factory, policy, names, guidance).build(id)
    }

    pub(super) fn restart_spec(&self) -> GenerationSpec {
        self.restart.clone()
    }

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
