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
pub(super) struct Generation {
    pub(super) id: u64,
    backend: Backend,
    engine: SttEngine,
    names: ModelNames,
    pub(super) guidance: Arc<[String]>,
    pub(super) admission: Arc<AdmissionGate>,
}

impl Generation {
    pub(super) fn from_engine(
        id: u64,
        backend: Backend,
        engine: SttEngine,
        names: ModelNames,
        guidance: Vec<String>,
    ) -> Self {
        Self {
            id,
            backend,
            engine,
            names,
            guidance: guidance.into(),
            admission: Arc::new(AdmissionGate::default()),
        }
    }

    pub(super) fn from_factory(
        id: u64,
        backend: Backend,
        factory: impl ModelFactory,
        policy: EnginePolicy,
        names: ModelNames,
        guidance: Vec<String>,
    ) -> Result<Self, SpeechError> {
        let engine = SttEngine::new(factory, policy).map_err(SpeechError::Engine)?;
        Ok(Self::from_engine(id, backend, engine, names, guidance))
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
