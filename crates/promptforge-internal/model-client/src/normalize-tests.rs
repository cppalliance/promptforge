//! Tests for the neutral reply checks, driven through the validating
//! constructors that run them: each check's kind and exact message, which a
//! wire decoder building its calls through those constructors reports
//! verbatim.

use serde_json::json;

use super::*;
use crate::client::{Completion, CompletionResult, ToolCall};

const EMPTY_REPLY_PHRASE: &str = "the model replied with no text and no tool calls";
const MALFORMED_PHRASE: &str = "the model backend sent a reply that could not be understood";

/// The kind and message `ToolCall::from_parts` refuses these parts with.
fn refusal(id: &str, name: &str, arguments: Value) -> (CompletionErrorKind, String) {
    let error = ToolCall::from_parts(id, name, arguments).expect_err("the parts are refused");
    (error.kind(), error.to_string())
}

#[test]
fn blank_tool_call_id_is_rejected() {
    assert_eq!(
        refusal("   ", "ping", json!({})),
        (
            CompletionErrorKind::MalformedResponse,
            format!("{MALFORMED_PHRASE}: tool call id was blank")
        )
    );
}

#[test]
fn blank_tool_call_name_is_rejected() {
    assert_eq!(
        refusal("call_1", "\t", json!({})),
        (
            CompletionErrorKind::MalformedResponse,
            format!("{MALFORMED_PHRASE}: tool call name was blank")
        )
    );
}

#[test]
fn non_object_arguments_are_rejected() {
    for arguments in [json!([1, 2, 3]), json!("{}"), Value::Null] {
        assert_eq!(
            refusal("call_arr", "ping", arguments),
            (
                CompletionErrorKind::MalformedResponse,
                format!("{MALFORMED_PHRASE}: tool call arguments were not a JSON object")
            )
        );
    }
}

#[test]
fn duplicate_tool_call_ids_are_rejected() {
    let call = |name: &str| ToolCall::from_parts("dup", name, json!({})).expect("a whole call");
    let error =
        Completion::from_result(CompletionResult::ToolCalls(vec![call("a"), call("b")]), "m")
            .expect_err("two calls sharing an id are refused");
    assert_eq!(error.kind(), CompletionErrorKind::MalformedResponse);
    assert_eq!(
        error.to_string(),
        format!("{MALFORMED_PHRASE}: duplicate tool call id \"dup\" within one turn")
    );
}

#[test]
fn an_empty_tool_call_batch_is_an_empty_reply() {
    let error = Completion::from_result(CompletionResult::ToolCalls(Vec::new()), "m")
        .expect_err("an empty batch is refused");
    assert_eq!(error.kind(), CompletionErrorKind::EmptyReply);
    assert_eq!(error.to_string(), EMPTY_REPLY_PHRASE);
    assert_eq!(error.finish_reason(), None);
}
