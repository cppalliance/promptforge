//! Dialect dispatch: request mutation, the dispatch trait, framed tool
//! results, and validated call/result correlation.

use serde_json::Value;

use super::{DialectEvidence, ToolDialectId};
use crate::client::{Message, ToolCall};
use crate::normalize::NormalizedTurn;
use crate::{Error, Result};

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
/// `results[i].id() == calls[i].id` (order and id correlation). A violation is an
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
