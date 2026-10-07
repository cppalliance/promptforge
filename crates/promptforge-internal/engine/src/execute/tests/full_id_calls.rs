//! Tests for script tool calls by full id: a script's `tools.call` may name
//! any tool in the run's catalog by its full id, which binds no global and
//! advertises nothing. `tools.add` and `tools.always` still take only
//! aliases, and a model-issued call never resolves a full id.

use super::models_loop::loop_models;
use super::*;
use crate::execute::run::{EffectRecord, ToolCallOrigin, ToolCaller};
use crate::test_support::tokio_driver::TokioDriver;

/// The echo fixture's full id.
const ECHO_ID: &str = "tools/echo";

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

/// A one-section prompt around `lua`. `slots` holds the YAML lines under
/// the frontmatter's `tools:` key, or is empty for a prompt that binds no
/// alias.
fn full_id_prompt(slots: &str, lua: &str) -> Prompt {
    let tools = if slots.is_empty() {
        String::new()
    } else {
        format!("plugins:\n  - tools\ntools:\n{slots}")
    };
    parse(&format!(
        "---\nname: t\ndescription: d\npromptforge: 0\n{tools}---\n\n# FullId\n\n## Only\n\n```lua\n{lua}\n```\n"
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
        "the fixture prompt's slots fill: {requirements:?}"
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

/// The echo fixture and a second tool the prompts never alias.
fn echo_and_concrete() -> Vec<Arc<dyn TestTool>> {
    vec![
        Arc::new(EchoTool),
        Arc::new(ScopedFixtureTool::new("concrete", "Concrete description.")),
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_calls_an_unaliased_catalog_tool_by_full_id_and_records_the_full_id_as_alias() {
    let prompt = full_id_prompt("", "return tools.call('tools/echo', { value = 'hi' })");
    let (ctx, fixture) = catalog_context(&prompt, &[Arc::new(EchoTool)]);
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler
        .drive()
        .await
        .expect("a full-id call reaches the catalog tool");
    assert_eq!(out, "echoed: hi");
    assert_eq!(
        *records.lock().expect("the tap mutex is not poisoned"),
        vec![EffectRecord::ToolCall {
            tool: echo_id(),
            alias: ECHO_ID.to_owned(),
            args: json!({ "value": "hi" }),
            origin: script_origin(),
        }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_full_id_call_advertises_nothing_until_the_tool_is_bound_under_an_alias_and_added() {
    let gateway = ScriptedChat::new(vec![resp_text("before"), resp_text("after")]);
    let prompt = full_id_prompt(
        "  echo: tools/echo\n",
        "tools.call('tools/echo', { value = 'a' })\n\
         tools.call('tools/concrete', { value = 'b' })\n\
         local before = messages.new()\n\
         before:user('before')\n\
         models.loop(before)\n\
         tools.add('echo')\n\
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
        vec![Vec::new(), vec!["echo".to_owned()]],
        "a full-id call scopes nothing; only the added alias is advertised"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn tools_add_and_tools_always_reject_a_full_id_as_an_invalid_alias() {
    let prompt = full_id_prompt(
        "  echo: tools/echo\n",
        "local add_ok, add_err = pcall(tools.add, 'tools/echo')\n\
         local always_ok, always_err = pcall(tools.always, 'tools/echo')\n\
         assert(not add_ok and not always_ok, 'a full id is not an alias')\n\
         return tostring(add_err) .. '\\n' .. tostring(always_err)",
    );
    let (ctx, fixture) = catalog_context(&prompt, &[Arc::new(EchoTool)]);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("both refusals are pcall-able");
    let (add, always) = out.split_once('\n').expect("the block joins both errors");
    for message in [add, always] {
        assert!(
            message.contains("invalid alias \"tools/echo\""),
            "a full id fails alias validation, got: {message}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn no_global_is_bound_under_a_full_id() {
    let prompt = full_id_prompt(
        "  echo: tools/echo\n",
        "tools.call('tools/concrete', { value = 'x' })\n\
         local slashed = {}\n\
         for name in next, _G do\n\
           if type(name) == 'string' and name:find('/', 1, true) then\n\
             slashed[#slashed + 1] = name\n\
           end\n\
         end\n\
         table.sort(slashed)\n\
         return type(echo) .. '|' .. table.concat(slashed, ',')",
    );
    let (ctx, fixture) = catalog_context(&prompt, &echo_and_concrete());
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the section runs");
    assert_eq!(
        out, "userdata|",
        "the frontmatter alias is a global and no full id is"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_frontmatter_bound_tool_issues_the_same_effect_by_alias_and_by_full_id_apart_from_the_alias()
 {
    let prompt = full_id_prompt(
        "  echo: tools/echo\n",
        "return tools.call('echo', { value = 'hi' }) .. '|' .. \
         tools.call('tools/echo', { value = 'hi' })",
    );
    let (ctx, fixture) = catalog_context(&prompt, &[Arc::new(EchoTool)]);
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("both calls reach the tool");
    assert_eq!(out, "echoed: hi|echoed: hi");
    let call = |alias: &str| EffectRecord::ToolCall {
        tool: echo_id(),
        alias: alias.to_owned(),
        args: json!({ "value": "hi" }),
        origin: script_origin(),
    };
    assert_eq!(
        *records.lock().expect("the tap mutex is not poisoned"),
        vec![call("echo"), call(ECHO_ID)]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_name_that_is_neither_an_alias_nor_a_catalog_id_is_still_unbound() {
    let prompt = full_id_prompt(
        "  echo: tools/echo\n",
        "return tools.call('tools/missing', {})",
    );
    let (ctx, fixture) = catalog_context(&prompt, &echo_and_concrete());
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let error = scheduler
        .drive()
        .await
        .expect_err("an unknown name fails the block");
    match &error {
        Error::UnboundToolCall { name, bound } => {
            assert_eq!(name, "tools/missing");
            assert_eq!(bound, &["echo".to_owned()], "only aliases are listed");
        }
        other => panic!("expected the typed unbound-tool error, got {other:?}"),
    }
    assert!(
        error.to_string().contains("bound aliases: [\"echo\"]"),
        "the message lists the bound aliases: {error}"
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
async fn a_model_issued_call_never_resolves_a_full_id() {
    let prompt = full_id_prompt(
        "",
        "local ok, err = pcall(tools.call_as_model, 'call_1', 'tools/echo', { value = 'hi' })\n\
         assert(not ok, 'a model-issued full-id call is refused')\n\
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
