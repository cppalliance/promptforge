//! Speech-model identity advertised by one active generation.

use gateway_stt_engine::DecodeMode;

pub(crate) const REALTIME_TRANSCRIBE_MODEL: &str = "realtime-transcribe";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReservedLogicalModelName(String);

impl ReservedLogicalModelName {
    pub(crate) fn into_name(self) -> String {
        self.0
    }
}

/// One active physical or logical speech model advertised by the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechModelInfo {
    name: String,
}

impl SpeechModelInfo {
    pub(crate) fn new(name: String) -> Self {
        Self { name }
    }

    /// Returns the caller-facing speech model name.
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
    pub(crate) fn new(
        interim: String,
        final_model: Option<String>,
    ) -> Result<Self, ReservedLogicalModelName> {
        if interim == REALTIME_TRANSCRIBE_MODEL
            || final_model.as_deref() == Some(REALTIME_TRANSCRIBE_MODEL)
        {
            return Err(ReservedLogicalModelName(
                REALTIME_TRANSCRIBE_MODEL.to_owned(),
            ));
        }
        Ok(Self {
            interim,
            final_model,
        })
    }

    pub(crate) fn scripted(has_final: bool) -> Self {
        Self {
            interim: "scripted-interim".to_owned(),
            final_model: has_final.then(|| "scripted-final".to_owned()),
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
        let mut models = Vec::with_capacity(if self.final_model.is_some() { 3 } else { 1 });
        models.push(SpeechModelInfo::new(self.interim.clone()));
        if let Some(final_model) = &self.final_model {
            models.push(SpeechModelInfo::new(final_model.clone()));
            models.push(SpeechModelInfo::new(REALTIME_TRANSCRIBE_MODEL.to_owned()));
        }
        models
    }
}

#[cfg(test)]
mod tests {
    use super::ModelNames;

    #[test]
    fn logical_name_is_reserved_from_single_physical_role() {
        assert!(ModelNames::new("realtime-transcribe".to_owned(), None).is_err());
    }

    #[test]
    fn logical_name_is_reserved_from_paired_physical_roles() {
        assert!(
            ModelNames::new(
                "physical-interim".to_owned(),
                Some("realtime-transcribe".to_owned())
            )
            .is_err()
        );
    }
}
