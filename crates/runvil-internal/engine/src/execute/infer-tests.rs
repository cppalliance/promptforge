//! The nested-inference answer: a round that comes back with tool calls,
//! though none were advertised, fails as a malformed reply.

use runvil_types::ids::RoundId;
use serde_json::json;

use super::*;
use crate::model::{CompletionErrorKind, ToolCall};
use crate::test_support::recording::null_emitter;

#[test]
fn an_infer_round_answered_with_tool_calls_fails_as_a_retryable_malformed_reply() {
    let calls = CompletionResult::ToolCalls(vec![
        ToolCall::from_parts("call-1", "grab", json!({})).expect("a whole call"),
    ]);
    let completion = Completion::from_result(calls, "test-model").expect("a whole result");
    let error = accept_infer(
        Ok(Box::new(completion)),
        RoundId::new(0),
        &null_emitter(),
        "Infer",
        &AtomicU32::new(0),
    )
    .expect_err("no tools were advertised, so tool calls are refused");
    let Error::Completion(failure) = &error else {
        panic!("required a completion failure, actual {error:?}");
    };
    assert_eq!(failure.kind(), CompletionErrorKind::MalformedResponse);
    assert!(
        failure.is_retryable(),
        "a reply that broke protocol may come back well formed"
    );
    assert!(crate::RunError::from(error).is_retryable());
}
