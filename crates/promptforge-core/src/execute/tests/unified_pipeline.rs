//! One finite pipeline end to end through the unified prompt model: explicit
//! lazy `prose`, `models.infer`, the Rust-backed `models.loop` with tool
//! dispatch, `messages.new()` builders, the removed `reply` register, and
//! synchronous `call`, driven through the public `run` against a scripted
//! gateway at `promptforge: 0`.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn finite_pipeline_runs_the_unified_surface_end_to_end() {
    let gateway = ScriptedGateway::start(vec![
        resp_text("draft text"),
        resp_tool_call("call_1", "echo", "{\"value\":\"polish\"}"),
        resp_text("refined text"),
    ])
    .await;
    let addr = gateway.addr();

    let source = "---\nname: unified\ndescription: d\npromptforge: 0\n---\n\n\
        # Unified\n\n\
        ```lua\n\
        tools.bind('echo', 'echo capability')\n\
        models.default('writer', 'A general model for tests')\n\
        ```\n\n\
        ## Draft\n\n\
        Summarize in one word: {{ args }}\n\n\
        ```lua\n\
        assert(reply == nil, 'the removed reply register names no global')\n\
        var.draft = models.infer(prose)\n\
        ```\n\n\
        ## Refine\n\n\
        ```lua\n\
        tools.add('echo')\n\
        local msgs = messages.new()\n\
        msgs:system('You refine drafts.')\n\
        msgs:user(var.draft)\n\
        assert(models.loop(msgs) == nil, 'models.loop returns nil')\n\
        assert(msgs[#msgs].role == 'assistant', 'the terminal record is the assistant reply')\n\
        return call('## Deliver', msgs[#msgs].content)\n\
        ```\n\n\
        ## Deliver\n\n\
        ```lua\n\
        assert(reply == nil, 'a call hands off args, never a reply register')\n\
        return 'delivered: ' .. args\n\
        ```\n";
    let test = bound_with_tools(source, Vec::new());
    let out = super::run(
        &test,
        "quantum",
        &[Arc::new(EchoTool) as Arc<dyn Tool>],
        &TestStore::new(),
        gatewayed(addr),
    )
    .await
    .expect("the unified pipeline must run end to end");

    assert_eq!(out, "delivered: refined text");
    assert_eq!(
        gateway.call_count(),
        3,
        "one infer round plus two loop rounds (tool call, then terminal text)"
    );
    let bodies = gateway.requests();
    assert_eq!(
        bodies[0]["messages"][0]["content"], "Summarize in one word: quantum",
        "the infer round carries the substituted lazy prose: {bodies:?}"
    );
    assert_eq!(
        bodies[1]["messages"][0]["role"], "system",
        "the loop sends the builder's system message first: {bodies:?}"
    );
    assert_eq!(
        bodies[1]["messages"][1]["content"], "draft text",
        "the loop sends the infer result as the user message: {bodies:?}"
    );
    assert_eq!(
        bodies[1]["tools"][0]["function"]["name"], "echo",
        "the loop advertises the section's tool scope: {bodies:?}"
    );
    let tool_turn = bodies[2]["messages"]
        .as_array()
        .expect("a request body carries a messages array")
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("the second loop round answers the tool call");
    assert_eq!(
        tool_turn["content"], "echoed: polish",
        "the dispatched tool result resumes into the loop: {bodies:?}"
    );
}
