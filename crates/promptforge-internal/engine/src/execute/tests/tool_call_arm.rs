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

use super::models_loop::{always_tool, echo_tools, loop_models};
use super::*;
use crate::lua::ToolSet;
use crate::test_support::tokio_driver::TokioDriver;

/// Records every observation and every `on_tool_result` report as one
/// rendered line, so a test reads the arm's whole reporting sequence.
#[derive(Default)]
struct ToolRecorder(Mutex<Vec<String>>);

impl ToolRecorder {
    fn push(&self, line: String) {
        self.0
            .lock()
            .expect("the tool recorder mutex is not poisoned")
            .push(line);
    }

    fn lines(&self) -> Vec<String> {
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
) -> (RunState, RunHost) {
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
    let host = tools
        .into()
        .install(&ctx, RunHost::new().observer(observer));
    ctx.expose_raw_shims_for_test();
    (ctx, host)
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

/// The `grab` local tool registration every local-tool test opens with.
const ADD_LOCAL_GRAB: &str = "tools.add_local('grab', 'Grab a value', { value = 'string' }, \
     function(args) return 'got ' .. args.value end)\n";

#[tokio::test(flavor = "current_thread")]
async fn a_failing_bound_tool_with_a_call_id_resumes_with_untrusted_failure_text() {
    let md = arm_prompt(
        "local out = tools.call_as_model('call_1', 'fail', {})\n\
         assert(type(out) == 'string', 'a model-issued call always resumes with content')\n\
         return out",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        failing_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
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
    let (ctx, host) = tool_context(
        &prompt,
        failing_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
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
async fn a_bound_alias_with_no_implementation_in_the_host_table_resumes_as_a_tool_error() {
    // The binding names an identity the engine advertises and journals,
    // but the host's table holds nothing under it: the performer answers
    // the effect with the error instead of a call, and a script call
    // raises it at the call site.
    let md = arm_prompt(
        "local ok, err = pcall(tools.call, 'echo', { value = 'hi' })\n\
         assert(not ok, 'an unresolvable identity raises at the call site')\n\
         return err.kind .. '|' .. tostring(err)",
    );
    let prompt = parse(&md);
    let (binding, _unregistered) = fixture_binding("echo", "echo capability", Arc::new(EchoTool));
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        ToolSet::for_test(vec![binding], vec!["echo".to_owned()]),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the missing implementation is pcall-able");
    assert!(
        out.starts_with("tool|"),
        "the missing implementation reads as kind `tool`, got: {out}"
    );
    assert!(
        out.contains("no implementation in the host's table"),
        "the raised message names the host table, got: {out}"
    );
    let lines = recorder.lines();
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "nothing was called, so no ToolResult is reported: {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_local_lua_tool_call_issues_no_leaf_work() {
    let md = arm_prompt(&format!(
        "{ADD_LOCAL_GRAB}\
         local out = tools.call('grab', {{ value = 'hi' }})\n\
         return out .. '|' .. tostring(tools.calls.grab)"
    ));
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, host, None);
    let out = scheduler
        .drive()
        .await
        .expect("a local tool answers on the parked chain's VM");
    assert_eq!(
        out, "got hi|1",
        "the handler's text resumes and the call counts"
    );
    assert_eq!(
        scheduler.leaf_requests_issued(),
        0,
        "a local tool call spawns no leaf request"
    );
    let lines = recorder.lines();
    assert!(
        lines
            .iter()
            .any(|line| line == &format!("Only: {}", detail::TOOL_CALL_SUCCEEDED)),
        "the local call is observed as a succeeded tool call: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "Only: tool_result id= alias=grab trusted=true content=got hi"),
        "a script-initiated local call reports its trusted result under no call id: {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_model_issued_local_tool_call_reports_under_its_call_id() {
    let md = arm_prompt(&format!(
        "{ADD_LOCAL_GRAB}\
         return tools.call_as_model('call_7', 'grab', {{ value = 'hi' }})"
    ));
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, host, None);
    let out = scheduler
        .drive()
        .await
        .expect("a model-issued local call answers inline");
    assert_eq!(out, "got hi");
    assert_eq!(
        scheduler.leaf_requests_issued(),
        0,
        "a model-issued local call spawns no leaf request"
    );
    let lines = recorder.lines();
    assert!(
        lines.iter().any(
            |line| line == "Only: tool_result id=call_7 alias=grab trusted=true content=got hi"
        ),
        "ToolResult fires under the model's call id: {lines:?}"
    );
}

/// The `tool_result` lines `recorder` saw, in order.
fn tool_result_lines(recorder: &ToolRecorder) -> Vec<String> {
    recorder
        .lines()
        .into_iter()
        .filter(|line| line.contains("tool_result"))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_called_local_handler_uses_the_store() {
    let md = arm_prompt(
        "tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)\n\
           store.write('grab.txt', 'kept ' .. args.value)\n\
           return store.read('grab.txt')\n\
         end)\n\
         return tools.call('grab', { value = 'hi' })",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the handler's store calls suspend and resume the block");
    assert_eq!(out, "kept hi");
    assert_eq!(
        tool_result_lines(&recorder),
        vec!["Only: tool_result id= alias=grab trusted=true content=kept hi".to_owned()]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_caller_catches_the_handlers_own_error_and_jump_is_restored() {
    let md = arm_prompt(
        "local raised = { kind = 'custom', message = 'handler exploded' }\n\
         tools.add_local('grab', 'Grab a value', {}, function() error(raised) end)\n\
         local ok, err = pcall(tools.call, 'grab', {})\n\
         assert(not ok, 'the handler raise reaches the call site')\n\
         assert(err == raised and err.kind == 'custom', 'the caller catches the handler table')\n\
         jump('## Other')",
    ) + "\n## Other\n\n```lua\nreturn 'jumped'\n```\n";
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the handler's raise is pcall-able at the call site");
    assert_eq!(
        out, "jumped",
        "after the caught failure the block's `jump` transfers"
    );
    let lines = recorder.lines();
    assert!(
        lines
            .iter()
            .any(|line| line == &format!("Only: {}", detail::TOOL_CALL_FAILED)),
        "the raising handler is observed as a failed tool call: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "a raising handler reports no ToolResult: {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_nested_local_call_reports_both_results_innermost_first() {
    let md = arm_prompt(
        "tools.add_local('inner', 'Inner', { value = 'string' }, function(args)\n\
           return 'inner ' .. args.value\n\
         end)\n\
         tools.add_local('outer', 'Outer', { value = 'string' }, function(args)\n\
           return 'outer ' .. tools.call('inner', { value = args.value })\n\
         end)\n\
         return tools.call_as_model('call_1', 'outer', { value = 'hi' })",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a handler may call another local tool");
    assert_eq!(out, "outer inner hi");
    assert_eq!(
        tool_result_lines(&recorder),
        vec![
            "Only: tool_result id= alias=inner trusted=true content=inner hi".to_owned(),
            "Only: tool_result id=call_1 alias=outer trusted=true content=outer inner hi"
                .to_owned(),
        ],
        "each call reports under its own id, the inner call closing first"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_handler_dispatches_a_bound_tool_as_one_leaf_request() {
    let md = arm_prompt(
        "tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)\n\
           return tools.call('echo', { value = args.value })\n\
         end)\n\
         local out = tools.call('grab', { value = 'hi' })\n\
         return out .. '|' .. tostring(tools.calls.grab) .. '|' .. tostring(tools.calls.echo)",
    );
    let prompt = parse(&md);
    let (ctx, host) = tool_context(
        &prompt,
        echo_tools(),
        Arc::new(NullObserver::default()) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, host, None);
    let out = scheduler
        .drive()
        .await
        .expect("a handler's bound call resumes inside the handler");
    assert_eq!(out, "echoed: hi|1|1", "both calls count once");
    assert_eq!(
        scheduler.leaf_requests_issued(),
        1,
        "the echo call is the one leaf request; the local call issues none"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_bound_failure_inside_a_model_issued_local_call_raises_kind_tool_in_the_handler() {
    let md = arm_prompt(
        "tools.add_local('grab', 'Grab a value', {}, function()\n\
           local ok, err = pcall(tools.call, 'fail', {})\n\
           assert(not ok, 'the bound failure raises inside the handler')\n\
           return err.kind\n\
         end)\n\
         return tools.call_as_model('call_1', 'grab', {})",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        failing_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the handler catches its own bound call's failure");
    assert_eq!(
        out, "tool",
        "the handler's call is a script call, raising kind `tool`"
    );
    assert_eq!(
        tool_result_lines(&recorder),
        vec!["Only: tool_result id=call_1 alias=grab trusted=true content=tool".to_owned()],
        "the inner failure fires no ToolResult; only the local call reports under call_1"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_handlers_text_stays_trusted_and_a_bound_tools_wrapper_survives() {
    let md = arm_prompt(
        "tools.add_local('plain', 'Plain', {}, function() return 'plain text' end)\n\
         tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)\n\
           return tools.call('echo', { value = args.value })\n\
         end)\n\
         return tools.call('plain', {}) .. '||' .. tools.call('grab', { value = 'hi' })",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        always_tool("echo", Arc::new(UntrustedEchoTool)),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("both local calls answer");
    let (plain, wrapped) = out.split_once("||").expect("the block joins both results");
    assert_eq!(plain, "plain text", "a handler's own text gets no wrapper");
    assert!(
        wrapped.contains("<untrusted_input_")
            && wrapped.contains("</untrusted_input_")
            && wrapped.contains("echoed: hi"),
        "the bound tool's untrusted output keeps its wrapper through the handler: {wrapped}"
    );
    let lines = tool_result_lines(&recorder);
    assert!(
        lines.contains(
            &"Only: tool_result id= alias=plain trusted=true content=plain text".to_owned()
        ),
        "the plain handler reports trusted: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.starts_with("Only: tool_result id= alias=grab trusted=true content=")
                && line.contains("<untrusted_input_")
        }),
        "the wrapping handler reports trusted, its text still wrapped: {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_reserved_task_name_answers_unbound_tool_before_alias_lookup() {
    // `task_status` is registered as a local tool so the test proves the
    // reservation wins over a lookup that would otherwise succeed.
    let md = arm_prompt(
        "tools.add_local('task_status', 'shadow', {}, function() return 'shadowed' end)\n\
         local kinds = {}\n\
         for _, name in ipairs({ 'task', 'task_cancel', 'task_status', 'task_events', 'await_tasks' }) do\n\
           local ok, err = pcall(tools.call, name, {})\n\
           assert(not ok, name .. ' must be refused')\n\
           kinds[#kinds + 1] = err.kind .. ':' .. err.name\n\
         end\n\
         return table.concat(kinds, ',')",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(ToolRecorder::default());
    let (ctx, host) = tool_context(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("each refusal is pcall-able");
    assert_eq!(
        out,
        "unbound_tool:task,unbound_tool:task_cancel,unbound_tool:task_status,\
         unbound_tool:task_events,unbound_tool:await_tasks"
    );
    let lines = recorder.lines();
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "a reserved name dispatches nothing: {lines:?}"
    );
}
