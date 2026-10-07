//! Tests for the scheduler's `tool_call` arm: the model-issued form
//! (`call_id: Some`), which always resumes with content and reports its
//! `ToolResult` under the model's call id; the script form (`call_id:
//! None`), which keeps the raise-at-call-site behavior; local Lua tools,
//! whose handlers run inside the calling block coroutine - store calls,
//! nested local calls, and bound calls included - with no leaf work of
//! their own; and the reserved task names, refused before alias lookup.
//! The fixtures reach the
//! model-issued form through the test-only `tools.call_as_model` install
//! (`expose_raw_shims_for_test`); in production only the loop shim yields
//! a `call_id`.
//! The local Lua tool cases sit in `local_handlers`.

use super::models_loop::loop_models;
use super::*;
use crate::lua::ToolSet;
use crate::test_support::tokio_driver::TokioDriver;

/// Records every observation and every `on_tool_result` report as one
/// rendered line, so a test reads the arm's whole reporting sequence.
#[derive(Default)]
pub(super) struct ToolRecorder(Mutex<Vec<String>>);

impl ToolRecorder {
    fn push(&self, line: String) {
        self.0
            .lock()
            .expect("the tool recorder mutex is not poisoned")
            .push(line);
    }

    pub(super) fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the tool recorder mutex is not poisoned")
            .clone()
    }
}

impl Observer for ToolRecorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.push(format!("{section}: {event}"));
    }

    fn on_tool_result(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        _turn: u32,
        tool_call_id: &str,
        alias: &str,
        content: &str,
        trusted: bool,
    ) {
        self.push(format!(
            "{section}: tool_result id={tool_call_id} alias={alias} trusted={trusted} content={content}"
        ));
    }
}

/// The run context for a tool-call arm test: the parsed prompt, the shared
/// model and tool sets pre-filled (the scheduler tests bypass the live H1
/// pass that would fill them), the given observer, and the raw protocol
/// shims exposed so a fixture can yield a model-issued call.
fn tool_context(
    prompt: &Prompt,
    tools: impl Into<FixtureTools>,
    observer: Arc<dyn Observer>,
) -> (RunState, RunFixture) {
    let mut ctx = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &TestStore::new().vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &test_context(EXECUTION),
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = loop_models();
    let fixture = tools
        .into()
        .install(&ctx, RunFixture::new().observer(observer));
    ctx.expose_raw_shims_for_test();
    (ctx, fixture)
}

/// The tool set with the always-failing fixture bound as `fail` and in
/// scope.
fn failing_tools() -> FixtureTools {
    FixtureTools::new(
        vec![fixture_binding(
            "fail",
            "failing capability",
            Arc::new(FailingTool),
        )],
        vec!["fail".to_owned()],
    )
}

/// The one-section prompt shell every arm test drives.
fn arm_prompt(lua: &str) -> String {
    format!(
        "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# ToolCall\n\n## Only\n\n```lua\n{lua}\n```\n"
    )
}

#[tokio::test(flavor = "current_thread")]
async fn a_failing_bound_tool_with_a_call_id_resumes_with_untrusted_failure_text() {
    let md = arm_prompt(
        "local out = tools.call_as_model('call_1', 'fail', {})\n\
         assert(type(out) == 'string', 'a model-issued call always resumes with content')\n\
         return out",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, fixture) = tool_context(
        &prompt,
        failing_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("a model-issued call never raises for the tool's own failure");
    assert!(
        out.contains("<untrusted_input_") && out.contains("</untrusted_input_"),
        "the failure text is nonce-wrapped as untrusted, got: {out}"
    );
    assert!(
        out.contains("the tool's own backend failed"),
        "the wrapped text includes the tool's message, got: {out}"
    );
    let lines = recorder.lines();
    assert!(
        lines
            .iter()
            .any(|line| line == &format!("Only: {}", detail::TOOL_CALL_FAILED)),
        "the tool's failure is observed: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Only: tool_result id=call_1 alias=fail trusted=false")),
        "ToolResult fires under the model's call id as untrusted: {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_same_failing_tool_without_a_call_id_raises_kind_tool() {
    let md = arm_prompt(
        "local ok, err = pcall(tools.call, 'fail', {})\n\
         assert(not ok, 'a script call raises the tool failure at the call site')\n\
         return err.kind .. '|' .. tostring(err)",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, fixture) = tool_context(
        &prompt,
        failing_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    assert!(
        out.starts_with("tool|"),
        "a script call's tool failure reads as kind `tool`, got: {out}"
    );
    assert!(
        out.contains("the tool's own backend failed"),
        "the raised message is the tool's own, got: {out}"
    );
    let lines = recorder.lines();
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "a failed script call reports no ToolResult: {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_bound_alias_with_no_implementation_in_the_test_driver_table_resumes_as_a_tool_error() {
    // The binding names an identity the Engine advertises and journals,
    // but the test driver's tool table holds nothing under it: the performer
    // answers the effect with the error instead of a call, and a script
    // call raises it at the call site.
    let md = arm_prompt(
        "local ok, err = pcall(tools.call, 'echo', { value = 'hi' })\n\
         assert(not ok, 'an unresolvable identity raises at the call site')\n\
         return err.kind .. '|' .. tostring(err)",
    );
    let prompt = parse(&md);
    let (binding, _unregistered) = fixture_binding("echo", "echo capability", Arc::new(EchoTool));
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, fixture) = tool_context(
        &prompt,
        ToolSet::for_test(vec![binding], vec!["echo".to_owned()], Vec::new()),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the missing implementation is pcall-able");
    assert!(
        out.starts_with("tool|"),
        "the missing implementation reads as kind `tool`, got: {out}"
    );
    assert!(
        out.contains("no implementation in the test driver's tool table"),
        "the raised message names the test driver's tool table, got: {out}"
    );
    let lines = recorder.lines();
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "nothing was called, so no ToolResult is reported: {lines:?}"
    );
}

/// The `tool_result` lines `recorder` saw, in order.
pub(super) fn tool_result_lines(recorder: &ToolRecorder) -> Vec<String> {
    recorder
        .lines()
        .into_iter()
        .filter(|line| line.contains("tool_result"))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_reserved_task_name_answers_unbound_tool_before_alias_lookup() {
    // `task_status` is registered as a local tool so the test proves the
    // reservation wins over a lookup that would otherwise succeed.
    let md = arm_prompt(
        "tools.add_local('task_status', 'shadow', {}, function() return 'shadowed' end)\n\
         local kinds = {}\n\
         for _, name in ipairs({ 'task', 'task_cancel', 'task_status', 'await_tasks' }) do\n\
           local ok, err = pcall(tools.call, name, {})\n\
           assert(not ok, name .. ' must be refused')\n\
           kinds[#kinds + 1] = err.kind .. ':' .. err.name\n\
         end\n\
         return table.concat(kinds, ',')",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, fixture) = tool_context(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("each refusal is pcall-able");
    assert_eq!(
        out,
        "unbound_tool:task,unbound_tool:task_cancel,unbound_tool:task_status,\
         unbound_tool:await_tasks"
    );
    let lines = recorder.lines();
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "a reserved name dispatches nothing: {lines:?}"
    );
}

#[path = "tool_call_arm-local-handlers.rs"]
mod local_handlers;
