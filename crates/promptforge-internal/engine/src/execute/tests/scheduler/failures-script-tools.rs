//! Script-initiated `tools.call` dispatch on the scheduler by canonical id:
//! string and table results, tool objects, unbound names, offered tools
//! outside the section scope, cancellation of a slow call, untrusted output
//! wrapping, and the model install around a call.

use super::*;
use crate::execute::tests::fixtures::{echo_tool, structured_tool, untrusted_echo_tool};

#[test]
fn a_script_tools_call_dispatches_and_resumes_as_a_string() {
    // The whole script path in one pass: the shim yields, the scheduler
    // dispatches the bound tool, the plain binding resumes as a Lua
    // string, and the counts land in the same `tools.calls` table the
    // prose loop feeds.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        tools.offer('tools/echo')\n\
        local out = tools.call('tools/echo', { value = 'hi' })\n\
        return out .. '|' .. tostring(tools.calls['tools/echo'])\n\
        ```\n";
    let trace = scheduler_scenario(md).offer("echo", echo_tool()).run();
    assert_eq!(trace.text(), "echoed: hi|1");
}

#[test]
fn a_script_tools_call_with_a_tool_object_dispatches_its_binding() {
    // The object form: `tools.get` reads the tool object, and passing it as
    // the leading argument dispatches the tool it names, identically to the
    // bare id string.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        tools.offer('tools/echo')\n\
        local echo = tools.get('tools/echo')\n\
        assert(type(echo) == 'userdata', 'tools.get reads a tool object')\n\
        return tools.call(echo, { value = 'hi' })\n\
        ```\n";
    let trace = scheduler_scenario(md).offer("echo", echo_tool()).run();
    assert_eq!(trace.text(), "echoed: hi");
}

#[test]
fn a_script_tools_call_with_an_unbound_name_lists_the_offered_ids() {
    // Script-initiated resolution runs against the run's whole offering,
    // so the unknown-name error lists every offered id, not the section's
    // effective scope.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\ntools.offer('tools/echo')\nreturn tools.call('missing', {})\n```\n";
    let trace = scheduler_scenario(md).offer("echo", echo_tool()).run();
    match trace.error().inner() {
        Error::UnboundToolCall { name, ids } => {
            assert_eq!(name, "missing");
            assert_eq!(ids, &["tools/echo".to_owned()]);
        }
        other => panic!("expected the typed unbound-tool error, got {other:?}"),
    }
}

#[test]
fn a_script_tools_call_reaches_an_offered_tool_outside_the_section_scope() {
    // A tool in the run's offering but never scoped into the section (no
    // `tools.offer`) still dispatches for a script: the scope shapes what
    // the model is offered, and the author's own code is not the model.
    // The count lands in the same shared map `tools.calls` reads.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        local out = tools.call('tools/echo', { value = 'hi' })\n\
        return out .. '|' .. tostring(tools.calls['tools/echo'])\n\
        ```\n";
    let trace = scheduler_scenario(md).offer("echo", echo_tool()).run();
    assert_eq!(trace.text(), "echoed: hi|1");
}

#[test]
fn cancellation_interrupts_a_slow_script_tools_call() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\ntools.offer('test/tools/slow')\nreturn tools.call('test/tools/slow', {})\n```\n";
    let started = Arc::new(AtomicUsize::new(0));
    let dispatched = Arc::clone(&started);
    let trace = scheduler_scenario(md)
        .offer("slow", slow_tool())
        .hold(move |effect| {
            let slow = calls_slow_tool(effect);
            if slow {
                dispatched.fetch_add(1, Ordering::SeqCst);
            }
            slow
        })
        .cancel_when(calls_slow_tool)
        .run();

    assert!(
        matches!(trace.result(), RunResult::Cancelled),
        "cancelling a suspended tools.call must interrupt the run, got {:?}",
        trace.result()
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "the cancellation must land after the tool call was in flight"
    );
}

#[test]
fn an_untrusted_script_tools_call_result_is_nonce_wrapped() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        tools.offer('tools/untrusted_echo')\n\
        return tools.call('tools/untrusted_echo', { value = 'hi' })\n\
        ```\n";
    let trace = scheduler_scenario(md)
        .offer("fetch", untrusted_echo_tool())
        .run();
    let out = trace.text();
    assert!(
        out.contains("<untrusted_input_") && out.contains("</untrusted_input_"),
        "the script must receive the nonce-wrapped envelope, got: {out}"
    );
    assert!(
        out.contains("echoed: hi"),
        "the wrapped block must still include the tool output, got: {out}"
    );
}

#[test]
fn a_structured_binding_resumes_as_a_lua_table() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        tools.offer('tools/structured')\n\
        local r = tools.call('tools/structured', {})\n\
        return r.text .. '|' .. tostring(#r.images)\n\
        ```\n";
    let trace = scheduler_scenario(md)
        .offer(
            "form",
            structured_tool("{\"text\":\"typed\",\"images\":[]}", true),
        )
        .run();
    assert_eq!(trace.text(), "typed|0");
}

#[test]
fn invalid_json_from_a_structured_tool_is_a_tool_error() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\ntools.offer('tools/structured')\nreturn tools.call('tools/structured', {})\n```\n";
    let trace = scheduler_scenario(md)
        .offer("form", structured_tool("not json", true))
        .run();
    match trace.error().inner() {
        Error::Tool { message, .. } => {
            assert!(
                message.contains("returned invalid JSON"),
                "the tool error names the invalid JSON, got: {message}"
            );
        }
        other => panic!("expected the typed tool error, got {other:?}"),
    }
}

#[test]
fn an_untrusted_structured_output_is_wrapped_before_classification() {
    // The untrusted nonce wrap precedes the structured JSON parse, so an
    // untrusted binding's valid JSON still fails the call: this ordering is
    // what restricts structured output to trusted tools. If classification
    // ever ran on the raw output, this test would resume a table and fail.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\ntools.offer('tools/structured')\nreturn tools.call('tools/structured', {})\n```\n";
    let trace = scheduler_scenario(md)
        .offer("form", structured_tool("{\"text\":\"typed\"}", false))
        .run();
    match trace.error().inner() {
        Error::Tool { message, .. } => {
            assert!(
                message.contains("returned invalid JSON"),
                "the wrap must precede the parse, got: {message}"
            );
        }
        other => panic!("expected the typed tool error, got {other:?}"),
    }
}

#[test]
fn a_script_tools_call_before_infer_keeps_the_model_install() {
    // The one-time section scope install is shared between the first script
    // dispatch and the model resolution: a script `tools.call` that runs
    // first must not swallow the install a later `models.infer` relies on.
    let gateway = ScriptedChat::new(vec![text("prose answer")]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\ntools.offer('tools/echo')\ntools.call('tools/echo', { value = 'x' })\n```\n\n\
        Say something.\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let trace = scheduler_scenario(md)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "prose answer");
}

#[test]
fn a_document_prompt_without_tools_call_is_unaffected() {
    // Bindings installed, shim present, `tools.call` never called: the
    // section runs exactly as before the dispatch arm existed.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\ntools.offer('tools/echo')\nreturn 'plain'\n```\n";
    let trace = scheduler_scenario(md).offer("echo", echo_tool()).run();
    assert_eq!(trace.text(), "plain");
}
