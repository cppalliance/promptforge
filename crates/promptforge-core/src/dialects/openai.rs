//! OpenAI function-calling dialect.
//!
//! This is the standard dialect for backends that support `tool_calls` in the
//! assistant message and `role=tool` result messages.

use serde_json::Value;

use crate::Result;
use crate::client::{Message, ToolCall};
use crate::normalize::{CompletionNormalizer, NormalizedTurn, OpenAiChatNormalizer};

use super::{DetectScore, DialectEvidence, DialectRequest, ToolDialect, ToolDialectId};

/// The standard OpenAI function-calling dialect.
///
/// Detects when the model advertises native tool-call support.
///
/// - `prepare_request` is an identity passthrough: the OpenAI wire format
///   already carries `tools` / `tool_choice` when the caller sets them, so
///   the dialect has nothing to reshape.
/// - `parse_turn` delegates to [`OpenAiChatNormalizer`], which handles
///   native `tool_calls` or plain text content.
/// - `echo_tool_results` pushes the assistant's `tool_calls` turn followed
///   by one `role=tool` message per result, matching the OpenAI wire shape.
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

    /// Identity passthrough - the OpenAI wire format needs no reshaping.
    fn prepare_request(&self, _request: &mut DialectRequest<'_>) -> Result<()> {
        Ok(())
    }

    fn parse_turn(&self, body: &Value) -> Result<NormalizedTurn> {
        OpenAiChatNormalizer.normalize(body)
    }

    /// Push the assistant's tool-call turn and one `role=tool` message per
    /// result into the conversation.
    ///
    /// `calls` and `results` are parallel: `results[i]` is `(id, content)`
    /// answering `calls[i]`. The assistant turn echoes the raw wire shape so
    /// the backend sees exactly the `tool_calls` array it emitted.
    fn echo_tool_results(
        &self,
        conversation: &mut Vec<Message>,
        calls: &[ToolCall],
        results: &[(String, String)],
    ) {
        let raw_calls: Vec<Value> = calls
            .iter()
            .map(|call| {
                serde_json::json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments.to_string(),
                    },
                })
            })
            .collect();
        conversation.push(Message::assistant_tool_calls(raw_calls));

        for (id, content) in results {
            conversation.push(Message::tool(id.clone(), content.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::CompletionResult;

    #[test]
    fn prepare_request_is_identity() {
        let dialect = OpenAiDialect;
        let mut body = serde_json::json!({"model": "gpt-4", "messages": []});
        let mut req = DialectRequest { body: &mut body };
        dialect.prepare_request(&mut req).unwrap();
        assert_eq!(body["model"], "gpt-4");
    }

    #[test]
    fn parse_turn_wire_tool_calls() {
        let dialect = OpenAiDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\":\"rust\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_1");
                assert_eq!(calls[0].name, "web_search");
                assert_eq!(calls[0].arguments, serde_json::json!({"query": "rust"}));
            }
            CompletionResult::Text(t) => panic!("expected tool calls, got text: {t}"),
        }
    }

    #[test]
    fn parse_turn_text_reply() {
        let dialect = OpenAiDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "hello" },
                "finish_reason": "stop"
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::Text(t) => assert_eq!(t, "hello"),
            CompletionResult::ToolCalls(_) => panic!("expected text"),
        }
    }

    #[test]
    fn echo_produces_role_tool_messages() {
        let dialect = OpenAiDialect;
        let calls = vec![
            ToolCall {
                id: "call_1".into(),
                name: "search".into(),
                arguments: serde_json::json!({"q": "rust"}),
            },
            ToolCall {
                id: "call_2".into(),
                name: "fetch".into(),
                arguments: serde_json::json!({"url": "https://example.com"}),
            },
        ];
        let results = vec![
            ("call_1".into(), "result 1".into()),
            ("call_2".into(), "result 2".into()),
        ];
        let mut conversation = Vec::new();
        dialect.echo_tool_results(&mut conversation, &calls, &results);

        assert_eq!(conversation.len(), 3);
        assert_eq!(conversation[0].role, "assistant");
        assert!(conversation[0].tool_calls.is_some());
        let tc = conversation[0].tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 2);
        assert_eq!(tc[0]["function"]["name"], "search");

        assert_eq!(conversation[1].role, "tool");
        assert_eq!(conversation[1].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(conversation[1].content, "result 1");

        assert_eq!(conversation[2].role, "tool");
        assert_eq!(conversation[2].tool_call_id.as_deref(), Some("call_2"));
        assert_eq!(conversation[2].content, "result 2");
    }
}
