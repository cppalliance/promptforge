//! Library code issues effects as part of the calling section: `store`,
//! model, and tool calls while it loads, and prompt-wide state an H1
//! `require` sets for every later section.

use promptforge_model_client::detail::tool_schema_name;

use crate::test_support::scripted_chat::{ScriptedChat, text};

use super::super::super::echo_tool;
use super::*;

/// A `require` case over `source` with the echo tool, the test model as
/// the current model, and `chat` answering its model rounds.
fn effect_case(source: &PromptSource, chat: &ScriptedChat) -> Scenario {
    require_case(source.build())
        .tool(echo_tool())
        .chat(chat)
        .bound()
}

/// The fixture tools Plugin and the `writer` role every effect case
/// declares.
fn effect_source() -> PromptSource {
    PromptSource::new().front("plugins: [tools]\nmodels: { writer: {} }")
}

#[test]
fn library_code_makes_store_model_and_tool_calls_while_it_loads() {
    let source = effect_source()
        .h1("models.default('writer')")
        .section("Main", "return require '## loader'")
        .section(
            "loader",
            "
            store.write('lib.md', 'stored')
            local echoed = tools.call('tools/echo', { value = 'lib' })
            local reply = models.infer('ping')
            return table.concat({ store.read('lib.md'), tostring(echoed), tostring(reply) }, '|')",
        );
    let gateway = ScriptedChat::new(vec![text("pong")]);
    let trace = effect_case(&source, &gateway).run();
    assert_eq!(trace.text(), "stored|echoed: lib|pong");
    assert_eq!(trace.read("lib.md").expect("the write landed"), "stored");
    assert_eq!(gateway.requests().len(), 1, "one model round ran");
}

#[test]
fn an_h1_library_sets_prompt_wide_state_every_later_section_sees() {
    let source = effect_source()
        .h1("require '## setup'")
        .section(
            "Main",
            "
            require '## offers'
            local msgs = messages.new()
            msgs:user('ping')
            models.loop(msgs)
            return msgs[#msgs].content",
        )
        .section("setup", "models.default('writer')")
        .section("offers", "tools.offer('tools/echo')");
    let gateway = ScriptedChat::new(vec![text("pong")]);
    let trace = effect_case(&source, &gateway).run();
    assert_eq!(
        trace.text(),
        "pong",
        "the loop runs under the H1 library's default"
    );
    let round = gateway
        .last_request()
        .expect("the round reaches the gateway");
    let offered: Vec<&str> = round.tools.iter().map(tool_schema_name).collect();
    assert_eq!(
        offered,
        ["tools_echo"],
        "the offers library's offer reaches the requiring section's round"
    );
}
