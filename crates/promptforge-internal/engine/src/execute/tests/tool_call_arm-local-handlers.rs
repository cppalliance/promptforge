//! Local Lua tools: a handler runs inside the calling block coroutine
//! with no leaf work of its own and reports its trusted result under
//! the model's call id or none; it uses the store, raises to a script
//! caller that catches it, calls another local tool innermost first,
//! and dispatches a bound tool as one leaf request, whose failure
//! raises kind `tool` in the handler and whose untrusted wrapper
//! survives.

use super::super::models_loop::{always_tool, echo_tools};
use super::*;

/// The `grab` local tool registration every local-tool test opens with.
const ADD_LOCAL_GRAB: &str = "tools.add_local('grab', 'Grab a value', { value = 'string' }, \
     function(args) return 'got ' .. args.value end)\n";

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
