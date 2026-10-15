//! Tests for the shared tool-dispatch body: the synchronous
//! `prepare_dispatch` and `prepare_model_dispatch` over fixture tools as
//! data and canned outputs, reported to a recorder.

use promptforge_types::tools::{OutputTrust, ToolDescriptor, ToolError, ToolErrorKind, ToolOutput};

use super::*;
use crate::test_vm::{TEST_SECTION, echo_tool, test_nonce, tool_at};
use crate::tests::recording::{Recorder, detail};

fn binding(alias: &str, tool: &ToolDescriptor) -> ToolBinding {
    ToolBinding::for_test(alias, "fixture capability", tool)
}

#[test]
fn prepare_dispatch_wraps_a_canned_untrusted_output_counts_it_and_reports_it() {
    let recorder = Recorder::default();
    let counts = ToolCallCounts::new(["tools/echo".to_owned()]);
    let echo = binding("echo", &echo_tool());
    let nonce = test_nonce();
    let outcome = prepare_dispatch(
        &echo,
        Ok(ToolOutput::untrusted("canned output")),
        Some(&counts),
        &nonce,
        recorder.emitter(),
        TEST_SECTION,
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
        counts.get("tools/echo").expect("the counts read"),
        Some(1),
        "preparing the outcome increments the tool's count under its id"
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
    assert_eq!(
        recorder.tool_result_tools(),
        vec![Some(echo_tool().id)],
        "the script's result names the tool its alias is bound to"
    );
}

#[test]
fn prepare_dispatch_turns_a_canned_tool_error_into_the_typed_error() {
    let recorder = Recorder::default();
    let failing = binding("failing", &tool_at("tools/failing"));
    let error = prepare_dispatch(
        &failing,
        Err(ToolError::message("canned failure").with_kind(ToolErrorKind::Backend)),
        None,
        &test_nonce(),
        recorder.emitter(),
        TEST_SECTION,
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
    let failing = binding("failing", &tool_at("tools/failing"));
    let nonce = test_nonce();
    let outcome = prepare_model_dispatch(
        &failing,
        Err(ToolError::message("the tool's own backend failed").with_kind(ToolErrorKind::Backend)),
        None,
        &nonce,
        recorder.emitter(),
        TEST_SECTION,
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
        &test_nonce(),
        recorder.emitter(),
        TEST_SECTION,
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
    assert_eq!(
        recorder.tool_result_tools(),
        vec![Some(echo_tool().id)],
        "the model's result names the tool its alias is bound to"
    );
}
