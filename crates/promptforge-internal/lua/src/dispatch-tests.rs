//! Tests for the shared tool-dispatch body: the fixture tools and recorder
//! every dispatch test uses, and the synchronous `prepare_dispatch` tests.

use promptforge_api_types::tools::{
    OutputTrust, ToolDescriptor, ToolError, ToolErrorKind, ToolId, ToolOutput,
};
use serde_json::json;

use super::*;
use crate::tests::recording::{Recorder, detail};

const SECTION: &str = "Test";

/// The `echo` fixture tool as data. `prepare_dispatch` sees only the
/// binding and a canned output; no implementation is ever called.
fn echo_tool() -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse("tests/tools/echo").expect("valid id"),
        "echo",
        "echo the value argument",
        json!({ "type": "object" }),
    )
}

/// The `failing` fixture tool as data.
fn failing_tool() -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse("tests/tools/failing").expect("valid id"),
        "failing",
        "always fail",
        json!({ "type": "object" }),
    )
}

/// The nonce a dispatch test wraps under.
fn nonce() -> GuardNonce {
    GuardNonce::from_seed(0xd15_9a7c)
}

fn binding(alias: &str, tool: &ToolDescriptor) -> ToolBinding {
    ToolBinding::for_test(alias, "fixture capability", tool)
}

#[test]
fn prepare_dispatch_wraps_a_canned_untrusted_output_counts_it_and_reports_it() {
    let recorder = Recorder::default();
    let counts = ToolCallCounts::new(["echo".to_owned()]);
    let echo = binding("echo", &echo_tool());
    let nonce = nonce();
    let outcome = prepare_dispatch(
        &echo,
        Ok(ToolOutput::untrusted("canned output")),
        Some(&counts),
        &nonce,
        recorder.emitter(),
        SECTION,
        Some(ScriptReport { turn: 3 }),
    )
    .expect("a canned output prepares without awaiting anything");
    assert_eq!(
        outcome.content(),
        nonce.wrap("canned output"),
        "an untrusted canned output is nonce-wrapped byte for byte"
    );
    assert_eq!(
        outcome.trust(),
        OutputTrust::Untrusted,
        "the untrusted marking survives"
    );
    assert_eq!(
        counts.get("echo").expect("the counts read"),
        Some(1),
        "preparing the outcome increments the alias count"
    );
    assert_eq!(
        recorder.kinds(),
        vec![detail::TOOL_CALL_SUCCEEDED],
        "a canned Ok output reports the succeeded observation"
    );
    assert_eq!(
        recorder.tool_results(),
        vec![(
            3,
            String::new(),
            "echo".to_owned(),
            nonce.wrap("canned output"),
            false,
        )],
        "a script-initiated preparation fires ToolResult with the wrapped text"
    );
}

#[test]
fn prepare_dispatch_turns_a_canned_tool_error_into_the_typed_error() {
    let recorder = Recorder::default();
    let failing = binding("failing", &failing_tool());
    let error = prepare_dispatch(
        &failing,
        Err(ToolError::message("canned failure").with_kind(ToolErrorKind::Backend)),
        None,
        &nonce(),
        recorder.emitter(),
        SECTION,
        None,
    )
    .expect_err("a canned failure fails the preparation");
    assert!(
        matches!(error, Error::Tool { .. }),
        "the canned failure is the typed tool error, got {error:?}"
    );
    assert_eq!(
        recorder.kinds(),
        vec![detail::TOOL_CALL_FAILED],
        "a canned Err output reports the failed observation"
    );
}

fn model_report(call_id: &str) -> ModelReport {
    ModelReport {
        script: ScriptReport { turn: 1 },
        call_id: call_id.to_owned(),
    }
}

#[test]
fn a_model_issued_tool_failure_becomes_untrusted_failure_text_under_its_call_id() {
    let recorder = Recorder::default();
    let failing = binding("failing", &failing_tool());
    let nonce = nonce();
    let outcome = prepare_model_dispatch(
        &failing,
        Err(ToolError::message("the tool's own backend failed").with_kind(ToolErrorKind::Backend)),
        None,
        &nonce,
        recorder.emitter(),
        SECTION,
        &model_report("call_1"),
    )
    .expect("a model-issued call never fails for the tool's own failure");
    assert_eq!(
        outcome.trust(),
        OutputTrust::Untrusted,
        "the failure text is untrusted"
    );
    assert_eq!(
        outcome.content(),
        nonce.wrap("the tool's own backend failed"),
        "the failure text is the tool's message, nonce-wrapped"
    );
    assert_eq!(
        recorder.tool_results(),
        vec![(
            1,
            "call_1".to_owned(),
            "failing".to_owned(),
            nonce.wrap("the tool's own backend failed"),
            false,
        )],
        "ToolResult fires once, under the model's call id"
    );
    assert_eq!(recorder.kinds(), vec![detail::TOOL_CALL_FAILED],);
}

#[test]
fn a_model_issued_dispatch_reports_its_result_under_the_call_id() {
    let recorder = Recorder::default();
    let echo = binding("echo", &echo_tool());
    let outcome = prepare_model_dispatch(
        &echo,
        Ok(ToolOutput::trusted("echoed: hi")),
        None,
        &nonce(),
        recorder.emitter(),
        SECTION,
        &model_report("call_2"),
    )
    .expect("the dispatch succeeds");
    assert_eq!(outcome.content(), "echoed: hi");
    assert_eq!(
        recorder.tool_results(),
        vec![(
            1,
            "call_2".to_owned(),
            "echo".to_owned(),
            "echoed: hi".to_owned(),
            true,
        )],
    );
}
