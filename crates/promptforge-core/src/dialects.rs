//! Tool-calling dialect detection, registry, and dispatch.
//!
//! A tool dialect encapsulates the wire differences between backends that
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
/// `#[non_exhaustive]` so new kinds do not break a caller's `match`. Obtain one
/// from [`DialectError::kind`] and match on it instead of parsing the message.
///
/// ```
/// use promptforge_core::dialects::{DialectEvidence, DialectErrorKind, ToolDialectRegistry};
///
/// // Empty evidence matches no dialect, so resolution fails with `NoMatch`.
/// let registry = ToolDialectRegistry::builtin();
/// let error = registry.resolve(&DialectEvidence::default()).unwrap_err();
/// assert_eq!(error.kind(), DialectErrorKind::NoMatch);
/// ```
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
/// and not constructible outside the crate; obtain one only from a failed
/// [`ToolDialectRegistry::resolve`] and inspect it through
/// [`kind`](DialectError::kind) and [`Display`](std::fmt::Display).
///
/// ```
/// use promptforge_core::dialects::{DialectEvidence, DialectErrorKind, ToolDialectRegistry};
///
/// let registry = ToolDialectRegistry::builtin();
/// let error = registry.resolve(&DialectEvidence::default()).unwrap_err();
/// assert_eq!(error.kind(), DialectErrorKind::NoMatch);
/// assert!(!error.to_string().is_empty());
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct DialectError {
    inner: Error,
}

impl DialectError {
    /// Returns the stable classification of this failure.
    ///
    /// ```
    /// use promptforge_core::dialects::{DialectEvidence, DialectErrorKind, ToolDialectRegistry};
    ///
    /// let registry = ToolDialectRegistry::builtin();
    /// let evidence = DialectEvidence::new(Some(true), None, None, None);
    /// // A single strong match resolves cleanly; a miss classifies as `NoMatch`.
    /// assert!(registry.resolve(&evidence).is_ok());
    /// let miss = registry.resolve(&DialectEvidence::default()).unwrap_err();
    /// assert_eq!(miss.kind(), DialectErrorKind::NoMatch);
    /// ```
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
///
/// ```
/// use promptforge_core::dialects::ToolDialectId;
///
/// // Serializes to the catalog wire string and round-trips.
/// let id = ToolDialectId::Gemma3ToolCode;
/// let json = serde_json::to_string(&id).unwrap();
/// assert_eq!(json, "\"gemma3_tool_code\"");
/// let back: ToolDialectId = serde_json::from_str(&json).unwrap();
/// assert_eq!(back, id);
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
/// assert_eq!(serde_json::to_string(&ToolsMode::Emulated).unwrap(), "\"emulated\"");
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
    /// assert_eq!(registry.resolve(&evidence).unwrap(), ToolDialectId::OpenAi);
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
/// The body is private: a dialect reshapes it only through the narrow,
/// validated operations below, so a malformed (non-object) request cannot be
/// silently mutated into a success and every reshape either applies cleanly or
/// returns a preparation error.
#[derive(Debug)]
pub(crate) struct DialectRequest<'a> {
    body: &'a mut Value,
}

impl<'a> DialectRequest<'a> {
    /// Wrap a request body under construction.
    pub(crate) fn new(body: &'a mut Value) -> DialectRequest<'a> {
        DialectRequest { body }
    }

    /// Borrow the body as a JSON object, or fail if the request is not one.
    fn object_mut(&mut self) -> Result<&mut serde_json::Map<String, Value>> {
        self.body
            .as_object_mut()
            .ok_or_else(|| Error::MalformedResponse("request body was not a JSON object".into()))
    }

    /// Validate that the body is a JSON object and, if present, `messages` is an
    /// array. Call before mutating so an invalid shape is rejected, not mutated.
    ///
    /// # Errors
    /// Returns [`Error::MalformedResponse`] when the body is not an object or
    /// `messages` is present but not an array.
    pub(crate) fn validate_shape(&mut self) -> Result<()> {
        let obj = self.object_mut()?;
        if let Some(messages) = obj.get("messages")
            && !messages.is_array()
        {
            return Err(Error::MalformedResponse(
                "request `messages` was present but not an array".into(),
            ));
        }
        Ok(())
    }

    /// Read a top-level field without removing it.
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.body.get(key)
    }

    /// Remove a top-level field, returning its prior value if present.
    ///
    /// # Errors
    /// Returns [`Error::MalformedResponse`] when the body is not a JSON object.
    pub(crate) fn remove(&mut self, key: &str) -> Result<Option<Value>> {
        Ok(self.object_mut()?.remove(key))
    }

    /// Insert `message` at the front of the `messages` array (creating it when
    /// absent).
    ///
    /// # Errors
    /// Returns [`Error::MalformedResponse`] when the body is not a JSON object
    /// or `messages` is present but not an array.
    pub(crate) fn prepend_message(&mut self, message: Value) -> Result<()> {
        let obj = self.object_mut()?;
        let messages = obj
            .entry("messages")
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(list) = messages.as_array_mut() else {
            return Err(Error::MalformedResponse(
                "request `messages` was present but not an array".into(),
            ));
        };
        list.insert(0, message);
        Ok(())
    }
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
    ///
    /// # Errors
    /// Returns an error when `calls` and `results` do not form a validated
    /// one-to-one correlation (see [`correlate_tool_results`]); the conversation
    /// is left unmodified in that case.
    fn echo_tool_results(
        &self,
        conversation: &mut Vec<Message>,
        calls: &[ToolCall],
        results: &[FramedToolResult],
    ) -> Result<()>;
}

/// A tool result whose content has already crossed the trust boundary.
///
/// The executor frames each result before echo: an untrusted result is
/// nonce-wrapped, a trusted one passes through verbatim. This newtype carries
/// that already-framed content so the echo boundary treats it as opaque data
/// and never re-frames, re-inspects trust, or accepts a bare unframed string.
#[derive(Debug, Clone)]
pub(crate) struct FramedToolResult {
    id: String,
    content: String,
}

impl FramedToolResult {
    /// Wrap an already-framed `(id, content)` pair for echo.
    pub(crate) fn new(id: String, content: String) -> FramedToolResult {
        FramedToolResult { id, content }
    }

    /// The tool-call id this result answers.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// The already-framed result content.
    pub(crate) fn content(&self) -> &str {
        &self.content
    }
}

/// Validates that `results` correlate one-to-one with `calls` before any dialect
/// echoes them into the conversation.
///
/// Enforces, in every build (not just debug), that the two slices have equal
/// length (count), that every call id is unique (uniqueness), and that
/// `results[i].0 == calls[i].id` (order and id correlation). A violation is an
/// internal invariant failure - the executor builds `results` in call order -
/// surfaced as a concrete error rather than a truncating `zip` that could echo
/// a result under the wrong call id.
///
/// # Errors
/// Returns [`Error::Internal`] when the count, ordering, id correlation, or call
/// id uniqueness invariant does not hold.
pub(crate) fn correlate_tool_results(
    calls: &[ToolCall],
    results: &[FramedToolResult],
) -> Result<()> {
    if calls.len() != results.len() {
        return Err(Error::Internal(
            "tool-result echo: result count does not match call count",
        ));
    }
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (call, result) in calls.iter().zip(results.iter()) {
        if !seen.insert(call.id.as_str()) {
            return Err(Error::Internal(
                "tool-result echo: duplicate tool call id within one turn",
            ));
        }
        if call.id != result.id() {
            return Err(Error::Internal(
                "tool-result echo: result id does not correlate with its call id in order",
            ));
        }
    }
    Ok(())
}

/// Registry of builtin tool dialects with evidence-based resolution.
///
/// `#[non_exhaustive]` so future fields (or a change away from the builtin-only
/// constructor) are not a breaking change; it is only constructible through
/// [`ToolDialectRegistry::builtin`].
#[non_exhaustive]
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
    ///
    /// ```
    /// use promptforge_core::dialects::{DialectEvidence, ToolDialectId, ToolDialectRegistry};
    ///
    /// let registry = ToolDialectRegistry::builtin();
    /// let evidence = DialectEvidence::new(Some(true), None, None, None);
    /// assert_eq!(registry.resolve(&evidence).unwrap(), ToolDialectId::OpenAi);
    /// ```
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
    /// Scans the registry once, keeping the highest detection score and every
    /// id tied at it. A unique top score resolves; a shared top score is a
    /// [`DialectErrorKind::Tie`]; no score at all is a
    /// [`DialectErrorKind::NoMatch`].
    ///
    /// ```
    /// use promptforge_core::dialects::{
    ///     DialectEvidence, DialectErrorKind, ToolDialectId, ToolDialectRegistry,
    /// };
    ///
    /// let registry = ToolDialectRegistry::builtin();
    ///
    /// // Authoritative native support -> OpenAI.
    /// let native = DialectEvidence::new(Some(true), None, None, None);
    /// assert_eq!(registry.resolve(&native).unwrap(), ToolDialectId::OpenAi);
    ///
    /// // A Gemma template without native tools -> Gemma tool_code.
    /// let gemma = DialectEvidence::new(
    ///     Some(false),
    ///     Some("<start_of_turn>user\n".into()),
    ///     Some("gemma-3-27b-it".into()),
    ///     None,
    /// );
    /// assert_eq!(registry.resolve(&gemma).unwrap(), ToolDialectId::Gemma3ToolCode);
    ///
    /// // No evidence -> NoMatch.
    /// let miss = registry.resolve(&DialectEvidence::default()).unwrap_err();
    /// assert_eq!(miss.kind(), DialectErrorKind::NoMatch);
    /// ```
    ///
    /// # Errors
    /// Returns a [`DialectError`] classified `NoMatch` when no dialect scores on
    /// the evidence, and `Tie` when two or more dialects share the top score.
    pub fn resolve(
        &self,
        evidence: &DialectEvidence,
    ) -> std::result::Result<ToolDialectId, DialectError> {
        // Single scan tracking the best score and every id tied at it, in
        // registry order - no intermediate collection or sort.
        let mut best: Option<DetectScore> = None;
        let mut leader: Option<ToolDialectId> = None;
        let mut tied: Vec<ToolDialectId> = Vec::new();
        for dialect in &self.dialects {
            let Some(score) = dialect.detect(evidence) else {
                continue;
            };
            let id = dialect.id();
            match best {
                Some(current) if score < current => {}
                Some(current) if score == current => tied.push(id),
                _ => {
                    best = Some(score);
                    leader = Some(id);
                    tied.clear();
                    tied.push(id);
                }
            }
        }

        let Some(leader) = leader else {
            return Err(DialectError::from(Error::DialectNone));
        };
        if tied.len() > 1 {
            return Err(DialectError::from(Error::DialectTie { candidates: tied }));
        }
        Ok(leader)
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
