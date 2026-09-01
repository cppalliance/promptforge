//! The shared tool-dispatch body every executor invokes.
//!
//! [`dispatch_tool`] is the one place a bound tool's call composes the
//! cancel race, the per-VM call counts, the untrusted nonce wrap, and the
//! observer events. Core's model tool loop and its scheduler's `tool_call`
//! arm both call it; the agent driver adopts it unchanged. Keeping the body
//! here - the crate every executor already depends on - is what stops
//! dispatch semantics from forking.

use promptforge_core_support::cancel;
use promptforge_core_support::observe::{Observer, detail};
use promptforge_core_support::untrusted::GuardNonce;
use promptforge_tools::OutputTrust;

use crate::error::{Error, Result};
use crate::{ToolBinding, ToolCallCounts};

/// The run coordinates a script-initiated dispatch reports under.
///
/// [`dispatch_tool`] fires [`Observer::on_tool_result`] with them; a model
/// tool-loop dispatch passes `None` instead, because its results ride the
/// conversation echo, not the content stream. A script call carries no
/// model-issued call id, so the report's `tool_call_id` is empty.
#[derive(Debug, Clone, Copy)]
pub struct ScriptReport {
    /// The chain the call fired in.
    pub chain_id: u32,
    /// The calling chain's execute depth.
    pub depth: u32,
    /// The section's completed model-turn count at dispatch.
    pub turn: u32,
}

/// Dispatches one bound tool call: the shared body the executors invoke.
///
/// The sequence is fixed: the counts increment (dispatch attempted, even if
/// the tool later errors), the call raced against cancellation, the
/// succeeded/failed observation, then the trust rule - a trusted output
/// passes verbatim, anything else is nonce-wrapped before it can reach a
/// model turn or a calling script. A script-initiated call (`script` is
/// `Some`) also fires [`Observer::on_tool_result`] with the final content;
/// a model-loop call fires no content event here.
///
/// # Errors
/// Returns [`Error::Interrupted`] when the run is cancelled mid-call,
/// [`Error::Tool`] when the tool itself fails (its typed error retained as
/// the cause), or the counts' own error when `binding`'s alias was never
/// seeded.
#[expect(
    clippy::too_many_arguments,
    reason = "the dispatch body names its full run coordinates in one call, exactly as the loop it was extracted from did"
)]
pub async fn dispatch_tool(
    binding: &ToolBinding,
    args: serde_json::Value,
    counts: Option<&ToolCallCounts>,
    nonce: &GuardNonce,
    observer: &dyn Observer,
    execution: &str,
    section: &str,
    script: Option<ScriptReport>,
) -> Result<String> {
    if let Some(counts) = counts {
        counts.increment(binding.alias())?;
    }
    // Race the tool call against cancellation so a slow or stuck tool
    // cannot hold the run past a Ctrl-C. On cancel the tool future is
    // dropped and the run ends promptly.
    let call_result = tokio::select! {
        biased;
        () = cancel::wait_cancelled() => {
            observer.observe(execution, section, detail::TOOL_CALL_FAILED);
            return Err(Error::Interrupted);
        }
        result = binding.tool().call(args) => result,
    };
    observer.observe(
        execution,
        section,
        if call_result.is_ok() {
            detail::TOOL_CALL_SUCCEEDED
        } else {
            detail::TOOL_CALL_FAILED
        },
    );
    let output = call_result.map_err(Error::tool)?;
    // Trust travels with the output: an untrusted result is nonce-wrapped
    // before it can reach the next model turn or the calling script. Every
    // wrap in the run shares the run's nonce, so identical content yields a
    // byte-identical envelope and KV-cache prefixes stay shared across
    // rounds and fanout arms; the `<`-escaping is what actually blocks a
    // forged close tag, so the reuse costs nothing.
    let (content, trusted) = match output.trust() {
        OutputTrust::Trusted => (output.text().to_owned(), true),
        // `OutputTrust` is `#[non_exhaustive]` in the contract crate: an
        // unknown future variant takes the safe path and is nonce-wrapped
        // as untrusted.
        _ => (nonce.wrap(output.text()), false),
    };
    if let Some(report) = script {
        observer.on_tool_result(
            execution,
            section,
            report.chain_id,
            report.depth,
            report.turn,
            "",
            binding.alias(),
            &content,
            trusted,
        );
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use promptforge_core_support::cancel::CancelHandle;
    use promptforge_core_support::observe::{NullObserver, Observation};
    use promptforge_tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput};
    use serde_json::json;

    use super::*;

    const EXECUTION: &str = "dispatch-test";
    const SECTION: &str = "Test";

    /// Echoes the `value` argument, trusted or untrusted per construction.
    struct EchoTool {
        trusted: bool,
    }

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn id(&self) -> ToolId {
            ToolId::new("tests", "echo").expect("valid id")
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn wire_name(&self) -> &str {
            "echo"
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn description(&self) -> &str {
            "echo the value argument"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }

        async fn call(
            &self,
            args: serde_json::Value,
        ) -> std::result::Result<ToolOutput, ToolError> {
            let text = format!("echoed: {}", args["value"].as_str().unwrap_or_default());
            Ok(if self.trusted {
                ToolOutput::trusted(text)
            } else {
                ToolOutput::untrusted(text)
            })
        }
    }

    /// Fails every call with a typed backend error carrying a cause.
    struct FailingTool;

    #[async_trait::async_trait]
    impl Tool for FailingTool {
        fn id(&self) -> ToolId {
            ToolId::new("tests", "failing").expect("valid id")
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn wire_name(&self) -> &str {
            "failing"
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn description(&self) -> &str {
            "always fail"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }

        async fn call(
            &self,
            _args: serde_json::Value,
        ) -> std::result::Result<ToolOutput, ToolError> {
            let cause = std::io::Error::other("upstream socket reset");
            Err(
                ToolError::with_source("the tool's own backend failed", cause)
                    .with_kind(ToolErrorKind::Backend),
            )
        }
    }

    /// Sleeps far past any test deadline, so only cancellation can end it.
    struct SlowTool;

    #[async_trait::async_trait]
    impl Tool for SlowTool {
        fn id(&self) -> ToolId {
            ToolId::new("tests", "slow").expect("valid id")
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

        async fn call(
            &self,
            _args: serde_json::Value,
        ) -> std::result::Result<ToolOutput, ToolError> {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok(ToolOutput::trusted("too late"))
        }
    }

    /// One recorded `on_tool_result` report: chain id, depth, turn, call
    /// id, alias, content, and the trusted flag, field for field.
    type ToolResultRecord = (u32, u32, u32, String, String, String, bool);

    /// Records fixed observations and `on_tool_result` content reports.
    #[derive(Default)]
    struct Recorder {
        observations: Mutex<Vec<Observation>>,
        tool_results: Mutex<Vec<ToolResultRecord>>,
    }

    impl Observer for Recorder {
        fn observe(&self, _execution: &str, _section: &str, event: Observation) {
            self.observations
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .push(event);
        }

        fn on_tool_result(
            &self,
            _execution: &str,
            _section: &str,
            chain_id: u32,
            depth: u32,
            turn: u32,
            tool_call_id: &str,
            alias: &str,
            content: &str,
            trusted: bool,
        ) {
            self.tool_results
                .lock()
                .expect("the recorder mutex must not be poisoned")
                .push((
                    chain_id,
                    depth,
                    turn,
                    tool_call_id.to_owned(),
                    alias.to_owned(),
                    content.to_owned(),
                    trusted,
                ));
        }
    }

    fn binding(alias: &str, tool: Arc<dyn Tool>) -> ToolBinding {
        ToolBinding::for_test(alias, "fixture capability", tool)
    }

    #[tokio::test]
    async fn a_trusted_output_passes_verbatim_and_counts_increment() {
        let counts = ToolCallCounts::new(["echo".to_owned()]);
        let echo = binding("echo", Arc::new(EchoTool { trusted: true }));
        let content = dispatch_tool(
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
        assert_eq!(content, "echoed: hi");
        assert_eq!(
            counts.get("echo").expect("the counts read"),
            Some(1),
            "an attempted dispatch increments the alias count"
        );
    }

    #[tokio::test]
    async fn an_untrusted_output_is_nonce_wrapped() {
        let echo = binding("echo", Arc::new(EchoTool { trusted: false }));
        let content = dispatch_tool(
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
            content.contains("<untrusted_input_") && content.contains("</untrusted_input_"),
            "an untrusted output must be wrapped, got: {content}"
        );
        assert!(
            content.contains("echoed: hi"),
            "the wrapped block must still carry the tool output, got: {content}"
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_the_dispatch() {
        let recorder = Recorder::default();
        let slow = binding("slow", Arc::new(SlowTool));
        let handle = CancelHandle::new();
        handle.cancel();
        let result = cancel::scope(
            handle,
            dispatch_tool(
                &slow,
                json!({}),
                None,
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
}
