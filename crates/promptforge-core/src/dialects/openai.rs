//! OpenAI function-calling dialect.
//!
//! This is the standard dialect for backends that support `tool_calls` in the
//! assistant message and `role=tool` result messages.

use serde_json::Value;

use crate::client::{Message, ToolCall};
use crate::normalize::NormalizedTurn;
use crate::{Error, Result};

use super::{DetectScore, DialectEvidence, DialectRequest, ToolDialect, ToolDialectId};

/// The standard OpenAI function-calling dialect.
///
/// Detects when the model advertises native tool-call support. Full
/// `prepare_request`, `parse_turn`, and `echo_tool_results` implementations
/// arrive in step 2.
#[derive(Debug, Clone, Copy)]
pub struct OpenAiDialect;

impl ToolDialect for OpenAiDialect {
    fn id(&self) -> ToolDialectId {
        ToolDialectId::OpenAi
    }

    fn detect(&self, evidence: &DialectEvidence) -> Option<DetectScore> {
        if evidence.supports_tool_calls == Some(true) {
            Some(DetectScore(80))
        } else {
            None
        }
    }

    fn prepare_request(&self, _request: &mut DialectRequest<'_>) -> Result<()> {
        Err(Error::DialectNotImplemented {
            dialect: ToolDialectId::OpenAi,
            operation: "prepare_request",
        })
    }

    fn parse_turn(&self, _body: &Value) -> Result<NormalizedTurn> {
        Err(Error::DialectNotImplemented {
            dialect: ToolDialectId::OpenAi,
            operation: "parse_turn",
        })
    }

    fn echo_tool_results(
        &self,
        _conversation: &mut Vec<Message>,
        _calls: &[ToolCall],
        _results: &[(String, String)],
    ) {
        // Step 2: full echo implementation.
    }
}
