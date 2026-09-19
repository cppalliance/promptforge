//! Tests for the section-visible `models.loop`: the shim-driven model-tool
//! loop over an author-owned message list. One terminal turn with no tools,
//! repeated model-tool rounds with automatic assistant and tool-result
//! appends, the nil return, explicit terminal removal, local and bound
//! tools, call-time tool scope, explicit-handle calls on a frozen binding,
//! and the atomic append of a tool-call batch. The shared loop fixtures
//! (`loop_models`, `loop_context`, `loop_prompt`, the tool sets, and the
//! `loop_events` filter) live here for every loop-driven sibling. The
//! compactor argument's tests live in `models_loop_compactors`; the loop's
//! exit rules in `exit_rules`; its cap, scope gate, and result-record
//! trust in `tool_loop`.

use super::*;
use crate::execute::tokio_driver::TokioDriver;
use crate::lua::{ToolBinding, ToolSet};
use crate::model::{ModelBinding, ModelId};
use promptforge_model_client::model::ModelInvocation;

/// The model set a loop test's run carries: `writer` (the prompt-wide
/// default, model `test-model`) and `other` (model `other-model`), so an
/// explicit handle provably runs on its own frozen binding.
pub(super) fn loop_models() -> ModelSet {
    let binding = |alias: &str, description: &str, model: &str| {
        ModelBinding::new(
            alias,
            description,
            ModelId::from_validated("gateway", model),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
            NonZeroU32::new(4096).expect("4096 is non-zero"),
        )
    };
    ModelSet {
        bindings: vec![
            binding("writer", "A general model for tests", "test-model"),
            binding("other", "A second model", "other-model"),
        ],
        default: Some("writer".to_owned()),
    }
}

/// Builds the run context for a loop test under the given observer: the
/// parsed prompt, an empty shared library, and the shared model and tool
/// sets pre-filled (the scheduler tests bypass the live H1 pass that would
/// fill them).
pub(super) fn loop_context_observed(
    prompt: &Prompt,
    tools: ToolSet,
    observer: Arc<dyn Observer>,
) -> RunState {
    let ctx = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &TestStore::new().vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &test_context(EXECUTION).observer(observer),
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = loop_models();
    *ctx.tool_set()
        .lock()
        .expect("the tool set mutex is not poisoned") = tools;
    ctx
}

/// [`loop_context_observed`] under the null observer.
pub(super) fn loop_context(prompt: &Prompt, tools: ToolSet) -> RunState {
    loop_context_observed(prompt, tools, Arc::new(NullObserver::default()))
}

/// The tool set with `tool` bound as `alias` and always in scope.
pub(super) fn always_tool(alias: &str, tool: Arc<dyn Tool>) -> ToolSet {
    ToolSet::for_test(
        vec![ToolBinding::for_test(alias, "fixture capability", tool)],
        vec![alias.to_owned()],
    )
}

/// The tool set with the `echo` fixture bound and always in scope.
pub(super) fn echo_tools() -> ToolSet {
    always_tool("echo", Arc::new(EchoTool))
}

/// The model-turn and tool-call observations `recorder` saw, in order,
/// with every other boundary event (chunk, section, run) dropped, so a
/// prompt-level test reads exactly the sequence the loop's rounds report.
pub(super) fn loop_events(recorder: &Recorder) -> Vec<String> {
    let loop_details = [
        detail::MODEL_TURN_COMPLETED,
        detail::MODEL_TURN_FAILED,
        detail::MODEL_TURN_TRUNCATED,
        detail::TOOL_CALL_SUCCEEDED,
        detail::TOOL_CALL_FAILED,
    ]
    .map(|observation| observation.to_string());
    recorder
        .events()
        .into_iter()
        .map(|(_, detail)| detail)
        .filter(|detail| loop_details.contains(detail))
        .collect()
}

/// The one-section prompt shell every loop test drives.
pub(super) fn loop_prompt(lua: &str) -> String {
    format!(
        "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n# Loop\n\n## Only\n\n```lua\n{lua}\n```\n"
    )
}

#[tokio::test(flavor = "current_thread")]
async fn models_loop_appends_the_terminal_assistant_record_and_returns_nil() {
    let gateway = ScriptedGateway::start(vec![resp_text("final answer")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         local result = models.loop(msgs)\n\
         assert(result == nil, 'models.loop returns nil')\n\
         assert(#msgs == 2, 'the loop appended exactly the terminal record')\n\
         assert(msgs[2].role == 'assistant', 'the terminal record is an assistant message')\n\
         assert(msgs[2].content == 'final answer', 'the terminal record carries the reply text')\n\
         msgs[#msgs] = nil\n\
         assert(#msgs == 1, 'explicit terminal removal shrinks the list')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a tool-free loop runs to its terminal turn");
    assert_eq!(out, "ok");
    assert_eq!(gateway.call_count(), 1, "one terminal turn is one request");
    let bodies = gateway.requests();
    assert_eq!(bodies[0]["messages"][0]["role"], "user");
    assert_eq!(bodies[0]["messages"][0]["content"], "hello");
    assert!(
        bodies[0].get("tools").is_none() || bodies[0]["tools"].is_null(),
        "no tools in scope means no tools on the wire: {bodies:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn models_loop_repeats_model_tool_rounds_and_appends_each_exchange() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "echo", "{\"value\":\"one\"}"),
        resp_tool_call("call_2", "echo", "{\"value\":\"two\"}"),
        resp_text("both done"),
    ])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('echo twice')\n\
         models.loop(msgs)\n\
         assert(#msgs == 6, 'user plus two exchanges plus the terminal record')\n\
         assert(msgs[2].role == 'assistant', 'round one appends the assistant call')\n\
         assert(msgs[2].tool_calls[1].id == 'call_1', 'the assistant record carries the normalized call')\n\
         assert(msgs[2].tool_calls[1].name == 'echo', 'the call keeps its wire name')\n\
         assert(msgs[2].tool_calls[1].arguments.value == 'one', 'the call arguments stay parsed')\n\
         assert(msgs[3].role == 'tool', 'the correlated result follows its call')\n\
         assert(msgs[3].tool_call_id == 'call_1', 'the result answers call_1')\n\
         assert(msgs[3].content == 'echoed: one', 'the trusted result appends verbatim')\n\
         assert(msgs[4].tool_calls[1].id == 'call_2', 'round two appends its own call')\n\
         assert(msgs[5].tool_call_id == 'call_2', 'round two appends its own result')\n\
         assert(msgs[6].role == 'assistant' and msgs[6].content == 'both done', 'terminal text is the final record')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, echo_tools());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the loop repeats until terminal text");
    assert_eq!(out, "ok");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 3, "two tool rounds plus the terminal round");
    let tool_turns: Vec<&str> = bodies[2]["messages"]
        .as_array()
        .expect("a request body must carry a messages array")
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| {
            message["content"]
                .as_str()
                .expect("tool content is a string")
        })
        .collect();
    assert_eq!(
        tool_turns,
        ["echoed: one", "echoed: two"],
        "both exchanges ride the terminal round's conversation: {bodies:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn models_loop_dispatches_local_and_bound_tools() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"x\"}"),
        resp_tool_call("call_2", "echo", "{\"value\":\"y\"}"),
        resp_text("tools done"),
    ])
    .await;
    let md = loop_prompt(
        "tools.add_local('grab', 'Local grab', { value = 'string' }, function(args)\n\
           return 'grabbed ' .. args.value\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('use both tools')\n\
         models.loop(msgs)\n\
         assert(#msgs == 6, 'both exchanges and the terminal record appended')\n\
         assert(msgs[3].content == 'grabbed x', 'the local handler ran on the section VM')\n\
         assert(msgs[3].tool_call_id == 'call_1', 'the local result correlates its call')\n\
         assert(msgs[5].content == 'echoed: y', 'the bound tool ran through dispatch')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, echo_tools());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the loop routes local and bound tools");
    assert_eq!(out, "ok");
    let bodies = gateway.requests();
    assert_eq!(
        bodies[0]["tools"].as_array().map(Vec::len),
        Some(2),
        "both the local and the bound tool are advertised: {bodies:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn models_loop_reads_the_tool_scope_at_each_call() {
    let gateway = ScriptedGateway::start(vec![
        resp_text("no tools yet"),
        resp_tool_call("call_1", "echo", "{\"value\":\"late\"}"),
        resp_text("scoped in"),
    ])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('first')\n\
         models.loop(msgs)\n\
         tools.add('echo')\n\
         msgs:user('second')\n\
         models.loop(msgs)\n\
         assert(msgs[#msgs].content == 'scoped in', 'the second loop converged')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    // Nothing always-scoped: the first call advertises no tools, the
    // `tools.add` between calls scopes `echo` in for the second.
    let tools = ToolSet::for_test(
        vec![crate::lua::ToolBinding::for_test(
            "echo",
            "echo capability",
            Arc::new(EchoTool),
        )],
        Vec::new(),
    );
    let ctx = loop_context(&prompt, tools);
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("each call reads the current scope");
    assert_eq!(out, "ok");
    let bodies = gateway.requests();
    assert_eq!(
        bodies.len(),
        3,
        "one tool-free round, then a tool round and its terminal"
    );
    assert!(
        bodies[0].get("tools").is_none() || bodies[0]["tools"].is_null(),
        "the first call predates the tools.add: {bodies:?}"
    );
    assert_eq!(
        bodies[1]["tools"][0]["function"]["name"], "echo",
        "the second call advertises the newly added tool: {bodies:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn models_loop_with_a_leading_handle_runs_on_its_frozen_binding() {
    let gateway = ScriptedGateway::start(vec![resp_text("first"), resp_text("second")]).await;
    let md = loop_prompt(
        "local other = models.get('other')\n\
         local msgs = messages.new()\n\
         msgs:user('one')\n\
         models.loop(other, msgs)\n\
         msgs:user('two')\n\
         models.loop(other, msgs)\n\
         assert(#msgs == 4, 'each call appends its terminal record')\n\
         return msgs[2].content .. '|' .. msgs[4].content",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("an explicit handle runs at any point in the section");
    assert_eq!(out, "first|second");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 2, "two loops, two requests");
    for body in &bodies {
        assert_eq!(
            body["model"], "other-model",
            "the handle's frozen binding serves, not the section default: {bodies:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_author_list_never_shows_a_half_answered_tool_batch() {
    // Two calls in one round: each handler runs while its sibling is
    // unanswered, and the list it reads must not yet hold the batch's
    // assistant record. After the round the assistant record and both
    // results land together, in call order, ahead of the terminal text.
    let gateway = ScriptedGateway::start(vec![
        resp_two_tool_calls(
            "grab",
            ("c1", "{\"value\":\"a\"}"),
            ("c2", "{\"value\":\"b\"}"),
        ),
        resp_text("done"),
    ])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('grab twice')\n\
         local seen = {}\n\
         tools.add_local('grab', 'Local grab', { value = 'string' }, function(args)\n\
           seen[#seen + 1] = #msgs\n\
           for _, m in ipairs(msgs) do\n\
             assert(m.tool_calls == nil, 'no assistant call record is visible mid-batch')\n\
             assert(m.role ~= 'tool', 'no tool record is visible mid-batch')\n\
           end\n\
           return 'grabbed ' .. args.value\n\
         end)\n\
         models.loop(msgs)\n\
         assert(#seen == 2, 'both calls in the batch ran')\n\
         assert(seen[1] == 1 and seen[2] == 1, 'each handler saw only the user message')\n\
         assert(#msgs == 5, 'user, the batch record, two results, and the terminal text')\n\
         assert(msgs[2].role == 'assistant' and #msgs[2].tool_calls == 2, 'one record carries the whole batch')\n\
         assert(msgs[2].tool_calls[1].id == 'c1' and msgs[2].tool_calls[2].id == 'c2', 'calls keep their order')\n\
         assert(msgs[3].tool_call_id == 'c1' and msgs[3].content == 'grabbed a', 'the first result follows')\n\
         assert(msgs[4].tool_call_id == 'c2' and msgs[4].content == 'grabbed b', 'the second result follows')\n\
         assert(msgs[5].role == 'assistant' and msgs[5].content == 'done', 'the terminal text is last')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a two-call batch appends atomically");
    assert_eq!(out, "ok");
    let tool_turns = gateway.requests()[1]["messages"]
        .as_array()
        .expect("a request body must carry a messages array")
        .iter()
        .filter(|message| message["role"] == "tool")
        .count();
    assert_eq!(tool_turns, 2, "both results ride the terminal round");
}
