//! Unit tests for section execution, tool scoping, and the tool-call loop.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use serde_json::{Value, json};

use super::context::RunState;
use super::scope::prepare_scoped_tools;
use super::support::advance_turn;
use super::*;
use crate::lua::{LuaProgram, SectionVm, current_tool_bindings};
use crate::model::{ModelDescriptor, ModelId, ModelSet, ThinkingMode};
use crate::parser::ParseErrorKind;
use crate::parser::Prompt;
use crate::store::{Access, StoreError, VfsRef};
use crate::test_support::mock_gateway_client::MockGatewayClient;
use crate::test_support::recording::DebugCapture;
use crate::test_support::recording::{NullObserver, Observation, Observer, detail, null_emitter};
use crate::test_support::tokio_driver::TokioDriver;
use crate::test_support::{RunHost, TestTool, TestToolTable};
use crate::tools::{ToolError, ToolErrorKind, ToolId, ToolOutput};
use crate::untrusted::GuardNonce;
use crate::{Error, Result};
use promptforge_lua::ToolOutputKind;
use promptforge_model_client::model::ModelCatalog;
use promptforge_store::StoreExt;

mod context;
mod fixtures;
mod gateway;

// The seam modules are glob-imported here (not re-exported) so every suite's
// existing `use super::*` keeps resolving the shared helpers.
use self::context::*;
use self::fixtures::*;
use self::gateway::*;

// --- Schema description overrides (ported from the deleted tool_bag.rs) ---
//
// `ToolBag::prepare` wrapped exactly this construction -
// `current_tool_bindings` plus `prepare_scoped_tools` - so the schema-level
// override coverage ports onto the prose path's scope building directly. The
// bag's generation cache is deleted with the bag, so the cache test has no
// behavior left to port; per-block scope rebuilds stay covered by the
// tool-scoping and fanout-arm suites.

/// The catalog text is advertised when no override exists at any layer, and a
/// `tools.add` override reaches the advertised schema.
#[test]
fn tool_description_override_appears_in_model_schema() {
    let echo: Arc<dyn TestTool> = Arc::new(EchoTool);
    // In production the binding's description is the descriptor's, copied
    // at fill time; the test's slot text stands in for it here.
    let (binding, _) = fixture_binding("echo", "echo capability for live matching", echo);
    let bindings = crate::lua::ToolSet::for_test(vec![binding], Vec::new());
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::from_seed(0x7e57),
        &Arc::new(Mutex::new(bindings)),
        &Arc::new(Mutex::new(ModelSet::default())),
        &null_emitter(),
        "Override",
    )
    .expect("captured bindings must install");
    vm.install_captured_bindings()
        .expect("alias globals must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");

    // tools.add(alias) with no override keeps the bound tool's catalog text.
    let add_default = LuaProgram::compile(
        "tools.add(echo)",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Override",
    )
    .expect("prologue must compile");
    vm.run_chunk(&add_default, &null_emitter(), "Override")
        .expect("tools.add(echo) without override must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(
        schemas[0].description, "echo capability for live matching",
        "no override anywhere must advertise the bound tool's description"
    );

    // tools.add(alias, override) overrides the model-facing schema.
    let add_override = LuaProgram::compile(
        "tools.add('echo', 'Author override for the model')",
        "prologue-2",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Override",
    )
    .expect("second prologue must compile");
    vm.run_chunk(&add_override, &null_emitter(), "Override")
        .expect("description override at tools.add must succeed");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(schemas[0].description, "Author override for the model");

    vm.teardown(&null_emitter(), "Override");
}

/// Precedence at the advertised schema: a `tools.add` override beats the
/// `model_description` recorded by `tools.bind` / `tools.always`, which itself
/// beats the catalog text.
#[test]
fn bind_override_reaches_the_schema_and_add_beats_bind() {
    let bindings = crate::lua::ToolSet::for_test(
        vec![crate::lua::ToolBinding {
            alias: "echo".to_owned(),
            description: "echo capability for live matching".to_owned(),
            id: ToolId::parse("tests/tools/echo").expect("valid id"),
            model_description: Some("bind override".to_owned()),
            schema: EchoTool.parameters_schema(),
            output_kind: ToolOutputKind::Plain,
            conflicts: Vec::new(),
        }],
        Vec::new(),
    );
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::from_seed(0x7e57),
        &Arc::new(Mutex::new(bindings)),
        &Arc::new(Mutex::new(ModelSet::default())),
        &null_emitter(),
        "Precedence",
    )
    .expect("captured bindings must install");
    vm.install_captured_bindings()
        .expect("alias globals must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");

    let add_plain = LuaProgram::compile(
        "tools.add('echo')",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Precedence",
    )
    .expect("prologue must compile");
    vm.run_chunk(&add_plain, &null_emitter(), "Precedence")
        .expect("tools.add without override must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(
        schemas[0].description, "bind override",
        "the bind/always override must beat the catalog text"
    );

    let add_override = LuaProgram::compile(
        "tools.add('echo', 'add override')",
        "prologue-2",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Precedence",
    )
    .expect("second prologue must compile");
    vm.run_chunk(&add_override, &null_emitter(), "Precedence")
        .expect("tools.add with override must succeed");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(
        schemas[0].description, "add override",
        "the add override must beat the bind/always override"
    );

    vm.teardown(&null_emitter(), "Precedence");
}

/// A tool whose call never completes, so the test can prove the tool-call
/// loop honors cancellation mid-call rather than waiting the call out.
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
    fn wire_name(&self) -> &str {
        // Matches the function name the mock gateway asks for.
        "echo"
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
    let error = run_with_context(&fixture(md), |ctx, host| (ctx.cancel(handle), host))
        .await
        .expect_err("a pre-cancelled handle must fail the run");
    assert!(
        matches!(error.kind(), RunErrorKind::Cancelled),
        "expected RunErrorKind::Cancelled, got {error:?}"
    );
    assert!(error.is_cancelled());
}

// --- Guard-wrapping of untrusted tool results in the loop ---

/// The content of the first `tool`-role message in the last recorded body.
///
/// The second request the loop sends includes the dispatched tool's result;
/// this pulls that result string back out so a test can assert on it.
fn last_tool_turn_content(bodies: &[Value]) -> String {
    let last = bodies.last().expect("the loop must send a second request");
    last["messages"]
        .as_array()
        .expect("a request body must include a messages array")
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("the re-sent conversation must include the tool turn")["content"]
        .as_str()
        .expect("a tool turn's content must be a string")
        .to_string()
}

/// Extracts the guard-tag nonce from every `tool`-role turn in the last body.
fn tool_turn_nonces(bodies: &[Value]) -> Vec<String> {
    let last = bodies.last().expect("the loop must send a final request");
    last["messages"]
        .as_array()
        .expect("a request body must include a messages array")
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
    // different host-drawn seeds wrap the same untrusted tool result under
    // different nonces, so an envelope's tag stays unguessable from one run
    // to the next as long as the host draws each seed afresh. (Under one
    // seed the two runs agree byte for byte, which `run_inputs` pins.)
    let md = "---\nname: t\ndescription: d\npromptforge: 0\ncapabilities:\n  - tests/tools\ntools:\n  echo: tests/tools/untrusted_echo\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('echo', { value = 'hi' })\n```\n";
    let test = bound_with_tools(md);
    let mut run_nonces = Vec::new();
    for seed in [1, 2] {
        let (catalog, table) = fixture_tools(&[Arc::new(UntrustedEchoTool) as Arc<dyn TestTool>]);
        let env = Environment::new().tools(catalog);
        let mut ctx = RunContext::new(EXECUTION, seed, TEST_STARTED_AT);
        let host = RunHost::new().tools(table);
        if let Some(model) = test.models.models().first() {
            ctx = ctx.model(model.clone());
        }
        let out = match crate::test_support::run_with_host(&env, &test.prompt, "", ctx, host).await
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

mod chat_arm;
mod chat_scope;
mod debug_and_counts;
mod effects;
mod exec_flow;
mod exit_rules;
mod fanout_acceptance;
mod input;
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
mod observations;
mod provenance;
mod run_inputs;
mod run_termination;
mod scheduler;
mod serial_driver;
mod task_events;
mod tasks;
mod timeouts;
mod tool_call_arm;
mod tool_loop;
mod tool_scoping;
mod unified_pipeline;
mod waits;
