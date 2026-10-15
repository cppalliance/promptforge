//! Regression guards for the list shapes the shipped prompts use, run
//! against a canned model: the `prompts/research-person.md` section and
//! the shipped chat agent's turn loop.

use promptforge_parser::test_support::{PromptSource, one_section};

use super::*;

#[test]
fn the_research_person_shape_returns_the_last_reply() {
    let gateway = ScriptedChat::new(vec![text("a factual summary")]);
    let source = PromptSource::new().prose(
        "Research",
        "
        Summarize the person.

        ```lua
        local msgs = messages.new()
        msgs:user(prose)
        models.loop(msgs)
        return msgs[#msgs].content
        ```",
    );
    let trace = loop_scenario(source.build()).chat(&gateway).run();
    assert_eq!(trace.text(), "a factual summary");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].messages[0]
            .content()
            .contains("Summarize the person."),
        "the section prose is the user message: {bodies:?}"
    );
}

#[test]
fn the_chat_agent_shape_completes_both_turns() {
    let gateway = ScriptedChat::new(vec![text("first reply"), text("second reply")]);
    let source = one_section(
        "local history = messages.new()
         for _, text in ipairs({ 'first question', 'second question' }) do
           history:user(text)
           pcall(function() return models.loop(history) end)
         end
         return #history .. '|' .. history[2].content .. '|' .. history[4].content",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "4|first reply|second reply");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 2, "one request per turn");
    let second: Vec<&str> = bodies[1]
        .messages
        .iter()
        .map(crate::model::Message::content)
        .collect();
    assert_eq!(
        second,
        ["first question", "first reply", "second question"],
        "the second turn sends the history the first turn left"
    );
}
