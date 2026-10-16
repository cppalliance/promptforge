//! One finite pipeline end to end through the unified prompt model: explicit
//! lazy `prose`, `models.infer`, the Rust-backed `models.loop` with tool
//! dispatch, `messages.new()` builders, and synchronous `call`, driven
//! through a run against a scripted gateway at `promptforge: 0`.

use promptforge_parser::test_support::PromptSource;

use super::context::with_writer;
use super::fixtures::echo_tool;
use super::*;

#[test]
fn finite_pipeline_runs_the_unified_surface_end_to_end() {
    let gateway = ScriptedChat::new(vec![
        text("draft text"),
        calls(&[("call_1", "tools_echo", "{\"value\":\"polish\"}")]),
        text("refined text"),
    ]);

    let source = with_writer(PromptSource::new().front("plugins: [tools]"))
        .prose(
            "Draft",
            "
            Summarize in one word: {{ args }}

            ```lua
            var.draft = models.infer(prose)
            ```",
        )
        .section(
            "Refine",
            "
            tools.offer('tools/echo')
            local msgs = messages.new()
            msgs:system('You refine drafts.')
            msgs:user(var.draft)
            assert(models.loop(msgs) == nil, 'models.loop returns nil')
            assert(msgs[#msgs].role == 'assistant', 'the terminal record is the assistant reply')
            return call('## Deliver', msgs[#msgs].content)",
        )
        .section("Deliver", "return 'delivered: ' .. args");
    let trace = Scenario::new(source.build())
        .args("quantum")
        .bound()
        .tool(echo_tool())
        .chat(&gateway)
        .run();

    assert_eq!(trace.text(), "delivered: refined text");
    assert_eq!(
        gateway.call_count(),
        3,
        "one infer round plus two loop rounds (tool call, then terminal text)"
    );
    let bodies = gateway.requests();
    assert_eq!(
        bodies[0].messages[0].content(),
        "Summarize in one word: quantum",
        "the infer round sends the substituted lazy prose: {bodies:?}"
    );
    assert_eq!(
        bodies[1].messages[0].role(),
        Role::System,
        "the loop sends the builder's system message first: {bodies:?}"
    );
    assert_eq!(
        bodies[1].messages[1].content(),
        "draft text",
        "the loop sends the infer result as the user message: {bodies:?}"
    );
    assert_eq!(
        bodies[1].tools[0].name(),
        "tools_echo",
        "the loop advertises the section's tool scope: {bodies:?}"
    );
    let tool_turn = bodies[2]
        .messages
        .iter()
        .find(|message| message.role() == Role::Tool)
        .expect("the second loop round answers the tool call");
    assert_eq!(
        tool_turn.content(),
        "echoed: polish",
        "the dispatched tool result resumes into the loop: {bodies:?}"
    );
}
