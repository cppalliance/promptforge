//! Capability preludes in a run: every section VM installs the preludes
//! handed to `Environment::preludes`, the shared chunk, fanout arms, and
//! spawned task chains included; a prelude that fails to load or whose
//! global collides fails the run as a Lua failure before it issues any
//! effect; and a prelude's functions, called from a block, reach the
//! yielding store and tool shims.

use promptforge_types::capabilities::{CapabilityId, Prelude};

use super::serial_driver::{perform_locally, text_reply};
use super::*;
use crate::execute::protocol::StoreOp;
use crate::execute::run::{Effect, EffectAnswer, EffectRecord, Run, ToolCallOrigin, ToolCaller};
use crate::test_support::drive;

/// A prelude contributed by the capability `id`.
fn prelude(id: &str, source: &str) -> Prelude {
    Prelude::new(
        CapabilityId::parse(id).expect("a valid capability id"),
        source,
    )
}

/// A prelude defining one table global whose function greets a name.
const GREETER: &str = "greeter = { greeting = 'hello' }\n\
    function greeter.greet(name)\n\
      return greeter.greeting .. ' ' .. name\n\
    end";

/// A prompt whose H1 block writes to the store before any section runs, so
/// a run that sets up its first section VM issues a store effect at once.
const EFFECT_FIRST: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Preludes\n\n\
    ```lua\nstore.write('h1.txt', 'x')\n```\n\n\
    ## Only\n\n\
    ```lua\nreturn 'done'\n```\n";

/// Prepares `md` against the echo fixture's catalog, the test model, and
/// `preludes`, as the Harness does, and starts a run over the prepared context.
fn prelude_run(md: &str, preludes: Vec<Prelude>) -> Run {
    let prompt = parse(md);
    let (catalog, _table) = fixture_tools(&[Arc::new(EchoTool) as Arc<dyn TestTool>]);
    let model = test_model_catalog()
        .models()
        .first()
        .cloned()
        .expect("the test catalog holds one model");
    let (ctx, requirements) = Environment::new()
        .tools(catalog)
        .preludes(preludes)
        .prepare(&prompt, test_context(EXECUTION).model(model));
    assert!(
        requirements.is_satisfied(),
        "the fixture prompt's slots fill: {requirements:?}"
    );
    Run::new(Arc::new(prompt), "", ctx)
}

/// Drives `run` to its end on the serial driver, answering a tool call
/// with the echo of its `value` and every other effect locally, and
/// returns the result beside the record of every effect, in issue order.
fn drive_recorded(run: Run) -> (RunResult, Vec<EffectRecord>) {
    let mut records = Vec::new();
    let (result, _events) = drive(run, |_, effect| {
        records.push(effect.record());
        match effect {
            Effect::ToolCall { args, .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted(
                format!("echoed: {}", args["value"].as_str().unwrap_or_default()),
            ))),
            other => perform_locally(other, &mut |_| text_reply("unused")),
        }
    });
    (result, records)
}

/// The text of a run that succeeds.
fn succeeded(result: RunResult) -> String {
    match result {
        RunResult::Ok(text) => text,
        other => panic!("the run succeeds: {other:?}"),
    }
}

/// Asserts `md` issues an effect when run without preludes, and that with
/// `preludes` it instead fails as a Lua failure naming every fragment,
/// before it issues any effect.
fn assert_fails_before_any_effect(md: &str, preludes: Vec<Prelude>, fragments: &[&str]) {
    let (control, control_records) = drive_recorded(prelude_run(md, Vec::new()));
    succeeded(control);
    assert!(
        !control_records.is_empty(),
        "without preludes the prompt issues an effect, so the check below is not vacuous"
    );
    let (result, records) = drive_recorded(prelude_run(md, preludes));
    let RunResult::Failure(error) = result else {
        panic!("the prelude fails the run: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::Lua, "{error:?}");
    let message = error.to_string();
    for fragment in fragments {
        assert!(
            message.contains(fragment),
            "the failure names `{fragment}`: {message}"
        );
    }
    assert!(records.is_empty(), "no effect is issued: {records:?}");
}

#[test]
fn a_prelude_that_calls_a_tool_while_loading_fails_the_run_as_lua_before_its_first_effect() {
    assert_fails_before_any_effect(
        EFFECT_FIRST,
        vec![prelude(
            "acme/eager",
            "tools.call('tests/tools/echo', { value = 'early' })",
        )],
        &[
            "capability `acme/eager`: its prelude failed to load",
            "it must not call tools while loading",
        ],
    );
}

#[test]
fn two_preludes_defining_one_global_fail_the_run_as_lua_before_its_first_effect() {
    assert_fails_before_any_effect(
        EFFECT_FIRST,
        vec![
            prelude("acme/first", GREETER),
            prelude("acme/second", "greeter = {}"),
        ],
        &[
            "capability `acme/second`: its prelude defines the global `greeter`",
            "which capability `acme/first`'s prelude already defines",
        ],
    );
}

#[test]
fn a_prelude_global_named_like_a_frontmatter_tool_alias_fails_the_run() {
    let md = EFFECT_FIRST.replace(
        "promptforge: 0\n",
        "promptforge: 0\ncapabilities:\n  - tests/tools\ntools:\n  echo: tests/tools/echo\n",
    );
    assert_fails_before_any_effect(
        &md,
        vec![prelude("acme/echo", "echo = {}")],
        &[
            "capability `acme/echo`: its prelude defines the global `echo`",
            "which the prompt's frontmatter binds as a tool or model alias",
        ],
    );
}

#[test]
fn a_prelude_global_named_like_a_frontmatter_model_alias_fails_the_run() {
    let md = EFFECT_FIRST.replace(
        "promptforge: 0\n",
        "promptforge: 0\nmodels:\n  writer: {}\n",
    );
    assert_fails_before_any_effect(
        &md,
        vec![prelude("acme/writer", "writer = 'mine'")],
        &[
            "capability `acme/writer`: its prelude defines the global `writer`",
            "which the prompt's frontmatter binds as a tool or model alias",
        ],
    );
}

#[test]
fn a_prelude_defining_ui_or_item_collides_on_a_run_that_binds_neither() {
    for name in ["ui", "item"] {
        assert_fails_before_any_effect(
            EFFECT_FIRST,
            vec![prelude("acme/reserved", &format!("{name} = {{}}"))],
            &[
                &format!("capability `acme/reserved`: its prelude defines the global `{name}`"),
                "which is reserved as an Engine global",
            ],
        );
    }
}

#[test]
fn the_shared_chunk_a_fanout_arm_and_a_spawned_task_all_see_a_prelude_global() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Preludes\n\n\
        ```lua shared\n\
        local at_load = greeter.greet('shared')\n\
        function shared_greeting() return at_load end\n\
        ```\n\n\
        ## Main\n\n\
        ```lua\n\
        local arms = fanout('### Arm', { 'arm' })\n\
        local t = tasks.spawn('## Child')\n\
        local _, ok, child = tasks.join_any({ t })\n\
        assert(ok, tostring(child))\n\
        return shared_greeting() .. '|' .. arms[1].text .. '|' .. child\n\
        ```\n\n\
        ### Arm\n\n\
        ```lua\nreturn greeter.greet(item)\n```\n\n\
        ## Child\n\n\
        ```lua\nreturn greeter.greet('task')\n```\n";
    let (result, _) = drive_recorded(prelude_run(md, vec![prelude("acme/greeter", GREETER)]));
    assert_eq!(
        succeeded(result),
        "hello shared|hello arm|hello task",
        "the shared chunk reads the prelude while loading, and the arm and the task chain see it"
    );
}

#[test]
fn a_prelude_function_called_from_a_block_reaches_the_yielding_store_shims() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Preludes\n\n\
        ## Only\n\n\
        ```lua\nreturn notes.keep('kept')\n```\n";
    let notes = "notes = {}\n\
        function notes.keep(text)\n\
          store.write('notes.md', text)\n\
          return store.read('notes.md')\n\
        end";
    let (result, records) = drive_recorded(prelude_run(md, vec![prelude("acme/notes", notes)]));
    assert_eq!(succeeded(result), "kept");
    assert_eq!(
        records,
        vec![
            EffectRecord::Store {
                op: StoreOp::Write {
                    path: "notes.md".to_owned(),
                    contents: "kept".to_owned(),
                },
            },
            EffectRecord::Store {
                op: StoreOp::Read {
                    path: "notes.md".to_owned(),
                    start: None,
                    end: None,
                },
            },
        ],
        "each store call inside the prelude function is a store effect"
    );
}

#[test]
fn a_tool_call_made_inside_a_prelude_function_records_a_script_caller() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Preludes\n\n\
        ## Only\n\n\
        ```lua\nreturn echoer.say('hi')\n```\n";
    let echoer = "echoer = {}\n\
        function echoer.say(value)\n\
          return tools.call('tests/tools/echo', { value = value })\n\
        end";
    let (result, records) = drive_recorded(prelude_run(md, vec![prelude("acme/echoer", echoer)]));
    assert_eq!(succeeded(result), "echoed: hi");
    assert_eq!(
        records,
        vec![EffectRecord::ToolCall {
            tool: ToolId::parse("tests/tools/echo").expect("a valid id"),
            alias: "tests/tools/echo".to_owned(),
            args: json!({ "value": "hi" }),
            origin: ToolCallOrigin {
                execution: EXECUTION.to_owned(),
                section: "Only".to_owned(),
                caller: ToolCaller::Script,
            },
        }]
    );
}
