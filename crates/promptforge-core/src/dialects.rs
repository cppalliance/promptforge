//! Tool-calling dialect detection, registry, and dispatch.
//!
//! A tool dialect encapsulates the wire differences between backends that
//! speak the same `/chat/completions` shape but vary in how tool calls are
//! declared, returned, and echoed. The [`ToolDialectRegistry`] holds the
//! builtin set and resolves evidence into a single dialect, hard-failing on
//! ties or no-match so the runtime never silently guesses.

#[path = "dialects/gemma3_tool_code/mod.rs"]
mod gemma3_tool_code;
#[path = "dialects/openai.rs"]
mod openai;

mod dispatch;
mod error;
mod registry;

use serde::{Deserialize, Serialize};

pub(crate) use dispatch::{
    DetectScore, DialectRequest, FramedToolResult, ToolDialect, correlate_tool_results,
};
pub use error::{DialectError, DialectErrorKind};
pub(crate) use gemma3_tool_code::Gemma3ToolCodeDialect;
pub(crate) use openai::OpenAiDialect;
pub use registry::ToolDialectRegistry;

/// Identifies a registered tool dialect.
///
/// Variants are `#[non_exhaustive]` so new backends can be added without a
/// breaking change. Serializes to the same lowercase strings used in catalog
/// JSON (`"openai"`, `"gemma3_tool_code"`).
///
/// ```
/// use promptforge_core::dialects::ToolDialectId;
///
/// // Serializes to the catalog wire string and round-trips.
/// let id = ToolDialectId::Gemma3ToolCode;
/// let json = serde_json::to_string(&id)?;
/// assert_eq!(json, "\"gemma3_tool_code\"");
/// let back: ToolDialectId = serde_json::from_str(&json)?;
/// assert_eq!(back, id);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ToolDialectId {
    /// Standard OpenAI function-calling protocol.
    #[serde(rename = "openai")]
    OpenAi,
    /// Gemma-3 `tool_code` fence protocol for models without native tool support.
    #[serde(rename = "gemma3_tool_code")]
    Gemma3ToolCode,
}

impl std::fmt::Display for ToolDialectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolDialectId::OpenAi => f.write_str("openai"),
            ToolDialectId::Gemma3ToolCode => f.write_str("gemma3_tool_code"),
        }
    }
}

/// Whether tool calls are handled natively by the backend or emulated by the
/// runtime through content-fence parsing.
///
/// ```
/// use promptforge_core::dialects::{ToolDialectId, ToolsMode};
///
/// // The mode is always derived from the dialect, never stored independently.
/// assert_eq!(ToolDialectId::OpenAi.tools_mode(), ToolsMode::Native);
/// assert_eq!(ToolDialectId::Gemma3ToolCode.tools_mode(), ToolsMode::Emulated);
/// assert_eq!(serde_json::to_string(&ToolsMode::Emulated)?, "\"emulated\"");
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ToolsMode {
    /// The backend speaks a native tool-call protocol (e.g. OpenAI `tool_calls`).
    #[serde(rename = "native")]
    Native,
    /// The runtime emulates tool calls by injecting tool descriptions into the
    /// prompt and parsing structured fences from model output.
    #[serde(rename = "emulated")]
    Emulated,
}

impl std::fmt::Display for ToolsMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolsMode::Native => f.write_str("native"),
            ToolsMode::Emulated => f.write_str("emulated"),
        }
    }
}

impl ToolDialectId {
    /// The tools mode implied by this dialect.
    ///
    /// ```
    /// use promptforge_core::dialects::{ToolDialectId, ToolsMode};
    ///
    /// assert_eq!(ToolDialectId::OpenAi.tools_mode(), ToolsMode::Native);
    /// assert_eq!(ToolDialectId::Gemma3ToolCode.tools_mode(), ToolsMode::Emulated);
    /// ```
    #[must_use]
    pub fn tools_mode(&self) -> ToolsMode {
        match self {
            ToolDialectId::OpenAi => ToolsMode::Native,
            ToolDialectId::Gemma3ToolCode => ToolsMode::Emulated,
        }
    }
}

/// Evidence collected from configuration and model metadata used to select a
/// dialect.
///
/// Fields are `Option` so callers supply only what they know. The struct is
/// `#[non_exhaustive]` so new evidence axes can be added without breakage.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DialectEvidence {
    /// Whether the model endpoint advertises native tool-call support.
    pub supports_tool_calls: Option<bool>,
    /// Whether [`Self::supports_tool_calls`] is authoritative.
    ///
    /// An authoritative `Some(false)` (the endpoint genuinely has no native
    /// tool-call protocol) must never be overridden by template heuristics into
    /// selecting a native dialect. An unreliable `Some(false)` - the default -
    /// may be overridden, because sources like llama.cpp `/props` can deny tool
    /// support that a GGUF chat template actually provides.
    pub supports_tool_calls_authoritative: bool,
    /// The raw Jinja chat template string, when available from model metadata.
    pub chat_template: Option<String>,
    /// The model identifier from the catalog or endpoint metadata.
    pub model_id: Option<String>,
    /// The model's source or provenance label (e.g. GGUF filename).
    pub source: Option<String>,
}

impl DialectEvidence {
    /// Builds evidence from the four optional axes.
    ///
    /// ```
    /// use promptforge_core::dialects::{DialectEvidence, ToolDialectId, ToolDialectRegistry};
    ///
    /// // Authoritative native tool-call support resolves to the OpenAI dialect.
    /// let evidence = DialectEvidence::new(Some(true), None, Some("gpt-4o".into()), None);
    /// let registry = ToolDialectRegistry::builtin();
    /// assert_eq!(registry.resolve(&evidence)?, ToolDialectId::OpenAi);
    /// # Ok::<(), promptforge_core::dialects::DialectError>(())
    /// ```
    #[must_use]
    pub fn new(
        supports_tool_calls: Option<bool>,
        chat_template: Option<String>,
        model_id: Option<String>,
        source: Option<String>,
    ) -> Self {
        Self {
            supports_tool_calls,
            supports_tool_calls_authoritative: false,
            chat_template,
            model_id,
            source,
        }
    }

    /// Marks [`Self::supports_tool_calls`] as authoritative, so an authoritative
    /// negative can never be overridden into a native dialect by template text.
    #[must_use]
    pub fn authoritative_tool_support(mut self, authoritative: bool) -> Self {
        self.supports_tool_calls_authoritative = authoritative;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_evidence_fails_resolve() {
        let registry = ToolDialectRegistry::builtin();
        let evidence = DialectEvidence::default();
        let error = registry
            .resolve(&evidence)
            .expect_err("empty evidence must fail to resolve");
        assert_eq!(error.kind(), DialectErrorKind::NoMatch);
    }

    #[test]
    fn openai_scores_when_supports_tool_calls() {
        let registry = ToolDialectRegistry::builtin();
        let evidence = DialectEvidence {
            supports_tool_calls: Some(true),
            ..Default::default()
        };
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id, ToolDialectId::OpenAi);
    }

    #[test]
    fn openai_scores_chatml_tool_template_without_native_flag() {
        let registry = ToolDialectRegistry::builtin();
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some(
                "<|im_start|>system\n{%- if tools %}<tool_call>{%- endif %}<|im_end|>".to_string(),
            ),
            model_id: Some("qwen3.5-9b".to_string()),
            ..Default::default()
        };
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id, ToolDialectId::OpenAi);
    }

    #[test]
    fn openai_scores_mistral_tools_template_without_native_flag() {
        let registry = ToolDialectRegistry::builtin();
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some(
                "[SYSTEM_PROMPT]x[/SYSTEM_PROMPT][AVAILABLE_TOOLS][][/AVAILABLE_TOOLS][INST][TOOL_CALLS][TOOL_RESULTS]"
                    .to_string(),
            ),
            model_id: Some("mistral-small".to_string()),
            ..Default::default()
        };
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id, ToolDialectId::OpenAi);
    }

    #[test]
    fn authoritative_negative_is_never_overridden_by_template() {
        // F5: an authoritative `Some(false)` must not be overridden into the
        // native dialect by tool-calling template markers.
        let registry = ToolDialectRegistry::builtin();
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            supports_tool_calls_authoritative: true,
            chat_template: Some(
                "<|im_start|>system\n<tool_call>{%- endif %}<|im_end|>".to_string(),
            ),
            model_id: Some("qwen-mystery".to_string()),
            ..Default::default()
        };
        let error = registry
            .resolve(&evidence)
            .expect_err("authoritative negative must not select a native dialect");
        assert_eq!(error.kind(), DialectErrorKind::NoMatch);
    }

    #[test]
    fn single_marker_template_does_not_select_native() {
        // F6: a lone marker (or a mere mention of "tool_call") must not select
        // the native dialect; a conjunction of request + response markers is
        // required.
        let registry = ToolDialectRegistry::builtin();
        for template in [
            "<|im_start|>system\nplease tool_call something<|im_end|>", // mention only
            "[AVAILABLE_TOOLS][]",                                      // request marker only
            "[TOOL_CALLS]",                                             // response marker only
        ] {
            let evidence = DialectEvidence {
                supports_tool_calls: Some(false),
                chat_template: Some(template.to_string()),
                model_id: Some("mystery".to_string()),
                ..Default::default()
            };
            assert_eq!(
                registry.resolve(&evidence).map_err(|e| e.kind()),
                Err(DialectErrorKind::NoMatch),
                "template {template:?} must not select a dialect",
            );
        }
    }

    #[test]
    fn gemma_scores_when_no_native_tools_and_template() {
        let registry = ToolDialectRegistry::builtin();
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some("<start_of_turn>user\n".to_string()),
            model_id: Some("gemma-3-27b-it".to_string()),
            ..Default::default()
        };
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id, ToolDialectId::Gemma3ToolCode);
    }

    #[test]
    fn dialect_id_serde_round_trip() {
        let openai = ToolDialectId::OpenAi;
        let json = serde_json::to_string(&openai).unwrap();
        assert_eq!(json, "\"openai\"");
        let parsed: ToolDialectId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, openai);

        let gemma = ToolDialectId::Gemma3ToolCode;
        let json = serde_json::to_string(&gemma).unwrap();
        assert_eq!(json, "\"gemma3_tool_code\"");
        let parsed: ToolDialectId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, gemma);
    }

    #[test]
    fn tools_mode_serde_round_trip() {
        let native = ToolsMode::Native;
        let json = serde_json::to_string(&native).unwrap();
        assert_eq!(json, "\"native\"");
        let parsed: ToolsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, native);

        let emulated = ToolsMode::Emulated;
        let json = serde_json::to_string(&emulated).unwrap();
        assert_eq!(json, "\"emulated\"");
        let parsed: ToolsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, emulated);
    }

    #[test]
    fn dialect_id_tools_mode_mapping() {
        assert_eq!(ToolDialectId::OpenAi.tools_mode(), ToolsMode::Native);
        assert_eq!(
            ToolDialectId::Gemma3ToolCode.tools_mode(),
            ToolsMode::Emulated
        );
    }
}
