use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::client::parse_client_event;
use super::shared::{
    AUDIO_RATE, AUDIO_TYPE, ClientError, ClientEvent, HYPOTHESIS_INCLUDE, MODEL, OptionalNullable,
    RequiredNullable, SESSION_OBJECT, SESSION_TYPE, deserialize_required_nullable,
};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EffectiveSession {
    id: String,
    object: String,
    r#type: String,
    audio: EffectiveAudio,
    include: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectiveAudio {
    input: EffectiveInput,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectiveInput {
    format: AudioFormat,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    noise_reduction: RequiredNullable<Never>,
    transcription: EffectiveTranscription,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    turn_detection: RequiredNullable<Never>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioFormat {
    r#type: String,
    rate: u32,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectiveTranscription {
    model: String,
    prompt: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
enum Never {}

impl EffectiveSession {
    pub(in crate::realtime) fn new(id: String) -> Self {
        Self {
            id,
            object: SESSION_OBJECT.to_owned(),
            r#type: SESSION_TYPE.to_owned(),
            audio: EffectiveAudio {
                input: EffectiveInput {
                    format: AudioFormat {
                        r#type: AUDIO_TYPE.to_owned(),
                        rate: AUDIO_RATE,
                    },
                    noise_reduction: RequiredNullable::Null,
                    transcription: EffectiveTranscription {
                        model: MODEL.to_owned(),
                        prompt: String::new(),
                    },
                    turn_detection: RequiredNullable::Null,
                },
            },
            include: Vec::new(),
        }
    }

    pub(in crate::realtime) fn apply_update_text(&mut self, text: &str) -> Result<(), ClientError> {
        let event = parse_client_event(text)?;
        if let ClientEvent::SessionUpdate { patch, .. } = event {
            let mut candidate = self.clone();
            if let Some(prompt) = patch.prompt {
                candidate.audio.input.transcription.prompt = prompt;
            }
            if let Some(include) = patch.include_hypothesis {
                candidate.include = if include {
                    vec![HYPOTHESIS_INCLUDE.to_owned()]
                } else {
                    Vec::new()
                };
            }
            *self = candidate;
        }
        Ok(())
    }

    pub(in crate::realtime) fn prompt(&self) -> &str {
        &self.audio.input.transcription.prompt
    }

    pub(in crate::realtime) fn includes_hypothesis(&self) -> bool {
        !self.include.is_empty()
    }

    fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || self.object != SESSION_OBJECT
            || self.r#type != SESSION_TYPE
            || self.audio.input.format.r#type != AUDIO_TYPE
            || self.audio.input.format.rate != AUDIO_RATE
            || self.audio.input.transcription.model != MODEL
            || !self.audio.input.noise_reduction.is_null()
            || !self.audio.input.turn_detection.is_null()
            || !(self.include.is_empty()
                || matches!(self.include.as_slice(), [value] if value == HYPOTHESIS_INCLUDE))
        {
            return Err("invalid effective transcription session".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub(crate) enum ServerEvent {
    #[serde(rename = "session.created")]
    SessionCreated {
        event_id: String,
        session: EffectiveSession,
    },
    #[serde(rename = "session.updated")]
    SessionUpdated {
        event_id: String,
        session: EffectiveSession,
    },
    #[serde(rename = "input_audio_buffer.committed")]
    InputCommitted {
        event_id: String,
        item_id: String,
        #[serde(deserialize_with = "deserialize_required_nullable")]
        previous_item_id: RequiredNullable<String>,
    },
    #[serde(rename = "input_audio_buffer.cleared")]
    InputCleared { event_id: String },
    #[serde(rename = "conversation.item.created")]
    ItemCreated {
        event_id: String,
        #[serde(deserialize_with = "deserialize_required_nullable")]
        previous_item_id: RequiredNullable<String>,
        item: ConversationItem,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.delta")]
    TranscriptionDelta {
        event_id: String,
        item_id: String,
        content_index: u8,
        delta: String,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.completed")]
    TranscriptionCompleted {
        event_id: String,
        item_id: String,
        content_index: u8,
        transcript: String,
        usage: DurationUsage,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.failed")]
    TranscriptionFailed {
        event_id: String,
        item_id: String,
        content_index: u8,
        error: WireError,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.hypothesis")]
    TranscriptionHypothesis {
        event_id: String,
        item_id: String,
        content_index: u8,
        revision: u64,
        transcript: String,
        finalized: String,
        agreed: String,
        tentative: String,
        audio_start_ms: u64,
        audio_end_ms: u64,
    },
    #[serde(rename = "error")]
    Error { event_id: String, error: WireError },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationItem {
    id: String,
    r#type: String,
    status: String,
    role: String,
    content: Vec<InputAudioContent>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputAudioContent {
    r#type: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    transcript: RequiredNullable<Never>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DurationUsage {
    r#type: String,
    seconds: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireError {
    r#type: String,
    code: String,
    message: String,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    param: OptionalNullable<String>,
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    event_id: OptionalNullable<String>,
}

impl ServerEvent {
    pub(in crate::realtime) fn transcription_delta(
        event_id: String,
        item_id: String,
        delta: String,
    ) -> Self {
        Self::TranscriptionDelta {
            event_id,
            item_id,
            content_index: 0,
            delta,
        }
    }

    pub(in crate::realtime) fn from_value(value: Value) -> Result<Self, String> {
        let event: Self = serde_json::from_value(value).map_err(|error| error.to_string())?;
        event.validate()?;
        Ok(event)
    }

    fn validate(&self) -> Result<(), String> {
        let (event_id, item_id, content_index) = match self {
            Self::SessionCreated { event_id, session }
            | Self::SessionUpdated { event_id, session } => {
                session.validate()?;
                (event_id, None, None)
            }
            Self::InputCommitted {
                event_id,
                item_id,
                previous_item_id,
            } => {
                validate_optional_id(previous_item_id.as_ref().map(String::as_str))?;
                (event_id, Some(item_id), None)
            }
            Self::InputCleared { event_id } | Self::Error { event_id, .. } => {
                (event_id, None, None)
            }
            Self::ItemCreated {
                event_id,
                previous_item_id,
                item,
            } => {
                validate_optional_id(previous_item_id.as_ref().map(String::as_str))?;
                item.validate()?;
                (event_id, Some(&item.id), None)
            }
            Self::TranscriptionDelta {
                event_id,
                item_id,
                content_index,
                ..
            }
            | Self::TranscriptionCompleted {
                event_id,
                item_id,
                content_index,
                ..
            }
            | Self::TranscriptionFailed {
                event_id,
                item_id,
                content_index,
                ..
            }
            | Self::TranscriptionHypothesis {
                event_id,
                item_id,
                content_index,
                ..
            } => (event_id, Some(item_id), Some(content_index)),
        };
        validate_id(event_id)?;
        if let Some(item_id) = item_id {
            validate_id(item_id)?;
        }
        if content_index.is_some_and(|index| *index != 0) {
            return Err("content_index must be zero".to_owned());
        }
        match self {
            Self::TranscriptionCompleted { usage, .. } => usage.validate(),
            Self::TranscriptionFailed { error, .. } => {
                error.validate()?;
                if error.has_event_id() {
                    return Err("item failure must not contain a client event ID".to_owned());
                }
                Ok(())
            }
            Self::Error { error, .. } => error.validate(),
            Self::TranscriptionHypothesis {
                transcript,
                finalized,
                agreed,
                tentative,
                audio_start_ms,
                audio_end_ms,
                ..
            } if transcript != &format!("{finalized}{agreed}{tentative}")
                || audio_start_ms > audio_end_ms =>
            {
                Err("invalid hypothesis snapshot".to_owned())
            }
            _ => Ok(()),
        }
    }
}

impl ConversationItem {
    fn validate(&self) -> Result<(), String> {
        validate_id(&self.id)?;
        if self.r#type != "message"
            || self.status != "completed"
            || self.role != "user"
            || self.content.len() != 1
            || self.content[0].r#type != "input_audio"
            || !self.content[0].transcript.is_null()
        {
            return Err("invalid conversation item".to_owned());
        }
        Ok(())
    }
}

impl DurationUsage {
    fn validate(&self) -> Result<(), String> {
        if self.r#type != "duration" || !self.seconds.is_finite() || self.seconds < 0.0 {
            return Err("invalid duration usage".to_owned());
        }
        Ok(())
    }
}

impl WireError {
    fn has_event_id(&self) -> bool {
        !self.event_id.is_missing()
    }

    fn validate(&self) -> Result<(), String> {
        if self.r#type.is_empty()
            || self.code.is_empty()
            || self.message.is_empty()
            || self.param.invalid_empty()
            || self.event_id.invalid_empty()
        {
            return Err("invalid wire error".to_owned());
        }
        Ok(())
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        Err("opaque ID must not be empty".to_owned())
    } else {
        Ok(())
    }
}

fn validate_optional_id(id: Option<&str>) -> Result<(), String> {
    id.map_or(Ok(()), validate_id)
}
