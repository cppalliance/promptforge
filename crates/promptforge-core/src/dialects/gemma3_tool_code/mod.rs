//! Gemma-3 `tool_code` fence dialect.
//!
//! Local Gemma models served via llama.cpp never emit a `message.tool_calls`
//! array. Instead they put a sole ` ```tool_code ` fence (Python-style call
//! syntax) in `message.content`. Results are echoed as a follow-up user turn
//! rather than `role=tool` messages, because Gemma chat templates reject the
//! OpenAI tool-result shape with HTTP 400.
//!
//! This dialect also handles the interim fenced-JSON `tool_calls` blob that
//! some Gemma quantizations emit. The moving parts live in focused child
//! modules: [`codec`] (the symmetric `tool_code` call grammar and its
//! adversarial tests), [`content`] (fence scanning and three-way
//! classification), and [`guide`] (the injected system guide).

mod codec;
mod content;
mod guide;

use serde_json::Value;

use crate::client::{Message, ToolCall};
use crate::normalize::NormalizedTurn;
use crate::{Error, Result};

use self::codec::render_tool_code_fence;
use self::content::{ContentParse, parse_content_tool_dialect};
use self::guide::render_tool_guide;
use super::{
    DetectScore, DialectEvidence, DialectRequest, FramedToolResult, ToolDialect, ToolDialectId,
    correlate_tool_results,
};

/// The Gemma-3 `tool_code` fence dialect.
///
/// - `detect`: never matches an endpoint with explicit native tool-call support
///   (`supports_tool_calls == Some(true)`); otherwise it scores a Gemma-
///   fingerprinted model (template, model-id, or source), adding weight for an
///   explicit `Some(false)`. Unknown capability plus a Gemma fingerprint still
///   scores, since Gemma via llama.cpp never emits native `tool_calls`.
/// - `prepare_request`: copies tool schemas into a system guide, then strips
///   `tools` / `tool_choice` (Gemma rejects the native OpenAI tool array).
/// - `parse_turn`: recognizes sole `tool_code` fences and fenced JSON
///   `tool_calls` blobs; mixed prose stays text.
/// - `echo_tool_results`: pushes the assistant's rendered `tool_code` fence
///   then a user message with `TOOL RESULT` blocks and a continue trailer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Gemma3ToolCodeDialect;

impl ToolDialect for Gemma3ToolCodeDialect {
    fn id(&self) -> ToolDialectId {
        ToolDialectId::Gemma3ToolCode
    }

    fn detect(&self, evidence: &DialectEvidence) -> Option<DetectScore> {
        // Native tool-call endpoints are never this dialect.
        if evidence.supports_tool_calls == Some(true) {
            return None;
        }

        // `<bos>` alone is too common; require the Gemma turn marker.
        let gemma_template = evidence
            .chat_template
            .as_deref()
            .is_some_and(|template| template.contains("<start_of_turn>"));
        let gemma_model = evidence
            .model_id
            .as_deref()
            .is_some_and(|id| id.to_ascii_lowercase().contains("gemma"));
        let gemma_source = evidence
            .source
            .as_deref()
            .is_some_and(|src| src.to_ascii_lowercase().contains("gemma"));

        // Require a Gemma fingerprint so other tools-unsupported models (e.g.
        // some Qwen GGUFs) do not resolve here from caps alone.
        if !gemma_template && !gemma_model && !gemma_source {
            return None;
        }

        let mut score: u8 = 0;
        if evidence.supports_tool_calls == Some(false) {
            score += 40;
        }
        if gemma_template {
            score += 30;
        }
        if gemma_model {
            score += 20;
        }
        if gemma_source {
            score += 10;
        }
        Some(DetectScore(score))
    }

    /// Emulate tools: copy schemas into a leading system guide, then strip the
    /// native OpenAI `tools` / `tool_choice` fields Gemma rejects.
    ///
    /// The complete guide is validated and built *before* any mutation, so an
    /// invalid request shape (non-object body, non-array `messages` or `tools`)
    /// returns a preparation error instead of a silently half-mutated success.
    ///
    /// # Errors
    /// Returns [`Error::MalformedResponse`] when the request body is not a JSON
    /// object, or when `messages` or `tools` is present but not an array.
    fn prepare_request(&self, request: &mut DialectRequest<'_>) -> Result<()> {
        request.validate_shape()?;

        // Build the guide first; only a validated, fully-built guide is applied.
        let guide = match request.get("tools") {
            None | Some(Value::Null) => None,
            Some(Value::Array(tools)) => render_tool_guide(tools),
            Some(_) => {
                return Err(Error::MalformedResponse(
                    "request `tools` was present but not an array".into(),
                ));
            }
        };

        // Validation passed: mutate atomically.
        request.remove("tools")?;
        request.remove("tool_choice")?;
        if let Some(guide) = guide {
            request.prepend_message(serde_json::json!({
                "role": "system",
                "content": guide,
            }))?;
        }
        Ok(())
    }

    fn parse_turn(&self, body: &Value) -> Result<NormalizedTurn> {
        // Choice/message/finish_reason/reasoning extraction is shared with the
        // OpenAI normalizer; only the content-fence recognition below is
        // dialect-specific (PF-NORM-006).
        let crate::normalize::TurnContext {
            message,
            finish_reason,
            reasoning_content,
        } = crate::normalize::turn_context(body)?;

        // Gemma never emits wire tool_calls; go straight to content parsing.
        if let Some(content) = message
            .get("content")
            .and_then(Value::as_str)
            .filter(|content| !content.trim().is_empty())
        {
            // Three-way classification: recognized calls become a tool turn,
            // ordinary prose stays text, and a recognized-but-malformed tool
            // fence propagates as a concrete error instead of masquerading as
            // final text.
            match parse_content_tool_dialect(content) {
                ContentParse::Calls(calls) => {
                    return Ok(NormalizedTurn {
                        outcome: crate::client::CompletionResult::ToolCalls(calls),
                        finish_reason,
                        reasoning_content,
                    });
                }
                ContentParse::NotProtocol => {
                    return Ok(NormalizedTurn {
                        outcome: crate::client::CompletionResult::Text(content.to_string()),
                        finish_reason,
                        reasoning_content,
                    });
                }
                ContentParse::Malformed(error) => return Err(error),
            }
        }

        Err(crate::normalize::empty_reply_error(
            reasoning_content.is_some(),
            finish_reason,
        ))
    }

    /// Echo tool-call results in the Gemma content-fence style.
    ///
    /// Pushes an assistant message with the rendered `tool_code` fence, then a
    /// user message containing `TOOL RESULT` blocks and a continue trailer.
    ///
    /// # Errors
    /// Returns an error, leaving the conversation unmodified, when `calls` and
    /// `results` fail [`correlate_tool_results`]. Correlation is validated in
    /// every build (not just debug) before either message is appended, so a
    /// count, ordering, or id-correlation break can never silently truncate or
    /// mislabel a result via `zip`.
    fn echo_tool_results(
        &self,
        conversation: &mut Vec<Message>,
        calls: &[ToolCall],
        results: &[FramedToolResult],
    ) -> Result<()> {
        correlate_tool_results(calls, results)?;

        let fence = render_tool_code_fence(calls)?;
        conversation.push(Message::assistant(fence));

        // Each result content is already trust-framed by the executor; it is
        // embedded verbatim as opaque data. The continuation is tool-neutral and
        // sits outside the data blocks, so it never assumes a specific tool
        // (e.g. fetch) and cannot be read as part of a result's framed payload.
        let mut parts: Vec<String> = Vec::with_capacity(results.len());
        for (call, result) in calls.iter().zip(results.iter()) {
            parts.push(format!(
                "TOOL RESULT {} ({}):\n{}",
                call.name,
                result.id(),
                result.content()
            ));
        }
        let mut follow_up = parts.join("\n\n");
        follow_up.push_str(
            "\n\nContinue the protocol: emit another tool_code fence to call a tool, or write your final answer using only the tool results above.",
        );
        conversation.push(Message::user(follow_up));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::CompletionResult;

    #[test]
    fn sole_tool_code_fence_becomes_tool_calls() {
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```tool_code\nsearch(query=\"C++ Alliance founder\")\n```"
                },
                "finish_reason": "stop"
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_tool_code_0");
                assert_eq!(calls[0].name, "search");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "query": "C++ Alliance founder" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn positional_search_maps_to_query() {
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```tool_code\nsearch(\"C++ Alliance organization\")\n```"
                }
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls[0].name, "search");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "query": "C++ Alliance organization" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn positional_fetch_maps_to_url() {
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```tool_code\nfetch(\"https://cppalliance.org\")\n```"
                }
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls[0].name, "fetch");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "url": "https://cppalliance.org" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn empty_content_carries_the_wire_finish_reason() {
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "stop"
            }]
        });
        match dialect.parse_turn(&body) {
            Err(Error::EmptyModelReply { finish_reason, .. }) => {
                assert_eq!(
                    finish_reason.as_deref(),
                    Some("stop"),
                    "the choice's finish_reason must survive on the error"
                );
            }
            other => panic!("expected EmptyModelReply, got {other:?}"),
        }
    }

    #[test]
    fn mixed_prose_stays_text() {
        let dialect = Gemma3ToolCodeDialect;
        let content = "Here is evidence.\n\n```tool_code\nsearch(query=\"x\")\n```\n";
        let body = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": content }
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::Text(text) => assert_eq!(text, content),
            CompletionResult::ToolCalls(_) => panic!("mixed prose must stay text"),
        }
    }

    #[test]
    fn fenced_json_tool_calls_parsed() {
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```json\n{\"tool_calls\":[{\"id\":\"1\",\"type\":\"function\",\"function\":{\"name\":\"fetch\",\"arguments\":\"{\\\"url\\\":\\\"https://example.com\\\"}\"}}]}\n```"
                }
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls[0].name, "fetch");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "url": "https://example.com" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn fenced_json_with_malformed_arguments_is_malformed_not_text() {
        // A fenced tool_calls blob (protocol intent) whose arguments are invalid
        // must surface as a malformed error, preserving the concrete decode
        // failure, rather than silently falling back to final text.
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```json\n{\"tool_calls\":[{\"id\":\"1\",\"type\":\"function\",\"function\":{\"name\":\"fetch\",\"arguments\":\"not json\"}}]}\n```"
                }
            }]
        });
        assert!(
            matches!(dialect.parse_turn(&body), Err(Error::MalformedResponse(_))),
            "malformed fenced arguments must be a malformed error, not text"
        );
    }

    #[test]
    fn malformed_tool_code_fence_is_malformed_not_text() {
        let dialect = Gemma3ToolCodeDialect;
        // Leading tool_code fence commits to protocol; a bad call line is an error.
        let bad_call = serde_json::json!({
            "choices": [{ "message": { "content": "```tool_code\nnot a call\n```" } }]
        });
        assert!(matches!(
            dialect.parse_turn(&bad_call),
            Err(Error::MalformedResponse(_))
        ));

        // Unterminated fence is malformed, not text.
        let unterminated = serde_json::json!({
            "choices": [{ "message": { "content": "```tool_code\nsearch(query=\"a\")" } }]
        });
        assert!(matches!(
            dialect.parse_turn(&unterminated),
            Err(Error::MalformedResponse(_))
        ));

        // Trailing prose after a valid fence in a protocol turn is malformed.
        let trailing = serde_json::json!({
            "choices": [{ "message": { "content": "```tool_code\nsearch(query=\"a\")\n```\nthen prose" } }]
        });
        assert!(matches!(
            dialect.parse_turn(&trailing),
            Err(Error::MalformedResponse(_))
        ));
    }

    #[test]
    fn backticks_inside_quoted_value_do_not_close_fence() {
        // A ``` inside a quoted argument value must not terminate the fence at
        // the first triple-backtick; only a standalone ``` line closes it.
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "```tool_code\necho(value=\"a ``` b\")\n```" } }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "value": "a ``` b" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn ordinary_json_data_fence_stays_text() {
        // A ```json code block that is not a tool_calls blob is ordinary data.
        let dialect = Gemma3ToolCodeDialect;
        let content = "```json\n{\"answer\": 42}\n```";
        let body = serde_json::json!({
            "choices": [{ "message": { "content": content } }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        assert!(matches!(turn.outcome, CompletionResult::Text(_)));
    }

    #[test]
    fn prepare_request_strips_tools_and_injects_guide() {
        let dialect = Gemma3ToolCodeDialect;
        let mut body = serde_json::json!({
            "model": "gemma-3",
            "messages": [{"role": "user", "content": "brief me"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "search",
                    "description": "Web search",
                    "parameters": {
                        "type": "object",
                        "properties": { "query": { "type": "string" } },
                        "required": ["query"]
                    }
                }
            }],
            "tool_choice": "auto"
        });
        let mut req = DialectRequest::new(&mut body);
        dialect.prepare_request(&mut req).unwrap();
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["model"], "gemma-3");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        let guide = messages[0]["content"].as_str().unwrap();
        assert!(guide.contains("search(query=...)"));
        assert!(guide.contains("```tool_code"));
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn prepare_request_rejects_invalid_shapes() {
        let dialect = Gemma3ToolCodeDialect;

        // Non-object body.
        let mut body = serde_json::json!([1, 2, 3]);
        let mut req = DialectRequest::new(&mut body);
        assert!(dialect.prepare_request(&mut req).is_err());

        // Non-array messages.
        let mut body = serde_json::json!({ "messages": "oops", "tools": [] });
        let mut req = DialectRequest::new(&mut body);
        assert!(dialect.prepare_request(&mut req).is_err());
        // Body must be untouched on failure (validated before mutating).
        assert!(body.get("tools").is_some(), "no mutation on rejected shape");

        // Non-array tools.
        let mut body = serde_json::json!({ "messages": [], "tools": {} });
        let mut req = DialectRequest::new(&mut body);
        assert!(dialect.prepare_request(&mut req).is_err());
        assert!(body.get("tools").is_some(), "no mutation on rejected shape");
    }

    #[test]
    fn detect_gemma_props_scores() {
        let dialect = Gemma3ToolCodeDialect;
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some("<start_of_turn>user\n".to_string()),
            model_id: Some("gemma-3-27b-it".to_string()),
            ..Default::default()
        };
        let score = dialect.detect(&evidence).expect("should score");
        assert!(score.0 >= 80, "expected high score, got {}", score.0);
    }

    #[test]
    fn detect_with_native_tools_returns_none() {
        let dialect = Gemma3ToolCodeDialect;
        let evidence = DialectEvidence {
            supports_tool_calls: Some(true),
            chat_template: Some("<start_of_turn>user\n".to_string()),
            model_id: Some("gemma-3-27b-it".to_string()),
            ..Default::default()
        };
        assert!(dialect.detect(&evidence).is_none());
    }

    #[test]
    fn detect_tools_false_without_gemma_fingerprint_returns_none() {
        let dialect = Gemma3ToolCodeDialect;
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some("{{ messages }}".to_string()),
            model_id: Some("qwen3-0.6b".to_string()),
            ..Default::default()
        };
        assert!(dialect.detect(&evidence).is_none());
    }

    #[test]
    fn consecutive_fences_mint_unique_ids() {
        let dialect = Gemma3ToolCodeDialect;
        let content =
            "```tool_code\nsearch(query=\"a\")\n```\n\n```tool_code\nfetch(\"https://x\")\n```";
        let body = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": content } }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].id, "call_tool_code_0");
                assert_eq!(
                    calls[1].id, "call_tool_code_1",
                    "ids must not restart per fence"
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn echo_rejects_uncorrelated_results() {
        let dialect = Gemma3ToolCodeDialect;
        let calls = vec![ToolCall {
            id: "call_tool_code_0".into(),
            name: "search".into(),
            arguments: serde_json::json!({ "query": "rust" }),
        }];
        // Wrong id breaks correlation; the conversation must stay untouched.
        let results = vec![FramedToolResult::new("mismatched_id".into(), "text".into())];
        let mut conversation = Vec::new();
        let error = dialect
            .echo_tool_results(&mut conversation, &calls, &results)
            .expect_err("uncorrelated results must be rejected");
        assert!(matches!(error, Error::Internal(_)));
        assert!(
            conversation.is_empty(),
            "no message may be appended on failure"
        );
    }

    #[test]
    fn optional_params_survive_in_guide() {
        let dialect = Gemma3ToolCodeDialect;
        let mut body = serde_json::json!({
            "model": "gemma-3",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "search",
                    "parameters": {
                        "type": "object",
                        "properties": { "query": { "type": "string" }, "count": { "type": "number" } },
                        "required": ["query"]
                    }
                }
            }]
        });
        let mut req = DialectRequest::new(&mut body);
        dialect.prepare_request(&mut req).unwrap();
        let guide = body["messages"][0]["content"].as_str().unwrap();
        assert!(
            guide.contains("query=..."),
            "required param present: {guide}"
        );
        assert!(
            guide.contains("count=...?"),
            "optional param must survive marked: {guide}"
        );
    }

    #[test]
    fn echo_tool_results_produces_user_turn() {
        let dialect = Gemma3ToolCodeDialect;
        let calls = vec![ToolCall {
            id: "call_tool_code_0".into(),
            name: "search".into(),
            arguments: serde_json::json!({"query": "rust"}),
        }];
        let results = vec![FramedToolResult::new(
            "call_tool_code_0".into(),
            "result text".into(),
        )];
        let mut conversation = Vec::new();
        dialect
            .echo_tool_results(&mut conversation, &calls, &results)
            .expect("correlated results echo cleanly");

        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation[0].role, "assistant");
        assert!(conversation[0].content.contains("```tool_code"));
        assert!(conversation[0].content.contains("search("));
        assert_eq!(conversation[1].role, "user");
        assert!(
            conversation[1]
                .content
                .contains("TOOL RESULT search (call_tool_code_0):")
        );
        assert!(conversation[1].content.contains("result text"));
        assert!(conversation[1].content.contains("Continue the protocol"));
    }
}
