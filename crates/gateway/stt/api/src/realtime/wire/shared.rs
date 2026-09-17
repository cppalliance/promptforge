use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub(super) const SESSION_OBJECT: &str = "realtime.transcription_session";
pub(super) const SESSION_TYPE: &str = "transcription";
pub(super) const AUDIO_TYPE: &str = "audio/pcm";
pub(super) const AUDIO_RATE: u32 = 24_000;
pub(super) const MODEL: &str = "realtime-transcribe";
pub(super) const HYPOTHESIS_INCLUDE: &str = "item.input_audio_transcription.hypothesis";

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::realtime) enum ClientEvent {
    SessionUpdate {
        event_id: Option<String>,
        patch: SessionPatch,
    },
    Append {
        event_id: Option<String>,
        audio: String,
    },
    Commit {
        event_id: Option<String>,
    },
    Clear {
        event_id: Option<String>,
    },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::realtime) struct SessionPatch {
    pub(super) prompt: Option<String>,
    pub(super) include_hypothesis: Option<bool>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum Correlation {
    Omitted,
    Null,
    Client(String),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ClientError {
    kind: &'static str,
    code: &'static str,
    message: String,
    param: Option<String>,
    correlation: Correlation,
}

impl ClientError {
    pub(super) fn new(
        code: &'static str,
        message: impl Into<String>,
        param: Option<&str>,
        correlation: Correlation,
    ) -> Self {
        Self {
            kind: "invalid_request_error",
            code,
            message: message.into(),
            param: param.map(str::to_owned),
            correlation,
        }
    }

    pub(in crate::realtime) fn into_server_event(self, event_id: &str) -> Value {
        let mut error = Map::new();
        error.insert("type".to_owned(), Value::from(self.kind));
        error.insert("code".to_owned(), Value::from(self.code));
        error.insert("message".to_owned(), Value::from(self.message));
        if let Some(param) = self.param {
            error.insert("param".to_owned(), Value::from(param));
        }
        match self.correlation {
            Correlation::Omitted => {}
            Correlation::Null => {
                error.insert("event_id".to_owned(), Value::Null);
            }
            Correlation::Client(client_id) => {
                error.insert("event_id".to_owned(), Value::from(client_id));
            }
        }
        serde_json::json!({"event_id": event_id, "type": "error", "error": error})
    }

    pub(in crate::realtime) fn request(
        code: &'static str,
        message: &'static str,
        param: Option<&'static str>,
        client_event_id: Option<String>,
    ) -> Self {
        Self::new(
            code,
            message,
            param,
            client_event_id.map_or(Correlation::Omitted, Correlation::Client),
        )
    }

    pub(in crate::realtime) fn overload(
        code: &'static str,
        message: &'static str,
        param: Option<&'static str>,
        client_event_id: Option<String>,
    ) -> Self {
        let mut error = Self::request(code, message, param, client_event_id);
        error.kind = "overload_error";
        error
    }

    pub(in crate::realtime) fn server(
        code: &'static str,
        message: &'static str,
        param: Option<&'static str>,
        client_event_id: Option<String>,
    ) -> Self {
        let mut error = Self::request(code, message, param, client_event_id);
        error.kind = "server_error";
        error
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ClientError {}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum RequiredNullable<T> {
    Null,
    Value(T),
}

impl<T> RequiredNullable<T> {
    #[cfg(test)]
    pub(super) fn as_ref(&self) -> Option<&T> {
        match self {
            Self::Null => None,
            Self::Value(value) => Some(value),
        }
    }

    #[cfg(test)]
    pub(super) fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

impl<T: Serialize> Serialize for RequiredNullable<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_none(),
            Self::Value(value) => serializer.serialize_some(value),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for RequiredNullable<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(|value| match value {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

pub(super) fn deserialize_required_nullable<'de, D, T>(
    deserializer: D,
) -> Result<RequiredNullable<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    RequiredNullable::deserialize(deserializer)
}

#[derive(Debug, Default)]
pub(super) enum OptionalNullable<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<T> OptionalNullable<T> {
    pub(super) fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    #[cfg(test)]
    pub(super) fn invalid_empty(&self) -> bool
    where
        T: AsRef<str>,
    {
        matches!(self, Self::Value(value) if value.as_ref().is_empty())
    }
}

impl<T: Serialize> Serialize for OptionalNullable<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Missing | Self::Null => serializer.serialize_none(),
            Self::Value(value) => serializer.serialize_some(value),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for OptionalNullable<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(|value| match value {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

static NEXT_GENERATOR: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub(in crate::realtime) struct IdGenerator {
    namespace: u64,
    events: AtomicU64,
    sessions: AtomicU64,
    items: AtomicU64,
}

impl Default for IdGenerator {
    fn default() -> Self {
        Self {
            namespace: NEXT_GENERATOR.fetch_add(1, Ordering::Relaxed),
            events: AtomicU64::new(1),
            sessions: AtomicU64::new(1),
            items: AtomicU64::new(1),
        }
    }
}

impl IdGenerator {
    pub(in crate::realtime) fn event(&self) -> String {
        self.next("evt", &self.events)
    }

    pub(in crate::realtime) fn session(&self) -> String {
        self.next("sess", &self.sessions)
    }

    pub(in crate::realtime) fn item(&self) -> String {
        self.next("item", &self.items)
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(in crate::realtime) fn event_count(&self) -> u64 {
        self.events.load(Ordering::Relaxed).saturating_sub(1)
    }

    fn next(&self, kind: &str, counter: &AtomicU64) -> String {
        let sequence = counter.fetch_add(1, Ordering::Relaxed);
        format!("{kind}_{:016x}_{sequence:016x}", self.namespace)
    }
}
