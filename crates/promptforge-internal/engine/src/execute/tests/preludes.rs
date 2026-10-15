//! Plugin preludes in a run: every section VM installs the preludes
//! handed to `RunContext::preludes`, fanout arms and spawned task chains
//! included, and the libraries a section requires see them; a prelude
//! that fails to load or whose global collides fails the run as a Lua
//! failure before it issues any effect; a store conflict a prelude's
//! install-time write meets ends the run with a determinism violation,
//! caught or not; and a prelude's functions, called from a block, reach
//! the yielding store and tool shims.

use promptforge_types::plugins::{PluginId, Prelude};

use super::*;
use crate::execute::protocol::VfsOp;
use crate::execute::run::{EffectRecord, ToolCallOrigin, ToolCaller};

/// A prelude contributed by the Plugin `id`.
pub(super) fn prelude(id: &str, source: &str) -> Prelude {
    Prelude::new(PluginId::parse(id).expect("a valid Plugin id"), source)
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

/// Runs `md` prepared against the echo tool as its catalog, the test model,
/// and `preludes`.
#[track_caller]
fn prelude_run(md: &str, preludes: Vec<Prelude>) -> Trace {
    Scenario::new(md)
        .bound()
        .tool(echo_tool())
        .context(move |ctx| ctx.preludes(preludes))
        .run()
}

/// The record of every effect `trace`'s run issued, in issue order.
fn records(trace: &Trace) -> Vec<EffectRecord> {
    trace
        .effects()
        .iter()
        .map(|(effect, _)| effect.clone())
        .collect()
}

/// Asserts `md` issues an effect when run without preludes, and that with
/// `preludes` it instead fails as a Lua failure naming every fragment,
/// before it issues any effect.
#[track_caller]
fn assert_fails_before_any_effect(md: &str, preludes: Vec<Prelude>, fragments: &[&str]) {
    let control = prelude_run(md, Vec::new());
    assert!(
        matches!(control.result(), RunResult::Ok(_)),
        "without preludes the prompt succeeds: {:?}",
        control.result()
    );
    assert!(
        !control.effects().is_empty(),
        "without preludes the prompt issues an effect, so the check below is not vacuous"
    );
    let trace = prelude_run(md, preludes);
    let error = trace.error();
    assert_eq!(error.kind(), RunErrorKind::Lua, "{error:?}");
    let message = error.to_string();
    for fragment in fragments {
        assert!(
            message.contains(fragment),
            "the failure names `{fragment}`: {message}"
        );
    }
    let records = records(&trace);
    assert!(records.is_empty(), "no effect is issued: {records:?}");
}

#[test]
fn a_prelude_that_calls_a_tool_while_loading_fails_the_run_as_lua_before_its_first_effect() {
    assert_fails_before_any_effect(
        EFFECT_FIRST,
        vec![prelude(
            "eager",
            "tools.call('tools/echo', { value = 'early' })",
        )],
        &[
            "Plugin `eager`: its prelude failed to load",
            "it must not call tools while loading",
        ],
    );
}

#[test]
fn two_preludes_defining_one_global_fail_the_run_as_lua_before_its_first_effect() {
    assert_fails_before_any_effect(
        EFFECT_FIRST,
        vec![prelude("first", GREETER), prelude("second", "greeter = {}")],
        &[
            "Plugin `second`: its prelude defines the global `greeter`",
            "which Plugin `first`'s prelude already defines",
        ],
    );
}

#[test]
fn a_prelude_global_named_like_an_offered_tools_wire_name_installs() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tools\n---\n\n\
        # Preludes\n\n\
        ## Only\n\n\
        ```lua\nreturn tools_echo .. '|' .. tools.get('tools/echo').id\n```\n";
    let trace = prelude_run(md, vec![prelude("echoes", "tools_echo = 'mine'")]);
    assert_eq!(
        trace.text(),
        "mine|tools/echo",
        "no tool is a global, so the prelude's global and the tool sit side by side"
    );
}

#[test]
fn a_prelude_global_named_like_a_model_role_label_installs() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Preludes\n\n\
        ## Only\n\n\
        ```lua\nreturn writer .. '|' .. models.get('writer').name\n```\n";
    let trace = prelude_run(md, vec![prelude("writer", "writer = 'mine'")]);
    assert_eq!(
        trace.text(),
        "mine|writer",
        "the prelude's global and the role's handle sit side by side"
    );
}

#[test]
fn a_prelude_defining_ui_or_item_collides_on_a_run_that_binds_neither() {
    for name in ["ui", "item"] {
        assert_fails_before_any_effect(
            EFFECT_FIRST,
            vec![prelude("reserved", &format!("{name} = {{}}"))],
            &[
                &format!("Plugin `reserved`: its prelude defines the global `{name}`"),
                "which is reserved as an Engine global",
            ],
        );
    }
}

#[test]
fn a_library_a_fanout_arm_and_a_spawned_task_all_see_a_prelude_global() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Preludes\n\n\
        ## Main\n\n\
        ```lua\n\
        require '## greetings'\n\
        local arms = tasks.fanout('### Arm', { 'arm' })\n\
        local t = tasks.spawn('## Child')\n\
        local _, ok, child = tasks.join_any({ t })\n\
        assert(ok, tostring(child))\n\
        return library_greeting() .. '|' .. arms[1].text .. '|' .. child\n\
        ```\n\n\
        ### Arm\n\n\
        ```lua\nreturn greeter.greet(item)\n```\n\n\
        ## Child\n\n\
        ```lua\nreturn greeter.greet('task')\n```\n\n\
        ## greetings\n\n\
        ```lua\n\
        local at_load = greeter.greet('library')\n\
        function library_greeting() return at_load end\n\
        ```\n";
    let trace = prelude_run(md, vec![prelude("greeter", GREETER)]);
    assert_eq!(
        trace.text(),
        "hello library|hello arm|hello task",
        "the library reads the prelude while loading, and the arm and the task chain see it"
    );
}

#[test]
fn a_prelude_write_across_two_arms_ends_the_run_with_determinism_caught_or_not() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Preludes\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = tasks.fanout('### Worker', { 'alpha', 'beta' })\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn item\n```\n";
    for source in [
        "store.write('seeded.txt', 'seed')",
        "pcall(function() store.write('seeded.txt', 'seed') end)",
    ] {
        let trace = prelude_run(md, vec![prelude("seeder", source)]);
        let RunResult::Failure(error) = trace.result() else {
            panic!(
                "the second arm's install-time write ends the run: `{source}`: {:?}",
                trace.result()
            );
        };
        assert_eq!(
            error.kind(),
            RunErrorKind::Determinism,
            "the conflict wins over the prelude's own outcome: `{source}`: {error:?}"
        );
    }
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
    let trace = prelude_run(md, vec![prelude("notes", notes)]);
    assert_eq!(trace.text(), "kept");
    assert_eq!(
        records(&trace),
        vec![
            EffectRecord::Vfs {
                op: VfsOp::Write {
                    path: "notes.md".to_owned(),
                    contents: "kept".to_owned(),
                },
            },
            EffectRecord::Vfs {
                op: VfsOp::Read {
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
          return tools.call('tools/echo', { value = value })\n\
        end";
    let trace = prelude_run(md, vec![prelude("echoer", echoer)]);
    assert_eq!(trace.text(), "echoed: hi");
    assert_eq!(
        records(&trace),
        vec![EffectRecord::ToolCall {
            tool: ToolId::parse("tools/echo").expect("a valid id"),
            alias: "tools_echo".to_owned(),
            args: json!({ "value": "hi" }),
            origin: ToolCallOrigin {
                execution: EXECUTION.to_owned(),
                section: "Only".to_owned(),
                caller: ToolCaller::Script,
            },
        }]
    );
}
