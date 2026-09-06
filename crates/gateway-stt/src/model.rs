//! Physical speech-model identity inside one active generation.

use gateway_stt_engine::DecodeMode;

/// One active physical speech model advertised by the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechModelInfo {
    name: String,
}

impl SpeechModelInfo {
    pub(crate) fn new(name: String) -> Self {
        Self { name }
    }

    /// Returns the configured physical model name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ModelNames {
    interim: String,
    final_model: Option<String>,
}

impl ModelNames {
    pub(crate) fn new(interim: String, final_model: Option<String>) -> Self {
        Self {
            interim,
            final_model,
        }
    }

    pub(crate) fn select(&self, name: &str) -> Option<DecodeMode> {
        if self.interim == name {
            Some(DecodeMode::Interim)
        } else if self.final_model.as_deref() == Some(name) {
            Some(DecodeMode::Final)
        } else {
            None
        }
    }

    pub(crate) fn infos(&self) -> Vec<SpeechModelInfo> {
        let mut models = Vec::with_capacity(usize::from(self.final_model.is_some()) + 1);
        models.push(SpeechModelInfo::new(self.interim.clone()));
        if let Some(final_model) = &self.final_model {
            models.push(SpeechModelInfo::new(final_model.clone()));
        }
        models
    }
}
