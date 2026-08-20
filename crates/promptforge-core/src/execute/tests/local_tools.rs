//! End-to-end tests for `tools.add_local` - Lua-backed tools dispatched on the
//! section VM - plus the `tools.add`-between-prose-blocks regression.

use super::super::*;
use super::run;
use super::*;

/// A response asking the model to call one tool twice in a single turn.
fn resp_two_tool_calls(name: &str, first: (&str, &str), second: (&str, &str)) -> GatewayReply {
    GatewayReply::Json(json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": first.0,
                        "type": "function",
                        "function": { "name": name, "arguments": first.1 }
                    },
                    {
                        "id": second.0,
                        "type": "function",
                        "function": { "name": name, "arguments": second.1 }
                    }
                ]
            }
        }]
    }))
}

/// Run `md` against a scripted gateway with no external tools, returning the
/// run result. The fixture gets the standard `models.default` H1 binding.
async fn run_local(test: &TestPrompt, addr: SocketAddr, store: &StoreRef) -> Result<String> {
    run(
        test,
        "",
        &[],
        store,
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
}

#[tokio::test]
async fn local_tool_handler_result_returns_to_the_model() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ])
    .await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\n\
tools['add_local']('grab', 'Grab a value', { value = 'string' }, function(args)\n  return 'got ' .. args.value\nend)\n\
```\n\n\
Use the tool.\n";
    let out = run_local(&bound_for_model(md), addr, &StoreRef::memory())
        .await
        .unwrap();
    assert_eq!(out, "final answer");

    let bodies = gateway.requests();
    let function = &bodies[0]["tools"][0]["function"];
    assert_eq!(function["name"], "grab");
    assert_eq!(function["description"], "Grab a value");
    assert_eq!(
        function["parameters"],
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        })
    );
    // The handler's trusted return reaches the model verbatim (no guard wrap).
    assert_eq!(last_tool_turn_content(&bodies), "got hi");
}

#[tokio::test]
async fn local_tool_handler_store_writes_persist() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ])
    .await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\n\
tools['add_local']('grab', 'Grab a value', { value = 'string' }, function(args)\n  store.write('tool-out.txt', args.value)\n  return 'stored'\nend)\n\
```\n\n\
Use the tool.\n";
    let store = StoreRef::memory();
    let out = run_local(&bound_for_model(md), addr, &store).await.unwrap();
    assert_eq!(out, "final answer");
    assert_eq!(store.read("tool-out.txt").unwrap(), "hi");
}

#[tokio::test]
async fn local_tool_multiple_calls_in_one_response_all_run() {
    let gateway = ScriptedGateway::start(vec![
        resp_two_tool_calls(
            "grab",
            ("c1", "{\"value\":\"a\"}"),
            ("c2", "{\"value\":\"b\"}"),
        ),
        resp_text("final answer"),
    ])
    .await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\n\
tools['add_local']('grab', 'Grab a value', { value = 'string' }, function(args)\n  store.append('calls.txt', args.value .. ';')\n  return 'ok ' .. args.value\nend)\n\
```\n\n\
Use the tool.\n";
    let store = StoreRef::memory();
    let out = run_local(&bound_for_model(md), addr, &store).await.unwrap();
    assert_eq!(out, "final answer");
    assert_eq!(store.read("calls.txt").unwrap(), "a;b;");

    let bodies = gateway.requests();
    let tool_turns = bodies[1]["messages"]
        .as_array()
        .expect("a request body must carry a messages array")
        .iter()
        .filter(|m| m["role"] == "tool")
        .count();
    assert_eq!(
        tool_turns, 2,
        "both handler results must go back: {bodies:?}"
    );
}

#[tokio::test]
async fn local_tool_handler_error_surfaces_as_a_tool_failure() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("unreachable"),
    ])
    .await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\n\
tools['add_local']('grab', 'Grab a value', { value = 'string' }, function(_args)\n  error('handler exploded')\nend)\n\
```\n\n\
Use the tool.\n";
    let recorder = Arc::new(Recorder::default());
    let error = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect_err("a handler Lua error must fail the tool call");
    assert!(
        error.to_string().contains("handler exploded"),
        "the handler's error must surface: {error}"
    );
    assert!(
        recorder
            .events()
            .contains(&("Only".to_string(), detail::TOOL_CALL_FAILED.to_string())),
        "the failed handler must be observed as a tool-call failure"
    );
}

#[tokio::test]
async fn local_tool_handler_shares_section_globals_with_later_chunks() {
    let gateway = ScriptedGateway::start(vec![
        resp_two_tool_calls(
            "grab",
            ("c1", "{\"value\":\"a\"}"),
            ("c2", "{\"value\":\"b\"}"),
        ),
        resp_text("final answer"),
    ])
    .await;
    let addr = gateway.addr();
    // The accumulator pattern: the handler appends to a section-global table
    // and a later chunk reads it, proving handler and chunks share one VM.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\n\
collected = {}\n\
tools['add_local']('grab', 'Grab a value', { value = 'string' }, function(args)\n  table.insert(collected, args.value)\n  return 'noted ' .. args.value\nend)\n\
```\n\n\
Use the tool.\n\n\
```lua\nreturn 'sum:' .. table.concat(collected, ',')\n```\n";
    let out = run_local(&bound_for_model(md), addr, &StoreRef::memory())
        .await
        .unwrap();
    assert_eq!(out, "sum:a,b");
}

#[tokio::test]
async fn local_tool_handler_cannot_jump() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("unreachable"),
    ])
    .await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\n\
tools['add_local']('grab', 'Grab a value', { value = 'string' }, function(_args)\n  jump('## Nowhere')\n  return 'x'\nend)\n\
```\n\n\
Use the tool.\n\n\
## Nowhere\n\n\
```lua\nreturn 'jumped'\n```\n";
    let error = run_local(&bound_for_model(md), addr, &StoreRef::memory())
        .await
        .expect_err("jump is nil inside a local tool handler");
    assert!(
        error.to_string().contains("jump"),
        "the nil-call error must name jump: {error}"
    );
}

#[tokio::test]
async fn local_tool_alias_cannot_shadow_a_declared_tool() {
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Concrete description.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua\n\
tools.need('grab', 'capability')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
## Only\n\n\
```lua\n\
tools.add_local('grab', 'Local grab', {}, function() return 'local' end)\n\
```\n",
        Vec::new(),
    );

    let error = run(
        &prompt,
        "",
        &[tool as Arc<dyn Tool>],
        &StoreRef::memory(),
        silent(),
    )
    .await
    .expect_err("a local alias must not shadow a declared tool");
    assert!(
        error
            .to_string()
            .contains("duplicates a declared tool alias"),
        "the error must identify the declared-alias collision: {error}"
    );
}

#[tokio::test]
async fn local_tool_alias_cannot_be_registered_twice() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
## Only\n\n\
```lua\n\
tools.add_local('grab', 'First grab', {}, function() return 'first' end)\n\
tools.add_local('grab', 'Second grab', {}, function() return 'second' end)\n\
```\n";
    let error = run_offline(md)
        .await
        .expect_err("a local alias must not be registered twice");
    assert!(
        error.to_string().contains("is already registered"),
        "the error must identify the duplicate local alias: {error}"
    );
}

#[tokio::test]
async fn tools_add_between_prose_blocks_takes_effect_on_the_second_block() {
    let gateway = ScriptedGateway::start(vec![
        resp_text("first"),
        resp_tool_call("c1", "section_tool", "{\"value\":\"x\"}"),
        resp_text("second"),
    ])
    .await;
    let addr = gateway.addr();
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Section concrete.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.need('section_tool', 'capability')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
## Only\n\n\
First ask.\n\n\
```lua\ntools.add('section_tool')\n```\n\n\
Second ask.\n",
        Vec::new(),
    );

    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "second");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let bodies = gateway.requests();
    assert!(
        bodies[0].get("tools").is_none(),
        "the first prose block predates tools.add: {bodies:?}"
    );
    assert_eq!(
        bodies[1]["tools"][0]["function"]["name"], "section_tool",
        "the second prose block must see the added tool: {bodies:?}"
    );
}
