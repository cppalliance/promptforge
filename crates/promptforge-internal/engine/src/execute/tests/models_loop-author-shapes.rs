//! Regression guards for the list shapes the shipped prompts use, run
//! against a canned model: the `prompts/research-person.md` section and
//! the shipped chat agent's turn loop.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn the_research_person_shape_returns_the_last_reply() {
    let gateway = ScriptedChat::new(vec![resp_text("a factual summary")]);
    let md = "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n\
        # Loop\n\n\
        ## Research\n\n\
        Summarize the person.\n\n\
        ```lua\n\
        local msgs = messages.new()\n\
        msgs:user(prose)\n\
        models.loop(msgs)\n\
        return msgs[#msgs].content\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the research shape runs to its reply");
    assert_eq!(out, "a factual summary");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].messages[0]
            .content()
            .contains("Summarize the person."),
        "the section prose is the user message: {bodies:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_chat_agent_shape_completes_both_turns() {
    let gateway = ScriptedChat::new(vec![resp_text("first reply"), resp_text("second reply")]);
    let md = loop_prompt(
        "local history = messages.new()\n\
         for _, text in ipairs({ 'first question', 'second question' }) do\n\
           history:user(text)\n\
           pcall(function() return models.loop(history) end)\n\
         end\n\
         return #history .. '|' .. history[2].content .. '|' .. history[4].content",
    );
    let prompt = parse(&md);
    let (ctx, fixture) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("both chat turns run");
    assert_eq!(out, "4|first reply|second reply");
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
