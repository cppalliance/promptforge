//! Tests for the shared tool-dispatch body: the fixture tools and recorder
//! every dispatch test uses, and the synchronous `prepare_dispatch` tests.
//! The async wrappers are tested in `dispatch-tests-race.rs`.

use std::sync::{Arc, Mutex};

use promptforge_api_types::observe::Observation;
use promptforge_api_types::tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput};
use serde_json::json;

use super::*;

#[path = "dispatch-tests-race.rs"]
mod race;

const EXECUTION: &str = "dispatch-test";
const SECTION: &str = "Test";

/// Echoes the `value` argument, trusted or untrusted per construction.
struct EchoTool {
    trusted: bool,
}

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/echo").expect("valid id")
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

    async fn call(&self, args: serde_json::Value) -> std::result::Result<ToolOutput, ToolError> {
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
        ToolId::parse("tests/tools/failing").expect("valid id")
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

    async fn call(&self, _args: serde_json::Value) -> std::result::Result<ToolOutput, ToolError> {
        let cause = std::io::Error::other("upstream socket reset");
        Err(
            ToolError::with_source("the tool's own backend failed", cause)
                .with_kind(ToolErrorKind::Backend),
        )
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

#[test]
fn prepare_dispatch_wraps_a_canned_untrusted_output_counts_it_and_reports_it() {
    let recorder = Recorder::default();
    let counts = ToolCallCounts::new(["echo".to_owned()]);
    let echo = binding("echo", Arc::new(EchoTool { trusted: false }));
    let nonce = GuardNonce::fresh();
    let outcome = prepare_dispatch(
        &echo,
        Ok(ToolOutput::untrusted("canned output")),
        Some(&counts),
        &nonce,
        &recorder,
        EXECUTION,
        SECTION,
        Some(ScriptReport {
            chain_id: 7,
            depth: 2,
            turn: 3,
        }),
    )
    .expect("a canned output prepares without awaiting anything");
    assert_eq!(
        outcome.content(),
        nonce.wrap("canned output"),
        "an untrusted canned output is nonce-wrapped byte for byte"
    );
    assert!(!outcome.trusted(), "the untrusted marking survives");
    assert_eq!(
        counts.get("echo").expect("the counts read"),
        Some(1),
        "preparing the outcome increments the alias count"
    );
    assert_eq!(
        *recorder
            .observations
            .lock()
            .expect("the recorder mutex must not be poisoned"),
        vec![Observation::ToolCallSucceeded],
        "a canned Ok output reports the succeeded observation"
    );
    assert_eq!(
        *recorder
            .tool_results
            .lock()
            .expect("the recorder mutex must not be poisoned"),
        vec![(
            7,
            2,
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
    let failing = binding("failing", Arc::new(FailingTool));
    let error = prepare_dispatch(
        &failing,
        Err(ToolError::message("canned failure").with_kind(ToolErrorKind::Backend)),
        None,
        &GuardNonce::fresh(),
        &recorder,
        EXECUTION,
        SECTION,
        None,
    )
    .expect_err("a canned failure fails the preparation");
    assert!(
        matches!(error, Error::Tool { .. }),
        "the canned failure is the typed tool error, got {error:?}"
    );
    assert_eq!(
        *recorder
            .observations
            .lock()
            .expect("the recorder mutex must not be poisoned"),
        vec![Observation::ToolCallFailed],
        "a canned Err output reports the failed observation"
    );
}
