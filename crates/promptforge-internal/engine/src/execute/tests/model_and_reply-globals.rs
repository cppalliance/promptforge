//! What a section reads through `sys.model`, `item`, and `reply`:
//! `sys.model` is unknown before the one-time scope install and reads
//! the catalog id after it, in prose, the epilog, and a fanout arm; a
//! table `item` member substitutes as compact JSON; and `reply` is not
//! a global: it reads nil in Lua and fails as an unknown global in
//! prose.

use promptforge_parser::test_support::one_section;

use super::*;
use crate::execute::tests::context::{setup_prompt, with_writer};
use crate::execute::tests::fixtures::echo_tool;

/// The prompt `source` builds over the fixture tools Plugin, run under the
/// test model with the echo tool: the scope install the first
/// `tools.call` runs is what `sys.model` reads after.
#[track_caller]
fn under_tools(source: PromptSource) -> Trace {
    let source = setup_prompt(source.front("plugins: [tools]"));
    Scenario::new(source).bound().tool(echo_tool()).run()
}

#[test]
fn a_library_function_sees_sys_model_unknown_before_scope_close() {
    let source = with_writer(PromptSource::new())
        .prose(
            "Only",
            "
            ```lua
            require '## sys_reader'
            return read_sys_model()
            ```

            prose",
        )
        .section(
            "sys_reader",
            "
            function read_sys_model()
              return sys.model
            end",
        );
    let trace = Scenario::new(source.build()).bound().run();
    let error = trace.error();
    assert!(
        error.to_string().contains("unknown sys field 'model'"),
        "error must name the missing field: {error}"
    );
}

#[test]
fn prologue_sys_model_unknown_before_scope_close() {
    let source =
        with_writer(PromptSource::new()).prose("Only", "```lua\nreturn sys.model\n```\n\nprose");
    let trace = Scenario::new(source.build()).bound().run();
    let error = trace.error();
    assert!(
        error.to_string().contains("unknown sys field 'model'"),
        "error must name the missing field: {error}"
    );
}

#[test]
fn prose_substitution_sees_sys_model_catalog_id() {
    // The first script dispatch runs the one-time scope install, which
    // enriches `sys.model` with the bound catalog id; a prose read after it
    // substitutes the catalog id, not the alias.
    let trace = under_tools(PromptSource::new().prose(
        "Only",
        "
        ```lua
        tools.call('tools/echo', { value = 'x' })
        ```

        Model id is {{ sys.model }}.

        ```lua
        return prose
        ```",
    ));
    assert_eq!(trace.text(), "Model id is claude-sonnet-4-6.");
}

#[test]
fn epilog_sees_model_catalog_id_not_alias_after_the_scope_install() {
    let trace = under_tools(PromptSource::new().prose(
        "Only",
        "
        ```lua
        tools.call('tools/echo', { value = 'x' })
        ```

        ```lua
        return sys.model
        ```",
    ));
    assert_eq!(trace.text(), "claude-sonnet-4-6");
}

#[test]
fn fanout_arm_sees_sys_model_catalog_id_after_the_scope_install() {
    let trace = under_tools(
        PromptSource::new()
            .section(
                "Parent",
                "
                local r = tasks.fanout('### Worker', prompt.list('### Items'))
                return table.concat(r, ',')",
            )
            .prose(
                "### Worker",
                "
                ```lua
                tools.call('tools/echo', { value = item })
                ```

                ```lua
                return sys.model .. ':' .. item
                ```",
            )
            .prose("### Items", "- a"),
    );
    assert_eq!(trace.text(), "claude-sonnet-4-6:a");
}

/// `{{ item }}` renders a non-string member per its type: here a table
/// member reaches the model as compact JSON.
#[test]
fn fanout_item_substitution_renders_a_table_member_as_compact_json() {
    let source = with_writer(PromptSource::new())
        .section(
            "Parent",
            "
            local r = tasks.fanout('### Worker', {{7, 'x'}})
            return r[1].text",
        )
        .prose(
            "### Worker",
            "
            ```lua
            -- prologue
            ```

            Item: {{ item }}.

            ```lua
            return models.infer(prose)
            ```",
        );
    let gateway = ScriptedChat::new(vec![text("hello from the mock")]);
    let trace = Scenario::new(source.build()).bound().chat(&gateway).run();
    assert_eq!(trace.text(), "hello from the mock");

    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    let user_content = body
        .messages
        .first()
        .map(crate::model::Message::content)
        .expect("first message must hold substituted prose");
    assert!(
        user_content.contains("Item: [7,\"x\"]."),
        "a table member must render as compact JSON, got: {user_content}"
    );
}

#[test]
fn reply_is_nil_in_first_section() {
    let trace = Scenario::new(one_section("return tostring(reply)")).run();
    assert_eq!(trace.text(), "nil");
}

#[test]
fn reply_substitution_is_an_unknown_global_error() {
    // `{{ reply }}` names no namespace and no bare global, so reading the
    // prose fails at the read site.
    let source = PromptSource::new().prose("Only", "{{ reply }}\n\n```lua\nreturn prose\n```");
    let trace = Scenario::new(source.build()).run();
    let err = trace.error();
    assert!(
        err.to_string().contains("reply"),
        "error must mention reply, got: {err}"
    );
}
