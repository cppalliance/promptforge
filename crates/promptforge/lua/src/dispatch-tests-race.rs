//! Tests for the async wrappers that still perform the call here:
//! `dispatch_tool`'s cancel race and `dispatch_model_tool`'s model-loop
//! failure rule.

use promptforge_api_types::cancel::CancelHandle;
use promptforge_api_types::observe::NullObserver;

use super::*;

/// Sleeps far past any test deadline, so only cancellation can end it.
struct SlowTool;

#[async_trait::async_trait]
impl Tool for SlowTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/slow").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn wire_name(&self) -> &str {
        "slow"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "a deliberately slow tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    async fn call(&self, _args: serde_json::Value) -> std::result::Result<ToolOutput, ToolError> {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        Ok(ToolOutput::trusted("too late"))
    }
}

fn model_report(call_id: &str) -> ModelReport {
    ModelReport {
        script: ScriptReport {
            chain_id: 4,
            depth: 0,
            turn: 1,
        },
        call_id: call_id.to_owned(),
    }
}

#[tokio::test]
async fn a_trusted_output_passes_verbatim_and_counts_increment() {
    let counts = ToolCallCounts::new(["echo".to_owned()]);
    let echo = binding("echo", Arc::new(EchoTool { trusted: true }));
    let outcome = dispatch_tool(
        &echo,
        json!({ "value": "hi" }),
        Some(&counts),
        &GuardNonce::fresh(),
        &NullObserver::default(),
        EXECUTION,
        SECTION,
        None,
    )
    .await
    .expect("the dispatch succeeds");
    assert_eq!(outcome.content(), "echoed: hi");
    assert!(outcome.trusted(), "a trusted output keeps its marking");
    assert_eq!(
        counts.get("echo").expect("the counts read"),
        Some(1),
        "an attempted dispatch increments the alias count"
    );
}

#[tokio::test]
async fn an_untrusted_output_is_nonce_wrapped() {
    let echo = binding("echo", Arc::new(EchoTool { trusted: false }));
    let outcome = dispatch_tool(
        &echo,
        json!({ "value": "hi" }),
        None,
        &GuardNonce::fresh(),
        &NullObserver::default(),
        EXECUTION,
        SECTION,
        None,
    )
    .await
    .expect("the dispatch succeeds");
    assert!(
        !outcome.trusted(),
        "an untrusted output reports its marking"
    );
    let content = outcome.content();
    assert!(
        content.contains("<untrusted_input_") && content.contains("</untrusted_input_"),
        "an untrusted output must be wrapped, got: {content}"
    );
    assert!(
        content.contains("echoed: hi"),
        "the wrapped block must still carry the tool output, got: {content}"
    );
}

#[tokio::test(start_paused = true)]
async fn cancellation_interrupts_the_dispatch_and_still_counts_the_attempt() {
    let recorder = Recorder::default();
    let counts = ToolCallCounts::new(["slow".to_owned()]);
    let slow = binding("slow", Arc::new(SlowTool));
    let handle = CancelHandle::new();
    handle.cancel();
    let result = cancel::scope(
        handle,
        dispatch_tool(
            &slow,
            json!({}),
            Some(&counts),
            &GuardNonce::fresh(),
            &recorder,
            EXECUTION,
            SECTION,
            None,
        ),
    )
    .await;
    assert!(
        matches!(result, Err(Error::Interrupted)),
        "a cancelled dispatch must interrupt, got {result:?}"
    );
    assert_eq!(
        *recorder
            .observations
            .lock()
            .expect("the recorder mutex must not be poisoned"),
        vec![Observation::ToolCallFailed],
        "a cancelled dispatch reports the failed observation"
    );
    assert_eq!(
        counts.get("slow").expect("the counts read"),
        Some(1),
        "the attempt counts before the race, so a cancelled dispatch still counts"
    );
}

#[tokio::test]
async fn a_tool_failure_is_a_typed_tool_error_with_its_cause() {
    let recorder = Recorder::default();
    let failing = binding("failing", Arc::new(FailingTool));
    let error = dispatch_tool(
        &failing,
        json!({}),
        None,
        &GuardNonce::fresh(),
        &recorder,
        EXECUTION,
        SECTION,
        None,
    )
    .await
    .expect_err("the failing tool must fail the dispatch");
    match &error {
        Error::Tool { source, .. } => {
            assert!(
                source.downcast_ref::<ToolError>().is_some(),
                "the tool's typed error must survive as the cause"
            );
        }
        other => panic!("expected the typed tool error, got {other:?}"),
    }
    assert_eq!(
        *recorder
            .observations
            .lock()
            .expect("the recorder mutex must not be poisoned"),
        vec![Observation::ToolCallFailed],
    );
}

#[tokio::test]
async fn a_script_report_fires_on_tool_result_exactly_once() {
    let recorder = Recorder::default();
    let echo = binding("echo", Arc::new(EchoTool { trusted: true }));
    dispatch_tool(
        &echo,
        json!({ "value": "hi" }),
        None,
        &GuardNonce::fresh(),
        &recorder,
        EXECUTION,
        SECTION,
        Some(ScriptReport {
            chain_id: 3,
            depth: 1,
            turn: 2,
        }),
    )
    .await
    .expect("the dispatch succeeds");
    assert_eq!(
        *recorder
            .tool_results
            .lock()
            .expect("the recorder mutex must not be poisoned"),
        vec![(
            3,
            1,
            2,
            String::new(),
            "echo".to_owned(),
            "echoed: hi".to_owned(),
            true,
        )],
        "a script-initiated dispatch reports its result exactly once"
    );
}

#[tokio::test]
async fn a_model_loop_dispatch_fires_no_content_report() {
    let recorder = Recorder::default();
    let echo = binding("echo", Arc::new(EchoTool { trusted: true }));
    dispatch_tool(
        &echo,
        json!({ "value": "hi" }),
        None,
        &GuardNonce::fresh(),
        &recorder,
        EXECUTION,
        SECTION,
        None,
    )
    .await
    .expect("the dispatch succeeds");
    assert!(
        recorder
            .tool_results
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .is_empty(),
        "a model-loop dispatch must fire no on_tool_result"
    );
}

#[tokio::test]
async fn a_model_issued_tool_failure_becomes_untrusted_failure_text_under_its_call_id() {
    let recorder = Recorder::default();
    let failing = binding("failing", Arc::new(FailingTool));
    let nonce = GuardNonce::fresh();
    let outcome = dispatch_model_tool(
        &failing,
        json!({}),
        None,
        &nonce,
        &recorder,
        EXECUTION,
        SECTION,
        &model_report("call_1"),
    )
    .await
    .expect("a model-issued call never fails for the tool's own failure");
    assert!(!outcome.trusted(), "the failure text is untrusted");
    assert_eq!(
        outcome.content(),
        nonce.wrap("the tool's own backend failed"),
        "the failure text is the tool's message, nonce-wrapped"
    );
    assert_eq!(
        *recorder
            .tool_results
            .lock()
            .expect("the recorder mutex must not be poisoned"),
        vec![(
            4,
            0,
            1,
            "call_1".to_owned(),
            "failing".to_owned(),
            nonce.wrap("the tool's own backend failed"),
            false,
        )],
        "ToolResult fires once, under the model's call id"
    );
    assert_eq!(
        *recorder
            .observations
            .lock()
            .expect("the recorder mutex must not be poisoned"),
        vec![Observation::ToolCallFailed],
    );
}

#[tokio::test]
async fn a_model_issued_dispatch_reports_its_result_under_the_call_id() {
    let recorder = Recorder::default();
    let echo = binding("echo", Arc::new(EchoTool { trusted: true }));
    let outcome = dispatch_model_tool(
        &echo,
        json!({ "value": "hi" }),
        None,
        &GuardNonce::fresh(),
        &recorder,
        EXECUTION,
        SECTION,
        &model_report("call_2"),
    )
    .await
    .expect("the dispatch succeeds");
    assert_eq!(outcome.content(), "echoed: hi");
    assert_eq!(
        *recorder
            .tool_results
            .lock()
            .expect("the recorder mutex must not be poisoned"),
        vec![(
            4,
            0,
            1,
            "call_2".to_owned(),
            "echo".to_owned(),
            "echoed: hi".to_owned(),
            true,
        )],
    );
}

#[tokio::test(start_paused = true)]
async fn a_model_issued_dispatch_still_propagates_cancellation() {
    let slow = binding("slow", Arc::new(SlowTool));
    let handle = CancelHandle::new();
    handle.cancel();
    let result = cancel::scope(
        handle,
        dispatch_model_tool(
            &slow,
            json!({}),
            None,
            &GuardNonce::fresh(),
            &NullObserver::default(),
            EXECUTION,
            SECTION,
            &model_report("call_3"),
        ),
    )
    .await;
    assert!(
        matches!(result, Err(Error::Interrupted)),
        "only the tool's own failure becomes content; cancellation propagates, got {result:?}"
    );
}
