//! The speech synthesis request body, its closed format sets, and its boundary validation.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The largest `input` a speech request may contain, in characters (OpenAI's
/// cap, and the cap the route enforces).
const MAX_SPEECH_INPUT_CHARS: usize = 4096;

/// The slowest accepted speech `speed` (OpenAI's lower bound).
const MIN_SPEECH_SPEED: f32 = 0.25;

/// The fastest accepted speech `speed` (OpenAI's upper bound).
const MAX_SPEECH_SPEED: f32 = 4.0;

/// The voice to synthesize with: a plain name or the OpenAI object form
/// (`{"id": "..."}`). Membership in a model's catalog is checked at the
/// route, never here, because voice sets are per-checkpoint.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum SpeechVoice {
    /// A plain voice name.
    Name(String),
    /// The OpenAI object form, with the voice under `id`.
    Id {
        /// The voice identifier.
        id: String,
    },
}

/// The audio encoding a speech response is requested in (the OpenAI set).
///
/// The set is closed on purpose: provider-only spellings (Together's `raw`
/// and `mulaw`) stay unrepresentable until the enum is deliberately widened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SpeechResponseFormat {
    /// MPEG audio. The default: OpenAI defaults to mp3 while Together
    /// defaults to wav, so the pin sits in the type and an omitted field
    /// resolves to mp3 at deserialization.
    #[default]
    Mp3,
    /// Opus in an Ogg container.
    Opus,
    /// AAC in an ADTS container.
    Aac,
    /// FLAC.
    Flac,
    /// Uncompressed WAV.
    Wav,
    /// Raw 24 kHz 16-bit signed little-endian PCM.
    Pcm,
}

/// How a streaming speech response is framed (the OpenAI set).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SpeechStreamFormat {
    /// Chunked binary audio (the behavior when the field is absent).
    Audio,
    /// Server-sent events containing base64-encoded audio.
    Sse,
}

/// An incoming speech synthesis request.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SpeechRequest {
    /// The model name, resolved against the routing table.
    pub model: String,
    /// The text to synthesize, at most 4096 characters.
    pub input: String,
    /// The voice to synthesize with.
    pub voice: SpeechVoice,
    /// The requested audio encoding. An omitted field resolves to `mp3` at
    /// deserialization, so the pin is structural and every forwarded body
    /// includes it.
    #[serde(default)]
    pub response_format: SpeechResponseFormat,
    /// The playback speed (0.25 to 4.0); absent means the backend's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// Style-control instructions (the gpt-4o-mini-tts dialect's field);
    /// absent means the backend's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The streaming framing selector; absent means chunked binary audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_format: Option<SpeechStreamFormat>,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl SpeechRequest {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 7] = [
        "model",
        "input",
        "voice",
        "response_format",
        "speed",
        "instructions",
        "stream_format",
    ];

    /// Validates the request shape at the trust boundary, without coercion.
    ///
    /// Rejects an empty model, an empty or over-cap `input`, an out-of-range
    /// `speed`, and any reserved key smuggled into the flattened `rest` map
    /// (WIRE-001/003). Everything else passes through verbatim.
    ///
    /// # Errors
    /// Returns a static reason string when the model is empty, the input is
    /// empty or over the character cap, the speed is out of range, or `rest`
    /// collides with a named field.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.model.trim().is_empty() {
            return Err("model must not be empty");
        }
        if self.input.is_empty() {
            return Err("input must not be empty");
        }
        if self.input.chars().count() > MAX_SPEECH_INPUT_CHARS {
            return Err("input must not exceed 4096 characters");
        }
        if let Some(speed) = self.speed
            && !(MIN_SPEECH_SPEED..=MAX_SPEECH_SPEED).contains(&speed)
        {
            return Err("speed must be between 0.25 and 4.0");
        }
        if Self::RESERVED
            .iter()
            .any(|key| self.rest.contains_key(*key))
        {
            return Err(
                "rest must not contain a reserved key (model, input, voice, response_format, speed, instructions, stream_format)",
            );
        }
        Ok(())
    }
}
