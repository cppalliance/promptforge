//! Tests for `tools.add_local`: the registration rules run end to end, and
//! the model-tool loop's local-dispatch arm is driven directly through the
//! test shim. Routing a local call back into the section VM returns with
//! the `models.loop` step; the loop arm's behavior is pinned here.

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

/// The advertised schema for the `grab` local tool the loop tests share.
fn local_grab_schema() -> ToolSchema {
    ToolSchema::new(
        "grab".to_string(),
        "Grab a value".to_string(),
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        }),
    )
    .expect("the local tool schema is valid")
}

/// The dispatch map marking `grab` as a local tool routed through the
/// section's local dispatcher.
fn local_grab_dispatch() -> BTreeMap<String, DispatchTarget> {
    let mut dispatch = BTreeMap::new();
    dispatch.insert("grab".to_string(), DispatchTarget::Local);
    dispatch
}

#[tokio::test]
async fn local_tool_handler_result_returns_to_the_model() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ])
    .await;
    let client = gateway_client(gateway.addr());
    let schemas = vec![local_grab_schema()];
    let dispatch = local_grab_dispatch();
    let local = |_name: &str, args: serde_json::Value| -> Result<String> {
        Ok(format!(
            "got {}",
            args["value"].as_str().expect("the value argument")
        ))
    };

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "Use the tool.".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        None,
        Some(&local),
    )
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
    let client = gateway_client(gateway.addr());
    let schemas = vec![local_grab_schema()];
    let dispatch = local_grab_dispatch();
    let calls = Mutex::new(Vec::new());
    let local = |_name: &str, args: serde_json::Value| -> Result<String> {
        let value = args["value"]
            .as_str()
            .expect("the value argument")
            .to_string();
        calls
            .lock()
            .expect("the calls mutex must not be poisoned")
            .push(value.clone());
        Ok(format!("ok {value}"))
    };

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "Use the tool.".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        None,
        Some(&local),
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");
    assert_eq!(
        calls
            .lock()
            .expect("the calls mutex must not be poisoned")
            .as_slice(),
        ["a".to_string(), "b".to_string()],
        "both calls in the one response must run"
    );

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
    let client = gateway_client(gateway.addr());
    let schemas = vec![local_grab_schema()];
    let dispatch = local_grab_dispatch();
    let local = |_name: &str, _args: serde_json::Value| -> Result<String> {
        Err(Error::Lua("handler exploded".to_string()))
    };
    let recorder = Arc::new(Recorder::default());

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let error = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "Use the tool.".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        recorder.as_ref(),
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        None,
        Some(&local),
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
async fn local_tool_alias_cannot_shadow_a_declared_tool() {
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Concrete description.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n```lua\n\
tools.bind('grab', 'capability')\n\
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
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
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
