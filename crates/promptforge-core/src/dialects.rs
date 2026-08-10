//! Tool-calling dialect detection, registry, and dispatch.
//!
//! A [`ToolDialect`] encapsulates the wire differences between backends that
//! speak the same `/chat/completions` shape but vary in how tool calls are
//! declared, returned, and echoed. The [`ToolDialectRegistry`] holds the
//! builtin set and resolves evidence into a single dialect, hard-failing on
//! ties or no-match so the runtime never silently guesses.

#[path = "dialects/gemma3_tool_code.rs"]
mod gemma3_tool_code;
#[path = "dialects/openai.rs"]
mod openai;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Message, ToolCall};
use crate::normalize::NormalizedTurn;
use crate::{Error, Result};

pub(crate) use gemma3_tool_code::Gemma3ToolCodeDialect;
pub(crate) use openai::OpenAiDialect;

/// A stable, matchable classification of a [`DialectError`].
///
/// `#[non_exhaustive]` so new kinds do not break a caller's `match`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DialectErrorKind {
    /// No registered dialect scored on the provided evidence.
    NoMatch,
    /// Two or more dialects tied for the highest detection score.
    Tie,
    /// A named dialect was not present in the registry.
    Unknown,
}

/// The error returned by [`ToolDialectRegistry::resolve`].
///
/// Carries a stable [`kind`](DialectError::kind) classifier. `#[non_exhaustive]`
/// and not constructible outside the crate.
#[derive(Debug)]
#[non_exhaustive]
pub struct DialectError {
    inner: Error,
}

impl DialectError {
    /// Returns the stable classification of this failure.
    #[must_use]
    pub fn kind(&self) -> DialectErrorKind {
        match &self.inner {
            Error::DialectTie { .. } => DialectErrorKind::Tie,
            Error::UnknownDialect(_) => DialectErrorKind::Unknown,
            _ => DialectErrorKind::NoMatch,
        }
    }
}

impl std::fmt::Display for DialectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for DialectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.inner)
    }
}

impl From<Error> for DialectError {
    fn from(inner: Error) -> Self {
        DialectError { inner }
    }
}

impl From<DialectError> for Error {
    fn from(error: DialectError) -> Self {
        error.inner
    }
}

/// Identifies a registered tool dialect.
///
/// Variants are `#[non_exhaustive]` so new backends can be added without a
/// breaking change. Serializes to the same lowercase strings used in catalog
/// JSON (`"openai"`, `"gemma3_tool_code"`).
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
    /// The raw Jinja chat template string, when available from model metadata.
    pub chat_template: Option<String>,
    /// The model identifier from the catalog or endpoint metadata.
    pub model_id: Option<String>,
    /// The model's source or provenance label (e.g. GGUF filename).
    pub source: Option<String>,
}

impl DialectEvidence {
    /// Builds evidence from the four optional axes.
    #[must_use]
    pub fn new(
        supports_tool_calls: Option<bool>,
        chat_template: Option<String>,
        model_id: Option<String>,
        source: Option<String>,
    ) -> Self {
        Self {
            supports_tool_calls,
            chat_template,
            model_id,
            source,
        }
    }
}

/// A dialect's confidence that it matches some [`DialectEvidence`].
///
/// Higher values win. The scale is arbitrary but values should stay in `0..=100`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DetectScore(pub(crate) u8);

/// Mutable view of a request under construction, passed to
/// [`ToolDialect::prepare_request`] so a dialect can reshape the payload.
///
/// Intentionally opaque for now; step 2 will add fields.
#[derive(Debug)]
pub(crate) struct DialectRequest<'a> {
    /// The request JSON body being assembled.
    pub(crate) body: &'a mut Value,
}

/// A tool-calling dialect that knows how to prepare requests, parse turns, and
/// echo tool results in its wire format.
///
/// Implementors must be object-safe (`dyn`-compatible) and thread-safe.
pub(crate) trait ToolDialect: Send + Sync {
    /// The dialect's identity.
    fn id(&self) -> ToolDialectId;

    /// Score how well this dialect matches the provided evidence.
    ///
    /// Returns `None` when the evidence is insufficient or contradicts this
    /// dialect.
    fn detect(&self, evidence: &DialectEvidence) -> Option<DetectScore>;

    /// Reshape a request body before it is sent to the backend.
    ///
    /// # Errors
    /// Returns an error if the request cannot be prepared.
    fn prepare_request(&self, request: &mut DialectRequest<'_>) -> Result<()>;

    /// Parse the response body into a [`NormalizedTurn`].
    ///
    /// # Errors
    /// Returns an error if the body cannot be understood.
    fn parse_turn(&self, body: &Value) -> Result<NormalizedTurn>;

    /// Echo tool-call results back into the conversation history.
    fn echo_tool_results(
        &self,
        conversation: &mut Vec<Message>,
        calls: &[ToolCall],
        results: &[(String, String)],
    );
}

/// Registry of builtin tool dialects with evidence-based resolution.
pub struct ToolDialectRegistry {
    dialects: Vec<Box<dyn ToolDialect>>,
}

impl std::fmt::Debug for ToolDialectRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<ToolDialectId> = self.dialects.iter().map(|d| d.id()).collect();
        f.debug_struct("ToolDialectRegistry")
            .field("dialects", &ids)
            .finish()
    }
}

impl ToolDialectRegistry {
    /// Construct the registry populated with all builtin dialects.
    #[must_use]
    pub fn builtin() -> ToolDialectRegistry {
        ToolDialectRegistry {
            dialects: vec![Box::new(OpenAiDialect), Box::new(Gemma3ToolCodeDialect)],
        }
    }

    /// Look up a dialect by its [`ToolDialectId`].
    #[must_use]
    pub(crate) fn get(&self, id: ToolDialectId) -> Option<&dyn ToolDialect> {
        self.dialects
            .iter()
            .find(|d| d.id() == id)
            .map(std::convert::AsRef::as_ref)
    }

    /// Resolve evidence into a single dialect, failing on ties or no match.
    ///
    /// # Errors
    /// Returns a [`DialectError`] classified `NoMatch` when no dialect scores on
    /// the evidence, and `Tie` when two or more dialects share the top score.
    pub fn resolve(
        &self,
        evidence: &DialectEvidence,
    ) -> std::result::Result<ToolDialectId, DialectError> {
        let mut scored: Vec<(ToolDialectId, DetectScore)> = self
            .dialects
            .iter()
            .filter_map(|d| d.detect(evidence).map(|s| (d.id(), s)))
            .collect();

        if scored.is_empty() {
            return Err(DialectError::from(Error::DialectNone));
        }

        scored.sort_by_key(|entry| std::cmp::Reverse(entry.1));

        if scored.len() > 1 && scored[0].1 == scored[1].1 {
            let tied: Vec<ToolDialectId> = scored
                .iter()
                .take_while(|(_, s)| *s == scored[0].1)
                .map(|(id, _)| *id)
                .collect();
            return Err(DialectError::from(Error::DialectTie { candidates: tied }));
        }

        Ok(scored[0].0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_evidence_fails_resolve() {
        let registry = ToolDialectRegistry::builtin();
        let evidence = DialectEvidence::default();
        let result = registry.resolve(&evidence);
        assert!(
            matches!(result, Err(Error::DialectNone)),
            "expected DialectNone, got {result:?}"
        );
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
            source: None,
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
                "[SYSTEM_PROMPT]x[/SYSTEM_PROMPT][AVAILABLE_TOOLS][][/AVAILABLE_TOOLS][INST]"
                    .to_string(),
            ),
            model_id: Some("mistral-small".to_string()),
            source: None,
        };
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id, ToolDialectId::OpenAi);
    }

    #[test]
    fn gemma_scores_when_no_native_tools_and_template() {
        let registry = ToolDialectRegistry::builtin();
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some("<start_of_turn>user\n".to_string()),
            model_id: Some("gemma-3-27b-it".to_string()),
            source: None,
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
        assert_eq!(serde_json::to_string(&native).unwrap(), "\"native\"");

        let emulated = ToolsMode::Emulated;
        assert_eq!(serde_json::to_string(&emulated).unwrap(), "\"emulated\"");
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
