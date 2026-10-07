//! The one complete engine runtime and its immutable published facts.

use std::sync::Arc;
use std::time::Duration;

use gateway_stt_engine::{
    DecodeMode, DecodeOutput, DecodeRequest, EnginePolicy, ModelFactory, SttEngine, TranscribeError,
};

use crate::admission::AdmissionGate;
use crate::artifacts::{SileroModel, SpeechError};
use crate::model::{ModelNames, SpeechModelInfo};
use crate::status::SpeechStatus;

#[derive(Debug, Clone, Copy)]
pub(super) enum Backend {
    Whisper,
    #[cfg(feature = "test-fixtures")]
    Scripted,
}

#[derive(Debug)]
pub(super) struct SharedFactory(pub(super) Arc<dyn ModelFactory>);

impl ModelFactory for SharedFactory {
    fn create(
        &self,
        mode: DecodeMode,
    ) -> Result<Option<Box<dyn gateway_stt_engine::Decoder>>, TranscribeError> {
        self.0.create(mode)
    }
}

#[derive(Debug)]
pub(super) struct GenerationSpec {
    backend: Backend,
    factory: Arc<dyn ModelFactory>,
    policy: EnginePolicy,
    names: ModelNames,
    guidance: Vec<String>,
    silero: Option<SileroModel>,
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
            silero: None,
            infer_scripted_final: false,
        }
    }

    pub(super) fn with_silero(mut self, silero: SileroModel) -> Self {
        self.silero = Some(silero);
        self
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

    pub(super) fn build(&self) -> Result<SpeechRuntime, SpeechError> {
        let engine = SttEngine::new(SharedFactory(Arc::clone(&self.factory)), self.policy)
            .map_err(SpeechError::Engine)?;
        let names = if self.infer_scripted_final {
            ModelNames::scripted(engine.has_final_pass())
        } else {
            self.names.clone()
        };
        Ok(SpeechRuntime {
            backend: self.backend,
            engine,
            names,
            guidance: self.guidance.clone().into(),
            silero: self.silero.clone(),
            admission: Arc::new(AdmissionGate::default()),
        })
    }
}

/// The smallest immutable runtime handle: engine, identity, guidance, the
/// Silero model, and admission ownership shared by every admitted request
/// and worker job.
#[derive(Debug)]
pub(super) struct SpeechRuntime {
    backend: Backend,
    engine: SttEngine,
    names: ModelNames,
    pub(super) guidance: Arc<[String]>,
    /// `None` for a backend that provisions no Silero model.
    pub(super) silero: Option<SileroModel>,
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
        SpeechStatus::active(gpu)
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

    pub(super) async fn decode(
        &self,
        request: DecodeRequest,
    ) -> Result<DecodeOutput, TranscribeError> {
        self.engine.decode(request).await
    }
}
