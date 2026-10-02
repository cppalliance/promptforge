//! The reply builders the suites script their [`ScriptedChat`] with, and
//! the scripted clients that fail every round.

use super::*;

/// The model every scripted reply names as its server.
pub(super) const MOCK_MODEL: &str = "mock-model";

/// One scripted tool call from its id, wire name, and JSON-encoded
/// arguments.
pub(super) fn scripted_call(id: &str, name: &str, arguments: &str) -> (String, String, Value) {
    (
        id.to_owned(),
        name.to_owned(),
        serde_json::from_str(arguments).expect("scripted tool-call arguments are JSON"),
    )
}

/// A text reply naming [`MOCK_MODEL`], with the given finish reason and no
/// reasoning or metrics.
fn scripted_text(content: &str, finish_reason: Option<&str>) -> ScriptedReply {
    ScriptedReply::Text {
        model: MOCK_MODEL.to_owned(),
        content: content.to_owned(),
        finish_reason: finish_reason.map(str::to_owned),
        reasoning: None,
        metrics: None,
    }
}

/// A response asking the model to call one tool.
pub(super) fn resp_tool_call(id: &str, name: &str, arguments: &str) -> ScriptedReply {
    ScriptedReply::ToolCalls {
        model: MOCK_MODEL.to_owned(),
        calls: vec![scripted_call(id, name, arguments)],
    }
}

/// A response asking the model to call one tool twice in a single turn.
pub(super) fn resp_two_tool_calls(
    name: &str,
    first: (&str, &str),
    second: (&str, &str),
) -> ScriptedReply {
    ScriptedReply::ToolCalls {
        model: MOCK_MODEL.to_owned(),
        calls: vec![
            scripted_call(first.0, name, first.1),
            scripted_call(second.0, name, second.1),
        ],
    }
}

/// A final assistant text reply.
pub(super) fn resp_text(content: &str) -> ScriptedReply {
    scripted_text(content, None)
}

/// A delayed final assistant text reply for in-flight cancellation tests.
pub(super) fn resp_delayed_text(content: &str, delay: std::time::Duration) -> ScriptedReply {
    ScriptedReply::Delayed(delay, Box::new(resp_text(content)))
}

/// A final assistant text reply with an explicit `finish_reason`.
pub(super) fn resp_text_finish(content: &str, finish_reason: &str) -> ScriptedReply {
    scripted_text(content, Some(finish_reason))
}

/// A failed round of `kind`, as the gateway client reports a non-success
/// `status` with `body`: the kind's phrase with the status appended, and
/// the body as the detail.
pub(super) fn resp_failure(
    kind: crate::model::CompletionErrorKind,
    status: u16,
    body: &str,
) -> ScriptedReply {
    ScriptedReply::Failure {
        kind,
        message: format!("{} (status {status})", kind.phrase()),
        detail: Some(body.to_owned()),
        finish_reason: None,
    }
}

/// The two-round `echo` tool-call-then-text script most loop tests use.
pub(super) fn echo_then_text_script() -> Vec<ScriptedReply> {
    vec![
        resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ]
}

/// A tool-call under `alias` on the first round, then a final text reply.
pub(super) fn aliased_tool_script(alias: &str) -> Vec<ScriptedReply> {
    vec![
        resp_tool_call("aliased_call", alias, "{\"value\":\"payload\"}"),
        resp_text("aliased final"),
    ]
}

/// A chat client whose every round fails with a broker-reported context
/// overflow, so a test drives the provider overflow path. It counts the
/// rounds it was asked to perform.
#[derive(Clone, Default)]
pub(super) struct OverflowClient {
    pub(super) calls: Arc<AtomicUsize>,
}

impl crate::test_support::ChatClient for OverflowClient {
    fn complete(
        &self,
        _messages: Vec<crate::model::Message>,
        _tools: Vec<crate::model::ToolSchema>,
        _options: crate::model::CompletionOptions,
        _limits: RunLimits,
        _on_delta: Option<crate::test_support::DeltaHook>,
    ) -> crate::test_support::BoxFuture<
        std::result::Result<crate::model::Completion, crate::model::CompletionError>,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(crate::model::CompletionError::context_overflow(
                Some(5000),
                Some(4096),
                "the request is larger than the model's context window",
            ))
        })
    }
}

/// Asserts `error` is a model failure of `kind` whose message is `message`.
pub(super) fn assert_model_failure(
    error: &Error,
    kind: crate::model::CompletionErrorKind,
    message: &str,
) {
    let Error::Completion(failure) = error else {
        panic!("expected a model failure, got {error:?}");
    };
    assert_eq!(
        (failure.kind(), failure.to_string().as_str()),
        (kind, message)
    );
}
