//! Tests for script tool calls by canonical id: a script's `tools.call`
//! names an offered tool by its id and records the tool's wire name, and
//! reaches by id a catalog tool the run could not offer, recording the id.
//! A call binds no global and advertises nothing; a wire name is the
//! model's alone, and a model-issued call never resolves an id.

use super::models_loop::loop_models;
use super::*;
use crate::execute::run::{EffectRecord, ToolCallOrigin, ToolCaller};
use crate::test_support::tokio_driver::TokioDriver;

/// The echo fixture's full id.
const ECHO_ID: &str = "tools/echo";

/// The echo fixture's wire name.
const ECHO_WIRE: &str = "tools_echo";

/// The echo fixture's full id as a [`ToolId`].
fn echo_id() -> ToolId {
    ToolId::parse(ECHO_ID).expect("valid id")
}

/// The origin of a script call made in the fixture prompt's one section.
fn script_origin() -> ToolCallOrigin {
    ToolCallOrigin {
        execution: EXECUTION.to_owned(),
        section: "Only".to_owned(),
        caller: ToolCaller::Script,
    }
}

/// A one-section prompt around `lua`.
fn full_id_prompt(lua: &str) -> Prompt {
    parse(&format!(
        "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# FullId\n\n## Only\n\n```lua\n{lua}\n```\n"
    ))
}

/// Prepares `prompt` against a catalog of `tools` and builds the run state
/// over the prepared context with the loop models pre-filled, beside the
/// fixture that holds the implementations.
fn catalog_context(prompt: &Prompt, tools: &[Arc<dyn TestTool>]) -> (RunState, RunFixture) {
    let (catalog, table) = fixture_tools(tools);
    let (prepared, requirements) = Environment::new()
        .tools(catalog)
        .prepare(prompt, test_context(EXECUTION));
    assert!(
        requirements.is_satisfied(),
        "the fixture prompt prepares: {requirements:?}"
    );
    let ctx = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &TestStore::new().vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &prepared,
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = loop_models();
    (ctx, RunFixture::new().tools(table))
}

/// The echo fixture and a second tool the prompts never offer.
fn echo_and_concrete() -> Vec<Arc<dyn TestTool>> {
    vec![
        Arc::new(EchoTool),
        Arc::new(ScopedFixtureTool::new("concrete", "Concrete description.")),
    ]
}

/// A catalog tool whose wire name, `task_cancel`, is a task built-in's, so
/// the run cannot offer it; it echoes its `value` argument.
struct Unofferable;

#[async_trait::async_trait]
impl TestTool for Unofferable {
    fn id(&self) -> ToolId {
        ToolId::parse("task/cancel").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "A tool the run cannot offer."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "value": { "type": "string" } } })
    }

    async fn call(&self, args: Value) -> std::result::Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted(format!(
            "unoffered: {}",
            args["value"].as_str().unwrap_or_default()
        )))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_call_by_id_records_the_offered_tools_wire_name() {
    let prompt = full_id_prompt("return tools.call('tools/echo', { value = 'hi' })");
    let (ctx, fixture) = catalog_context(&prompt, &[Arc::new(EchoTool)]);
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler
        .drive()
        .await
        .expect("an id call reaches the offered tool");
    assert_eq!(out, "echoed: hi");
    assert_eq!(
        *records.lock().expect("the tap mutex is not poisoned"),
        vec![EffectRecord::ToolCall {
            tool: echo_id(),
            alias: ECHO_WIRE.to_owned(),
            args: json!({ "value": "hi" }),
            origin: script_origin(),
        }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_reaches_a_catalog_tool_left_out_of_the_offering_by_id() {
    let prompt = full_id_prompt(
        "assert(tools.get('task/cancel') == nil, 'the tool is not offered')\n\
         return tools.call('task/cancel', { value = 'hi' })",
    );
    let (ctx, fixture) = catalog_context(&prompt, &[Arc::new(Unofferable)]);
    assert!(
        ctx.tool_set_snapshot()
            .expect("the tool set mutex is not poisoned")
            .offered()
            .is_empty(),
        "a tool whose wire name is a task built-in's is left out of the offering"
    );
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler
        .drive()
        .await
        .expect("the call runs through the catalog bindings");
    assert_eq!(out, "unoffered: hi");
    assert_eq!(
        *records.lock().expect("the tap mutex is not poisoned"),
        vec![EffectRecord::ToolCall {
            tool: ToolId::parse("task/cancel").expect("valid id"),
            alias: "task/cancel".to_owned(),
            args: json!({ "value": "hi" }),
            origin: script_origin(),
        }],
        "the id stands as the alias of a tool the run could not offer"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_call_advertises_nothing_until_the_tool_is_offered() {
    let gateway = ScriptedChat::new(vec![resp_text("before"), resp_text("after")]);
    let prompt = full_id_prompt(
        "tools.call('tools/echo', { value = 'a' })\n\
         tools.call('tools/concrete', { value = 'b' })\n\
         local before = messages.new()\n\
         before:user('before')\n\
         models.loop(before)\n\
         tools.offer('tools/echo')\n\
         local after = messages.new()\n\
         after:user('after')\n\
         models.loop(after)\n\
         return 'ok'",
    );
    let (ctx, fixture) = catalog_context(&prompt, &echo_and_concrete());
    let mut scheduler = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)));
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("both rounds complete");
    assert_eq!(out, "ok");
    let advertised: Vec<Vec<String>> = records
        .lock()
        .expect("the tap mutex is not poisoned")
        .iter()
        .filter_map(|record| match record {
            EffectRecord::Chat { tools, .. } => Some(tools.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        advertised,
        vec![Vec::new(), vec![ECHO_WIRE.to_owned()]],
        "a script call scopes nothing; only the offered tool is advertised"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_call_by_wire_name_or_local_looking_name_is_unbound() {
    let prompt = full_id_prompt(
        "local wire_ok, wire_err = pcall(tools.call, 'tools_echo', {})\n\
         local bare_ok, bare_err = pcall(tools.call, 'echo', {})\n\
         assert(not wire_ok and not bare_ok, 'neither name is a catalog id')\n\
         return wire_err.kind .. ':' .. wire_err.name .. '|' .. bare_err.kind .. ':' .. bare_err.name",
    );
    let (ctx, fixture) = catalog_context(&prompt, &[Arc::new(EchoTool)]);
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler
        .drive()
        .await
        .expect("both refusals are pcall-able");
    assert_eq!(out, "unbound_tool:tools_echo|unbound_tool:echo");
    assert!(
        records
            .lock()
            .expect("the tap mutex is not poisoned")
            .is_empty(),
        "nothing was issued"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn no_global_is_bound_under_a_wire_name_or_an_id() {
    let prompt = full_id_prompt(
        "tools.offer('tools/echo')\n\
         tools.call('tools/concrete', { value = 'x' })\n\
         local named = {}\n\
         for name in next, _G do\n\
           if type(name) == 'string' and (name:find('/', 1, true) or name:find('tools_', 1, true)) then\n\
             named[#named + 1] = name\n\
           end\n\
         end\n\
         table.sort(named)\n\
         return type(tools_echo) .. '|' .. table.concat(named, ',')",
    );
    let (ctx, fixture) = catalog_context(&prompt, &echo_and_concrete());
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the section runs");
    assert_eq!(out, "nil|", "no tool is a global under any name");
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_call_by_id_and_by_tool_object_issue_the_same_effect() {
    let prompt = full_id_prompt(
        "return tools.call('tools/echo', { value = 'hi' }) .. '|' .. \
         tools.call(tools.get('tools/echo'), { value = 'hi' })",
    );
    let (ctx, fixture) = catalog_context(&prompt, &[Arc::new(EchoTool)]);
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("both calls reach the tool");
    assert_eq!(out, "echoed: hi|echoed: hi");
    let call = EffectRecord::ToolCall {
        tool: echo_id(),
        alias: ECHO_WIRE.to_owned(),
        args: json!({ "value": "hi" }),
        origin: script_origin(),
    };
    assert_eq!(
        *records.lock().expect("the tap mutex is not poisoned"),
        vec![call.clone(), call]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_name_that_is_no_catalog_id_is_unbound_and_lists_the_offered_ids() {
    let prompt = full_id_prompt("return tools.call('tools/missing', {})");
    let (ctx, fixture) = catalog_context(&prompt, &echo_and_concrete());
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let error = scheduler
        .drive()
        .await
        .expect_err("an unknown name fails the block");
    match &error {
        Error::UnboundToolCall { name, ids } => {
            assert_eq!(name, "tools/missing");
            assert_eq!(
                ids,
                &["tools/concrete".to_owned(), ECHO_ID.to_owned()],
                "every offered tool's id is listed"
            );
        }
        other => panic!("expected the typed unbound-tool error, got {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "tool \"tools/missing\" is not a tool in this run; \
         catalog tools: [\"tools/concrete\", \"tools/echo\"]",
        "the message lists the offered ids"
    );
    assert!(
        records
            .lock()
            .expect("the tap mutex is not poisoned")
            .is_empty(),
        "nothing was issued"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_model_issued_call_never_resolves_an_id() {
    let prompt = full_id_prompt(
        "local ok, err = pcall(tools.call_as_model, 'call_1', 'tools/echo', { value = 'hi' })\n\
         assert(not ok, 'a model-issued id call is refused')\n\
         return err.kind .. ':' .. err.name",
    );
    let (mut ctx, fixture) = catalog_context(&prompt, &[Arc::new(EchoTool)]);
    ctx.expose_raw_shims_for_test();
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("the refusal is pcall-able");
    assert_eq!(out, "unbound_tool:tools/echo");
    assert!(
        records
            .lock()
            .expect("the tap mutex is not poisoned")
            .is_empty(),
        "nothing was issued"
    );
}
