//! OpenAI function-calling dialect.
//!
//! This is the standard dialect for backends that support `tool_calls` in the
//! assistant message and `role=tool` result messages.

use serde_json::Value;

use crate::Result;
use crate::client::{Message, ToolCall};
use crate::normalize::{CompletionNormalizer, NormalizedTurn, OpenAiChatNormalizer};

use super::{
    DetectScore, DialectEvidence, DialectRequest, FramedToolResult, ToolDialect, ToolDialectId,
    correlate_tool_results,
};

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
pub(crate) struct OpenAiDialect;

impl ToolDialect for OpenAiDialect {
    fn id(&self) -> ToolDialectId {
        ToolDialectId::OpenAi
    }

    fn detect(&self, evidence: &DialectEvidence) -> Option<DetectScore> {
        match evidence.supports_tool_calls {
            // Positive support authoritatively selects the native dialect.
            Some(true) => return Some(DetectScore(80)),
            // F5: an authoritative negative is never overridden by template
            // heuristics; this endpoint has no native tool-call protocol.
            Some(false) if evidence.supports_tool_calls_authoritative => return None,
            // Unknown, or an unreliable negative (e.g. llama.cpp `/props`, which
            // can deny tool support a GGUF template actually provides): fall
            // through to structured template evidence.
            _ => {}
        }

        // F6: require a *conjunction* of a request-side and a response/result
        // marker for each template family, not a single broad substring that a
        // mere mention of "tool_call" would satisfy.
        let template = evidence.chat_template.as_deref().unwrap_or("");
        // Qwen/ChatML: the `<|im_start|>` turn framing plus the `<tool_call>`
        // request tag together indicate a genuine tool-calling template.
        let chatml_tools = template.contains("<|im_start|>") && template.contains("<tool_call>");
        // Mistral Tekken / Small Instruct: the `[AVAILABLE_TOOLS]` declaration
        // plus a call or result marker.
        let mistral_tools = template.contains("[AVAILABLE_TOOLS]")
            && (template.contains("[TOOL_CALLS]") || template.contains("[TOOL_RESULTS]"));
        if chatml_tools || mistral_tools {
            Some(DetectScore(70))
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
    /// `calls` and `results` correlate one-to-one. The assistant turn is a
    /// **canonical, deliberately lossy reconstruction** of each call, not a
    /// verbatim echo: exactly `{ "id", "type": "function", "function": { "name",
    /// "arguments" } }`, where `arguments` is the compact JSON string of the
    /// parsed object. Unknown/extension fields, key ordering, and original
    /// whitespace from the model's wire call are intentionally dropped because
    /// [`ToolCall`] retains only the validated `id`, `name`, and `arguments`.
    /// This canonical subset is what backends require to continue a tool loop;
    /// it is stable and tested (see `echo_produces_canonical_subset`).
    ///
    /// # Errors
    /// Returns an error, leaving the conversation unmodified, when `calls` and
    /// `results` fail [`correlate_tool_results`].
    fn echo_tool_results(
        &self,
        conversation: &mut Vec<Message>,
        calls: &[ToolCall],
        results: &[FramedToolResult],
    ) -> Result<()> {
        correlate_tool_results(calls, results)?;
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

        for result in results {
            conversation.push(Message::tool(
                result.id().to_string(),
                result.content().to_string(),
            ));
        }
        Ok(())
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
        let expected = body.clone();
        let mut req = DialectRequest::new(&mut body);
        dialect.prepare_request(&mut req).unwrap();
        assert_eq!(body, expected);
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
    fn parse_turn_rejects_malformed_tool_calls() {
        let dialect = OpenAiDialect;
        // Wrong `type`.
        let wrong_type = serde_json::json!({
            "choices": [{ "message": { "content": null, "tool_calls": [
                { "id": "a", "type": "tool", "function": { "name": "x", "arguments": "{}" } }
            ] } }]
        });
        assert!(dialect.parse_turn(&wrong_type).is_err());

        // Blank id.
        let blank_id = serde_json::json!({
            "choices": [{ "message": { "content": null, "tool_calls": [
                { "id": "", "type": "function", "function": { "name": "x", "arguments": "{}" } }
            ] } }]
        });
        assert!(dialect.parse_turn(&blank_id).is_err());

        // Duplicate ids within one turn.
        let dup = serde_json::json!({
            "choices": [{ "message": { "content": null, "tool_calls": [
                { "id": "d", "type": "function", "function": { "name": "x", "arguments": "{}" } },
                { "id": "d", "type": "function", "function": { "name": "y", "arguments": "{}" } }
            ] } }]
        });
        assert!(dialect.parse_turn(&dup).is_err());

        // Missing arguments.
        let missing_args = serde_json::json!({
            "choices": [{ "message": { "content": null, "tool_calls": [
                { "id": "m", "type": "function", "function": { "name": "x" } }
            ] } }]
        });
        assert!(dialect.parse_turn(&missing_args).is_err());
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
            FramedToolResult::new("call_1".into(), "result 1".into()),
            FramedToolResult::new("call_2".into(), "result 2".into()),
        ];
        let mut conversation = Vec::new();
        dialect
            .echo_tool_results(&mut conversation, &calls, &results)
            .expect("correlated results echo cleanly");

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

    #[test]
    fn echo_produces_canonical_subset() {
        // F2/F10: the echoed assistant turn is exactly the canonical subset -
        // id, type=function, function.name, and function.arguments as a compact
        // JSON string - with no extra fields and a stable shape.
        let dialect = OpenAiDialect;
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "search".into(),
            arguments: serde_json::json!({ "b": 2, "a": 1 }),
        }];
        let results = vec![FramedToolResult::new("call_1".into(), "ok".into())];
        let mut conversation = Vec::new();
        dialect
            .echo_tool_results(&mut conversation, &calls, &results)
            .expect("echoes");

        let raw = conversation[0].tool_calls.as_ref().expect("tool_calls");
        assert_eq!(raw.len(), 1);
        // Exactly the canonical fields, nothing else.
        let obj = raw[0].as_object().expect("object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["function", "id", "type"]);
        assert_eq!(obj["type"], "function");
        assert_eq!(obj["id"], "call_1");
        let function = obj["function"].as_object().expect("function object");
        let mut fkeys: Vec<&str> = function.keys().map(String::as_str).collect();
        fkeys.sort_unstable();
        assert_eq!(fkeys, vec!["arguments", "name"]);
        assert_eq!(function["name"], "search");
        // arguments is a JSON *string*.
        let args = function["arguments"].as_str().expect("arguments string");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(args).unwrap(),
            serde_json::json!({ "a": 1, "b": 2 })
        );
    }

    #[test]
    fn echo_rejects_count_and_order_mismatch() {
        let dialect = OpenAiDialect;
        let calls = vec![
            ToolCall {
                id: "call_1".into(),
                name: "a".into(),
                arguments: serde_json::json!({}),
            },
            ToolCall {
                id: "call_2".into(),
                name: "b".into(),
                arguments: serde_json::json!({}),
            },
        ];

        // Count mismatch.
        let mut conversation = Vec::new();
        assert!(
            dialect
                .echo_tool_results(
                    &mut conversation,
                    &calls,
                    &[FramedToolResult::new("call_1".into(), "r".into())]
                )
                .is_err()
        );
        assert!(conversation.is_empty());

        // Order/id mismatch.
        let swapped = vec![
            FramedToolResult::new("call_2".into(), "r2".into()),
            FramedToolResult::new("call_1".into(), "r1".into()),
        ];
        let mut conversation = Vec::new();
        assert!(
            dialect
                .echo_tool_results(&mut conversation, &calls, &swapped)
                .is_err()
        );
        assert!(conversation.is_empty());
    }
}
