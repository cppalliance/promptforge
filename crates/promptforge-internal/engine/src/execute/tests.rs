//! Unit tests for section execution, tool scoping, and the tool-call loop.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::context::RunState;
use super::scope::prepare_scoped_tools;
use super::support::advance_turn;
use super::*;
use crate::lua::{LuaProgram, SectionVm, current_tool_bindings};
use crate::model::{ModelDescriptor, ModelId, ModelSet, ThinkingMode};
use crate::parser::ParseErrorKind;
use crate::parser::Prompt;
use crate::test_support::recording::DebugCapture;
use crate::test_support::recording::{NullObserver, Observation, Observer, detail, null_emitter};
use crate::test_support::scripted_chat::{ScriptedCall, ScriptedChat, ScriptedReply};
use crate::test_support::tokio_driver::TokioDriver;
use crate::test_support::{RunFixture, TestTool, TestToolTable};
use crate::tools::{ToolError, ToolErrorKind, ToolId, ToolOutput};
use crate::untrusted::GuardNonce;
use crate::{Error, Result};
use promptforge_model_client::detail::{tool_schema_description, tool_schema_name};
use promptforge_model_client::model::ModelCatalog;
use promptforge_vfs::{Access, VfsError, VfsRef};

mod context;
mod fixtures;
mod gateway;

// The seam modules are glob-imported here (not re-exported) so every suite's
// existing `use super::*` keeps resolving the shared helpers.
use self::context::*;
use self::fixtures::*;
use self::gateway::*;

/// A tool whose call never completes, so the test can prove the tool-call
/// loop honors cancellation mid-call rather than waiting the call out, and
/// a task parked on it stays live until its owner ends or cancels it.
struct SlowTool;

#[async_trait::async_trait]
impl TestTool for SlowTool {
    fn id(&self) -> ToolId {
        ToolId::parse("test/tools/slow").expect("valid slow tool id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "a deliberately slow tool"
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn call(&self, _args: Value) -> std::result::Result<ToolOutput, ToolError> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn run_with_a_pre_cancelled_handle_fails_as_cancelled() {
    use crate::cancel::CancelHandle;

    // The explicit-cancel wiring of the public entry point: a handle passed
    // through `RunContext::cancel` is installed around the whole run body, so
    // the section's Lua instruction hook observes it and the run maps the
    // interruption to `RunErrorKind::Cancelled`.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Loop\n\n```lua\nlocal n = 0\nwhile true do n = n + 1 end\n```\n";
    let handle = CancelHandle::new();
    handle.cancel();
    let error = run_with_context(&fixture(md), |ctx, fixture| (ctx.cancel(handle), fixture))
        .await
        .expect_err("a pre-cancelled handle must fail the run");
    assert!(
        matches!(error.kind(), RunErrorKind::Cancelled),
        "expected RunErrorKind::Cancelled, got {error:?}"
    );
    assert!(error.is_cancelled());
}

// --- Guard-wrapping of untrusted tool results in the loop ---

/// The content of the first `tool`-role message in the last recorded round.
///
/// The second round the loop sends includes the dispatched tool's result;
/// this pulls that result string back out so a test can assert on it.
fn last_tool_turn_content(bodies: &[ScriptedCall]) -> String {
    let last = bodies.last().expect("the loop must send a second request");
    last.messages_json()
        .as_array()
        .expect("a round's messages serialize to an array")
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("the re-sent conversation must include the tool turn")["content"]
        .as_str()
        .expect("a tool turn's content must be a string")
        .to_string()
}

/// Extracts the guard-tag nonce from every `tool`-role turn in the last
/// recorded round.
fn tool_turn_nonces(bodies: &[ScriptedCall]) -> Vec<String> {
    let last = bodies.last().expect("the loop must send a final request");
    last.messages_json()
        .as_array()
        .expect("a round's messages serialize to an array")
        .iter()
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["content"].as_str())
        .filter_map(|content| {
            let marker = "<untrusted_input_";
            let start = content.find(marker)? + marker.len();
            let rest = &content[start..];
            let end = rest.find('>')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

#[tokio::test]
async fn untrusted_nonce_differs_across_runs_under_different_seeds() {
    // The nonce is the run seed's: two runs of the same prompt under
    // different caller-drawn seeds wrap the same untrusted tool result under
    // different nonces, so an envelope's tag stays unguessable from one run
    // to the next as long as the caller draws each seed afresh. (Under one
    // seed the two runs agree byte for byte, which `run_inputs` pins.)
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tools\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua\nrequire '## setup'\n```\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('tools/untrusted_echo', { value = 'hi' })\n```\n\n\
        ## setup\n\n```lua\nmodels.default('writer')\n```\n";
    let test = bound_with_tools(md);
    let mut run_nonces = Vec::new();
    for seed in [1, 2] {
        let (catalog, table) = fixture_tools(&[Arc::new(UntrustedEchoTool) as Arc<dyn TestTool>]);
        let env = Environment::new().tools(catalog);
        let mut ctx = RunContext::new(EXECUTION, seed, TEST_STARTED_AT);
        let fixture = RunFixture::new().tools(table);
        if let Some(model) = test.models.models().first() {
            ctx = ctx.model(model.clone());
        }
        let out = match crate::test_support::run_with_fixture(&env, &test.prompt, "", ctx, fixture)
            .await
        {
            RunResult::Ok(out) => out,
            other => panic!("the echo run succeeds: {other:?}"),
        };
        let marker = "<untrusted_input_";
        let start = out.find(marker).expect("the result is guard-wrapped") + marker.len();
        let end = out[start..]
            .find('>')
            .map(|end| start + end)
            .expect("the guard tag closes");
        run_nonces.push(out[start..end].to_string());
    }
    assert_ne!(
        run_nonces[0], run_nonces[1],
        "each run must mint its own nonce"
    );
}

// --- Progress reporting ---

/// The two-section fixture the fall-through test uses: the first section
/// falls through, the second returns from Lua.
const TWO_SECTIONS: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn \"second\"\n```\n";

const STORE_SECTIONS: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## First\n\n```lua\nstore.write('state.txt', 'first')\n```\n\n\
## Second\n\n```lua\nstore.append('state.txt', '\\nsecond')\nreturn \"second\"\n```\n";

/// Records every [`DebugEvent`] so tests can assert capture wiring.
#[derive(Default)]
struct RecordingCapture(
    Mutex<
        Vec<(
            String,
            String,
            u32,
            crate::test_support::recording::DebugEvent,
        )>,
    >,
);

impl crate::test_support::recording::DebugCapture for RecordingCapture {
    fn on_event(
        &self,
        execution: &str,
        section: &str,
        turn_index: u32,
        event: crate::test_support::recording::DebugEvent,
    ) {
        self.0
            .lock()
            .expect("the capture mutex must not be poisoned")
            .push((execution.to_owned(), section.to_owned(), turn_index, event));
    }
}

impl RecordingCapture {
    fn events(
        &self,
    ) -> Vec<(
        String,
        String,
        u32,
        crate::test_support::recording::DebugEvent,
    )> {
        self.0
            .lock()
            .expect("the capture mutex must not be poisoned")
            .clone()
    }
}

mod batch_turn;
mod chat_arm;
mod chat_record_rebuild;
mod chat_scope;
mod debug_and_counts;
mod effects;
mod exec_flow;
mod exit_rules;
mod fanout_acceptance;
mod full_id_calls;
mod happens_before;
mod live_infer;
mod local_tools;
mod model_and_reply;
mod model_task_acceptance;
mod model_task_answers;
mod model_task_awaits;
mod model_task_ids_and_scope;
mod model_task_notices;
mod model_task_trust;
mod model_tasks;
mod models_loop;
mod models_loop_compactors;
mod models_loop_contract;
mod observations;
mod offering;
mod parallel_tool_calls;
mod precheck_anchor;
mod preludes;
mod provenance;
mod rounds;
mod run_inputs;
mod run_termination;
mod scheduler;
mod serial_driver;
mod suite;
mod tasks;
mod timeouts;
mod tool_call_access;
mod tool_call_arm;
mod tool_loop;
mod tool_scoping;
mod unified_pipeline;
mod waits;
