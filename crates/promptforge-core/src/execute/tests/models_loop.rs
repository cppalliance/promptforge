//! Tests for the section-visible `models.loop`: the Rust-backed model-tool
//! loop over an author-owned message list. One terminal turn with no tools,
//! repeated model-tool rounds with automatic assistant and tool-result
//! appends, the nil return, explicit terminal removal, local and bound
//! tools, call-time tool scope, the omitted-compactor default,
//! `compactors.fail` invocation with the overflow reason, typed context
//! exhaustion, and explicit-handle calls on a frozen binding.

use super::*;
use crate::execute::scheduler::Scheduler;
use crate::lua::{OverflowReason, ToolSet};
use crate::model::{ModelBinding, ModelId, ModelInvocation};

/// The model set a loop test's run carries: `writer` (the prompt-wide
/// default, model `test-model`) and `other` (model `other-model`), so an
/// explicit handle provably runs on its own frozen binding.
fn loop_models() -> ModelSet {
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

/// Builds the run context for a loop test: the parsed prompt, an empty
/// shared library, and the shared model and tool sets pre-filled (the
/// scheduler tests bypass the live H1 pass that would fill them).
fn loop_context(prompt: &Prompt, tools: ToolSet) -> RunContext {
    let ctx = RunContext::new(
        prompt,
        "",
        &StoreRef::memory(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &RunConfig::new(EXECUTION),
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = loop_models();
    *ctx.tool_set()
        .lock()
        .expect("the tool set mutex is not poisoned") = tools;
    ctx
}

/// The tool set with the `echo` fixture bound and always in scope.
fn echo_tools() -> ToolSet {
    ToolSet::for_test(
        vec![crate::lua::ToolBinding::for_test(
            "echo",
            "echo capability",
            Arc::new(EchoTool),
        )],
        vec!["echo".to_owned()],
    )
}

/// The one-section prompt shell every loop test drives.
fn loop_prompt(lua: &str) -> String {
    format!(
        "---\nname: loop\ndescription: d\npromptforge: 1\n---\n\n# Loop\n\n## Only\n\n```lua\n{lua}\n```\n"
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
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
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
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
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
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
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
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
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
async fn an_omitted_compactor_defaults_to_fail_with_typed_precheck_exhaustion() {
    let gateway = ScriptedGateway::start(vec![resp_text("unreachable")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user(string.rep('x', 100000))\n\
         models.loop(msgs)\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let error = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("an over-window request must exhaust the context");
    assert!(
        matches!(
            error,
            Error::ContextExhausted {
                reason: OverflowReason::Precheck
            }
        ),
        "the omitted compactor defaults to compactors.fail with the precheck reason, got {error:?}"
    );
    assert_eq!(
        gateway.call_count(),
        0,
        "the precheck fires before any request leaves"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn models_loop_raises_context_exhaustion_at_the_call_site() {
    let gateway = ScriptedGateway::start(vec![resp_text("unreachable")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user(string.rep('x', 100000))\n\
         local ok, err = pcall(models.loop, msgs)\n\
         assert(not ok, 'the overflow raises')\n\
         assert(#msgs == 1, 'a refused dispatch appends nothing')\n\
         return tostring(err)",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    assert!(
        out.starts_with("context exhausted: "),
        "the raised error is the typed exhaustion's message, got: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_explicit_compactors_fail_invocation_carries_the_provider_reason() {
    let gateway = ScriptedGateway::start(vec![resp_status(
        400,
        "This model's maximum context length is 4096 tokens.",
    )])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('a small prompt')\n\
         models.loop(msgs, compactors.fail)\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let error = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("a provider context rejection must exhaust the context");
    assert!(
        matches!(
            error,
            Error::ContextExhausted {
                reason: OverflowReason::Provider
            }
        ),
        "the explicit compactors.fail invocation carries the provider reason, got {error:?}"
    );
    assert_eq!(gateway.call_count(), 1, "the request left and was rejected");
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
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
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
