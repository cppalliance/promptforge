//! Local Lua tools: a handler runs inside the calling block coroutine
//! with no leaf work of its own and reports its trusted result under
//! the model's call id or none; it uses the store, raises to a script
//! caller that catches it, calls another local tool innermost first,
//! and dispatches a bound tool as one leaf request, whose failure
//! raises kind `tool` in the handler and whose untrusted wrapper
//! survives.

use promptforge_parser::test_support::prompt;

use super::*;

/// The `grab` local tool registration every local-tool test opens with.
const ADD_LOCAL_GRAB: &str = "tools.offer_local('grab', 'Grab a value', { value = 'string' }, \
     function(args) return 'got ' .. args.value end)\n";

#[test]
fn a_local_lua_tool_call_issues_no_leaf_work() {
    let source = one_section(&format!(
        "{ADD_LOCAL_GRAB}\
         local out = tools.call('grab', {{ value = 'hi' }})\n\
         return out .. '|' .. tostring(tools.calls.grab)"
    ));
    let trace = arm_scenario(source).run();
    assert_eq!(
        trace.text(),
        "got hi|1",
        "the handler's text resumes and the call counts"
    );
    assert_eq!(
        trace.effects().len(),
        0,
        "a local tool call spawns no leaf request"
    );
    let lines = tool_lines(&trace).lines();
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

#[test]
fn a_model_issued_local_tool_call_reports_under_its_call_id() {
    let source = one_section(&format!(
        "{ADD_LOCAL_GRAB}\
         return tools.call_as_model('call_7', 'grab', {{ value = 'hi' }})"
    ));
    let trace = arm_scenario(source).run();
    assert_eq!(trace.text(), "got hi");
    assert_eq!(
        trace.effects().len(),
        0,
        "a model-issued local call spawns no leaf request"
    );
    let lines = tool_lines(&trace).lines();
    assert!(
        lines.iter().any(
            |line| line == "Only: tool_result id=call_7 alias=grab trusted=true content=got hi"
        ),
        "ToolResult fires under the model's call id: {lines:?}"
    );
}

#[test]
fn a_script_called_local_handler_uses_the_store() {
    let source = one_section(
        "tools.offer_local('grab', 'Grab a value', { value = 'string' }, function(args)
           store.write('grab.txt', 'kept ' .. args.value)
           return store.read('grab.txt')
         end)
         return tools.call('grab', { value = 'hi' })",
    );
    let trace = arm_scenario(source).run();
    assert_eq!(trace.text(), "kept hi");
    assert_eq!(
        tool_result_lines(&tool_lines(&trace)),
        vec!["Only: tool_result id= alias=grab trusted=true content=kept hi".to_owned()]
    );
}

#[test]
fn a_script_caller_catches_the_handlers_own_error_and_jump_is_restored() {
    let only = "local raised = { kind = 'custom', message = 'handler exploded' }
                tools.offer_local('grab', 'Grab a value', {}, function() error(raised) end)
                local ok, err = pcall(tools.call, 'grab', {})
                assert(not ok, 'the handler raise reaches the call site')
                assert(err == raised and err.kind == 'custom', 'the caller catches the handler table')
                jump('## Other')";
    let source = prompt("", &[("Only", only), ("Other", "return 'jumped'")]);
    let trace = arm_scenario(source).run();
    assert_eq!(
        trace.text(),
        "jumped",
        "after the caught failure the block's `jump` transfers"
    );
    let lines = tool_lines(&trace).lines();
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

#[test]
fn a_nested_local_call_reports_both_results_innermost_first() {
    let source = one_section(
        "tools.offer_local('inner', 'Inner', { value = 'string' }, function(args)
           return 'inner ' .. args.value
         end)
         tools.offer_local('outer', 'Outer', { value = 'string' }, function(args)
           return 'outer ' .. tools.call('inner', { value = args.value })
         end)
         return tools.call_as_model('call_1', 'outer', { value = 'hi' })",
    );
    let trace = arm_scenario(source).run();
    assert_eq!(trace.text(), "outer inner hi");
    assert_eq!(
        tool_result_lines(&tool_lines(&trace)),
        vec![
            "Only: tool_result id= alias=inner trusted=true content=inner hi".to_owned(),
            "Only: tool_result id=call_1 alias=outer trusted=true content=outer inner hi"
                .to_owned(),
        ],
        "each call reports under its own id, the inner call closing first"
    );
}

#[test]
fn a_handler_dispatches_a_bound_tool_as_one_leaf_request() {
    let source = one_section(
        "tools.offer('tools/echo')
         tools.offer_local('grab', 'Grab a value', { value = 'string' }, function(args)
           return tools.call('tools/echo', { value = args.value })
         end)
         local out = tools.call('grab', { value = 'hi' })
         return out .. '|' .. tostring(tools.calls.grab) .. '|' .. tostring(tools.calls['tools/echo'])",
    );
    let trace = arm_scenario(source).offer("echo", echo_tool()).run();
    assert_eq!(trace.text(), "echoed: hi|1|1", "both calls count once");
    assert_eq!(
        trace.effects().len(),
        1,
        "the echo call is the one leaf request; the local call issues none"
    );
}

#[test]
fn a_bound_failure_inside_a_model_issued_local_call_raises_kind_tool_in_the_handler() {
    let source = one_section(
        "tools.offer('tools/failing')
         tools.offer_local('grab', 'Grab a value', {}, function()
           local ok, err = pcall(tools.call, 'tools/failing', {})
           assert(not ok, 'the bound failure raises inside the handler')
           return err.kind
         end)
         return tools.call_as_model('call_1', 'grab', {})",
    );
    let trace = arm_scenario(source).offer("fail", failing_tool()).run();
    assert_eq!(
        trace.text(),
        "tool",
        "the handler's call is a script call, raising kind `tool`"
    );
    assert_eq!(
        tool_result_lines(&tool_lines(&trace)),
        vec!["Only: tool_result id=call_1 alias=grab trusted=true content=tool".to_owned()],
        "the inner failure fires no ToolResult; only the local call reports under call_1"
    );
}

#[test]
fn a_handlers_text_stays_trusted_and_a_bound_tools_wrapper_survives() {
    let source = one_section(
        "tools.offer('tools/untrusted_echo')
         tools.offer_local('plain', 'Plain', {}, function() return 'plain text' end)
         tools.offer_local('grab', 'Grab a value', { value = 'string' }, function(args)
           return tools.call('tools/untrusted_echo', { value = args.value })
         end)
         return tools.call('plain', {}) .. '||' .. tools.call('grab', { value = 'hi' })",
    );
    let trace = arm_scenario(source)
        .offer("echo", untrusted_echo_tool())
        .run();
    let (plain, wrapped) = trace
        .text()
        .split_once("||")
        .expect("the block joins both results");
    assert_eq!(plain, "plain text", "a handler's own text gets no wrapper");
    assert!(
        wrapped.contains("<untrusted_input_")
            && wrapped.contains("</untrusted_input_")
            && wrapped.contains("echoed: hi"),
        "the bound tool's untrusted output keeps its wrapper through the handler: {wrapped}"
    );
    let lines = tool_result_lines(&tool_lines(&trace));
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
