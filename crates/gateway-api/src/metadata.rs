//! Model-metadata vocabulary: what a model can do, independent of how the
//! gateway reaches it.
//!
//! These types are the canonical home of the metadata hoisted from
//! `gateway-config` (`Capabilities`, `ModelKind`, `ThinkingMode`) and
//! `gateway-protocol` (`ModelInfo`); both crates re-export them at their
//! old paths so downstream call sites compile unchanged.

use std::fmt;

use serde::{Deserialize, Serialize};

/// How a model exposes chain-of-thought / thinking tokens to callers.
///
/// Catalogued on each `[[model]]` so hosts can filter bindings before a
/// request is built. `never` and `always` mean the backend ignores a
/// per-call switch; `switchable` means the client may emit
/// `chat_template_kwargs.enable_thinking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ThinkingMode {
    /// The backend never emits thinking tokens; a per-call switch is ignored.
    #[default]
    Never,
    /// The backend always emits thinking tokens; a per-call switch is ignored.
    Always,
    /// The client may turn thinking on or off per request.
    Switchable,
}

/// The workload a model serves: chat completions, embeddings,
/// classification, speech synthesis, transcription, or image or video
/// generation.
///
/// The kind scopes which configuration fields are meaningful: chat-only
/// fields (for example `thinking`, `default_max_tokens`,
/// `chat_template_file`) are rejected for non-chat kinds at validation,
/// while `context` applies to every kind. The catalog carries the kind so
/// clients can filter before building a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ModelKind {
    /// Chat completions (`POST /v1/chat/completions`). The default.
    #[default]
    Chat,
    /// Text embeddings.
    Embedding,
    /// Classification / reranking.
    Classifier,
    /// Speech synthesis (`POST /v1/audio/speech`).
    Speech,
    /// Speech-to-text transcription.
    Transcription,
    /// Image generation.
    Image,
    /// Video generation.
    Video,
}

impl fmt::Display for ModelKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let spelling = match self {
            ModelKind::Chat => "chat",
            ModelKind::Embedding => "embedding",
            ModelKind::Classifier => "classifier",
            ModelKind::Speech => "speech",
            ModelKind::Transcription => "transcription",
            ModelKind::Image => "image",
            ModelKind::Video => "video",
        };
        f.write_str(spelling)
    }
}

/// Capability metadata advertised on the model catalog.
///
/// These fields describe what a model can do rather than how the gateway
/// reaches it. They are flattened into `[[model]]` and `[[local_model]]`,
/// validated at load, and surfaced verbatim on `GET /v1/models` so clients
/// can shape requests before sending them. The effort knobs are chat-only
/// and require a `thinking` mode other than `never`; the `voices` list is
/// speech-only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Capabilities {
    /// Max output tokens the model can emit per completion. Must not exceed
    /// `context` when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u32>,
    /// Sampling temperature applied when the caller omits one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_temperature: Option<f32>,
    /// Whether the model accepts image inputs. Defaults to false; a
    /// `[local_model.multimodal_projector]` companion implies true.
    #[serde(default)]
    pub images: bool,
    /// Whether the model can emit parallel tool calls. Defaults to false.
    #[serde(default)]
    pub parallel_tool_calls: bool,
    /// The reasoning-effort levels the model accepts. Empty means the model
    /// has no effort knob.
    #[serde(default)]
    pub effort_levels: Vec<String>,
    /// The effort level applied when the caller omits one; requires a
    /// non-empty `effort_levels` and must name a listed level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// Whether the model adaptively chooses how much to think per request;
    /// chat kind only. Defaults to false.
    #[serde(default)]
    pub adaptive_thinking: bool,
    /// The voices the model offers for speech synthesis; speech kind only.
    /// Empty means the model exposes no fixed voice list.
    #[serde(default)]
    pub voices: Vec<String>,
}

impl Capabilities {
    /// Returns the max output tokens the model can emit per completion, when
    /// set.
    ///
    /// # Examples
    /// ```
    /// let mut capabilities = gateway_api::Capabilities::default();
    /// capabilities.max_output = Some(4096);
    /// assert_eq!(capabilities.max_output(), Some(4096));
    /// ```
    #[must_use]
    pub fn max_output(&self) -> Option<u32> {
        self.max_output
    }

    /// Returns the sampling temperature applied when the caller omits one,
    /// when set.
    ///
    /// # Examples
    /// ```
    /// let mut capabilities = gateway_api::Capabilities::default();
    /// capabilities.default_temperature = Some(0.7);
    /// assert_eq!(capabilities.default_temperature(), Some(0.7));
    /// ```
    #[must_use]
    pub fn default_temperature(&self) -> Option<f32> {
        self.default_temperature
    }

    /// Returns whether the model accepts image inputs.
    ///
    /// # Examples
    /// ```
    /// let mut capabilities = gateway_api::Capabilities::default();
    /// capabilities.images = true;
    /// assert!(capabilities.images());
    /// ```
    #[must_use]
    pub fn images(&self) -> bool {
        self.images
    }

    /// Returns whether the model can emit parallel tool calls.
    ///
    /// # Examples
    /// ```
    /// let mut capabilities = gateway_api::Capabilities::default();
    /// capabilities.parallel_tool_calls = true;
    /// assert!(capabilities.parallel_tool_calls());
    /// ```
    #[must_use]
    pub fn parallel_tool_calls(&self) -> bool {
        self.parallel_tool_calls
    }

    /// Returns the reasoning-effort levels the model accepts (empty when the
    /// model has no effort knob).
    ///
    /// # Examples
    /// ```
    /// let mut capabilities = gateway_api::Capabilities::default();
    /// capabilities.effort_levels = vec!["low".to_owned(), "high".to_owned()];
    /// assert_eq!(capabilities.effort_levels(), ["low", "high"]);
    /// ```
    #[must_use]
    pub fn effort_levels(&self) -> &[String] {
        &self.effort_levels
    }

    /// Returns the effort level applied when the caller omits one, when set.
    ///
    /// # Examples
    /// ```
    /// let mut capabilities = gateway_api::Capabilities::default();
    /// capabilities.default_effort = Some("low".to_owned());
    /// assert_eq!(capabilities.default_effort(), Some("low"));
    /// ```
    #[must_use]
    pub fn default_effort(&self) -> Option<&str> {
        self.default_effort.as_deref()
    }

    /// Returns whether the model adaptively chooses how much to think per
    /// request.
    ///
    /// # Examples
    /// ```
    /// let mut capabilities = gateway_api::Capabilities::default();
    /// capabilities.adaptive_thinking = true;
    /// assert!(capabilities.adaptive_thinking());
    /// ```
    #[must_use]
    pub fn adaptive_thinking(&self) -> bool {
        self.adaptive_thinking
    }

    /// Returns the voices the model offers for speech synthesis (empty when
    /// the model exposes no fixed voice list).
    ///
    /// # Examples
    /// ```
    /// let mut capabilities = gateway_api::Capabilities::default();
    /// capabilities.voices = vec!["alloy".to_owned(), "nova".to_owned()];
    /// assert_eq!(capabilities.voices(), ["alloy", "nova"]);
    /// ```
    #[must_use]
    pub fn voices(&self) -> &[String] {
        &self.voices
    }
}

/// One catalogued model, with PromptForge extensions beside the OpenAI `id`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelInfo {
    /// The caller-facing model name (`[[model]].name`).
    pub id: String,
    /// Always `"model"`.
    pub object: &'static str,
    /// The workload this model serves (`"chat"`, `"embedding"`,
    /// `"classifier"`, `"speech"`, `"transcription"`, `"image"`,
    /// `"video"`).
    pub kind: ModelKind,
    /// Prose describing the model for catalog consumers and semantic bind.
    pub description: String,
    /// Context window size in tokens.
    pub context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    pub thinking: ThinkingMode,
    /// Capability metadata (`max_output`, `images`, effort levels, and so
    /// on), flattened into the catalog entry.
    #[serde(flatten)]
    pub capabilities: Capabilities,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_kind_variants_use_catalog_spelling() {
        // Catches a serde rename or Display regression on every variant,
        // including the hoisted transcription/image/video extensions.
        for (kind, spelling) in [
            (ModelKind::Chat, "chat"),
            (ModelKind::Embedding, "embedding"),
            (ModelKind::Classifier, "classifier"),
            (ModelKind::Speech, "speech"),
            (ModelKind::Transcription, "transcription"),
            (ModelKind::Image, "image"),
            (ModelKind::Video, "video"),
        ] {
            let json = serde_json::to_value(kind).expect("serialize");
            assert_eq!(json.as_str(), Some(spelling));
            assert_eq!(kind.to_string(), spelling);
        }
    }

    #[test]
    fn thinking_mode_uses_catalog_spelling() {
        // Catches a serde rename regression on the hoisted ThinkingMode.
        for (mode, spelling) in [
            (ThinkingMode::Never, "never"),
            (ThinkingMode::Always, "always"),
            (ThinkingMode::Switchable, "switchable"),
        ] {
            let json = serde_json::to_value(mode).expect("serialize");
            assert_eq!(json.as_str(), Some(spelling));
        }
    }
}
