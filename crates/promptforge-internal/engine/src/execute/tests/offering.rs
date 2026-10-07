//! Tests for the offering: the run's catalog tools whose Plugin the prompt
//! doesn't declare, bound under model-facing names that `tools.offered()`
//! lists, `tools.add` scopes, and both script and model calls resolve.

use super::models_loop::loop_models;
use super::*;
use crate::execute::run::{EffectRecord, ToolCallOrigin, ToolCaller};
use crate::execute::scope::{DispatchTarget, prepare_scoped_tools};
use promptforge_model_client::detail::tool_schema_new;

/// A catalog tool under any id, echoing its `value` argument.
struct Offered {
    id: &'static str,
    schema: Value,
}

impl Offered {
    fn at(id: &'static str) -> Arc<dyn TestTool> {
        Arc::new(Self {
            id,
            schema: json!({ "type": "object", "properties": { "value": { "type": "string" } } }),
        })
    }

    /// A tool whose parameters are not a JSON object, which no model
    /// round can advertise.
    fn shapeless(id: &'static str) -> Arc<dyn TestTool> {
        Arc::new(Self {
            id,
            schema: json!("scalar"),
        })
    }
}

#[async_trait::async_trait]
impl TestTool for Offered {
    fn id(&self) -> ToolId {
        ToolId::parse(self.id).expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "offered"
    }

    fn description(&self) -> &str {
        self.id
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    async fn call(&self, args: Value) -> std::result::Result<ToolOutput, ToolError> {
        let value = args["value"]
            .as_str()
            .ok_or_else(|| ToolError::message("the value argument is required"))?;
        Ok(ToolOutput::trusted(format!("{}: {value}", self.id)))
    }
}

/// Prepares `prompt` against a catalog of `tools` and builds the run state
/// with the loop models filled, beside the fixture that holds the
/// implementations.
fn offering_context(prompt: &Prompt, tools: &[Arc<dyn TestTool>]) -> (RunState, RunFixture) {
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

/// A one-section prompt around `lua` under `frontmatter`, the lines
/// between `promptforge: 0` and the closing fence.
fn offering_prompt(frontmatter: &str, lua: &str) -> Prompt {
    parse(&format!(
        "---\nname: t\ndescription: d\npromptforge: 0\n{frontmatter}---\n\n# Offering\n\n## Only\n\n```lua\n{lua}\n```\n"
    ))
}

/// The offered bindings' names and ids, in the set's order.
fn offered(ctx: &RunState) -> Vec<(String, String)> {
    ctx.tool_set_snapshot()
        .expect("the tool set mutex is not poisoned")
        .offered()
        .iter()
        .map(|binding| (binding.alias().to_owned(), binding.id().to_string()))
        .collect()
}

fn pair(name: &str, id: &str) -> (String, String) {
    (name.to_owned(), id.to_owned())
}

#[test]
fn the_offering_binds_every_undeclared_plugins_tool_under_its_model_facing_name_in_id_order() {
    let prompt = offering_prompt(
        "plugins:\n  - tools\ntools:\n  echo: tools/echo\n",
        "return 'ok'",
    );
    let (ctx, _) = offering_context(
        &prompt,
        &[
            Arc::new(EchoTool),
            Offered::at("tools/concrete"),
            Offered::at("gh/search"),
            Offered::at("alpha/zed"),
            Offered::at("gh/issues.list"),
            Offered::at("alpha/fetch"),
        ],
    );
    assert_eq!(
        offered(&ctx),
        vec![
            pair("alpha_fetch", "alpha/fetch"),
            pair("alpha_zed", "alpha/zed"),
            pair("gh_issues_list", "gh/issues.list"),
            pair("gh_search", "gh/search"),
        ],
        "a declared Plugin's tools stay out, bound to a slot or not"
    );
    let set = ctx
        .tool_set_snapshot()
        .expect("the tool set mutex is not poisoned");
    let fetch = set
        .offered_binding("alpha_fetch")
        .expect("the offered name resolves");
    assert_eq!(
        fetch.description(),
        "alpha/fetch",
        "the binding holds the descriptor's own text"
    );
    assert!(
        set.binding("alpha_fetch").is_none(),
        "an offered name is never a frontmatter slot"
    );
}

#[test]
fn the_offering_leaves_out_a_name_that_is_aliased_reserved_unadvertisable_or_repeated() {
    let prompt = offering_prompt(
        "plugins:\n  - tools\ntools:\n  gh_list: tools/echo\n",
        "return 'ok'",
    );
    let (ctx, _) = offering_context(
        &prompt,
        &[
            Arc::new(EchoTool),
            Offered::at("gh/list"),
            Offered::at("task/cancel"),
            Offered::shapeless("bad/shape"),
            Offered::at("dup/a.b"),
            Offered::at("dup/a/b"),
            Offered::at("ok/fine"),
        ],
    );
    assert_eq!(
        offered(&ctx),
        vec![pair("dup_a_b", "dup/a/b"), pair("ok_fine", "ok/fine")],
        "the frontmatter alias, the task built-in name, the schema no round \
         can advertise, and the later repeat are each left out"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_adds_the_offering_and_the_model_calls_an_offered_tool_by_its_name() {
    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "tools_echo", r#"{"value":"hi"}"#),
        resp_text("done"),
    ]);
    let prompt = offering_prompt(
        "",
        "tools.add(tools.offered())\n\
         local history = messages.new()\n\
         history:user('go')\n\
         models.loop(history)\n\
         return 'ok'",
    );
    let (ctx, fixture) = offering_context(&prompt, &[Arc::new(EchoTool)]);
    let mut scheduler = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)));
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("the loop completes");
    assert_eq!(out, "ok");
    let records = records.lock().expect("the tap mutex is not poisoned");
    let advertised: Vec<Vec<String>> = records
        .iter()
        .filter_map(|record| match record {
            EffectRecord::Chat { tools, .. } => Some(tools.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(advertised, vec![vec!["tools_echo".to_owned()]; 2]);
    let calls: Vec<&EffectRecord> = records
        .iter()
        .filter(|record| matches!(record, EffectRecord::ToolCall { .. }))
        .collect();
    assert_eq!(
        calls,
        vec![&EffectRecord::ToolCall {
            tool: ToolId::parse("tools/echo").expect("valid id"),
            alias: "tools_echo".to_owned(),
            args: json!({ "value": "hi" }),
            origin: ToolCallOrigin {
                execution: EXECUTION.to_owned(),
                section: "Only".to_owned(),
                caller: ToolCaller::Model,
            },
        }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_calls_an_offered_tool_by_its_record_and_by_its_name_without_adding_it() {
    let prompt = offering_prompt(
        "",
        "local record = tools.offered()[1]\n\
         return tools.call(record, { value = 'a' }) .. '|' .. \
           tools.call('tools_echo', { value = 'b' }) .. '|' .. record.id .. '|' .. record.plugin",
    );
    let (ctx, fixture) = offering_context(&prompt, &[Arc::new(EchoTool)]);
    let mut scheduler = TokioDriver::new(&ctx, fixture, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("both calls reach the tool");
    assert_eq!(out, "echoed: a|echoed: b|tools/echo|tools");
    let call = |value: &str| EffectRecord::ToolCall {
        tool: ToolId::parse("tools/echo").expect("valid id"),
        alias: "tools_echo".to_owned(),
        args: json!({ "value": value }),
        origin: ToolCallOrigin {
            execution: EXECUTION.to_owned(),
            section: "Only".to_owned(),
            caller: ToolCaller::Script,
        },
    };
    assert_eq!(
        *records.lock().expect("the tap mutex is not poisoned"),
        vec![call("a"), call("b")]
    );
}

#[test]
fn a_round_leaves_out_an_offered_binding_a_local_tool_shares_a_name_with() {
    let offered = crate::lua::ToolBinding::from_descriptor("tools_echo", &EchoTool.descriptor());
    let local = tool_schema_new("tools_echo", "the local echo", json!({ "type": "object" }))
        .expect("the local schema is valid");
    let (schemas, dispatch) =
        prepare_scoped_tools(&[offered], &[local]).expect("the round's scope builds");
    assert_eq!(schemas.len(), 1, "the model sees one tool under the name");
    assert_eq!(tool_schema_description(&schemas[0]), "the local echo");
    assert!(
        matches!(dispatch.get("tools_echo"), Some(DispatchTarget::Local)),
        "the local tool answers the name: {dispatch:?}"
    );
}
