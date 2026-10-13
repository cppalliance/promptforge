//! Naming Plugins and adding their Lua: a Plugin id holds exactly
//! its own tools, a prelude on the context defines a function every
//! section can call, and each run's Lua sees its own context's roster.

use std::error::Error;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::plugins::{PluginId, PluginIdErrorKind, Prelude};
use promptforge::timestamp::Timestamp;
use promptforge::tools::ToolId;
use promptforge::vfs::perform_vfs_op;
use promptforge::{Prompt, Run, RunContext, RunResult, Step, UnavailablePlugin};

#[test]
fn a_plugin_holds_its_own_tools_and_a_two_part_name_is_not_one() -> Result<(), Box<dyn Error>> {
    let web = PluginId::parse("web")?;
    let fetch = ToolId::parse("web/fetch")?;
    let other = ToolId::parse("other/fetch")?;
    assert!(web.contains(&fetch));
    assert!(!web.contains(&other));

    let error = PluginId::parse("promptforge/web")
        .err()
        .ok_or("a two-part name is not a Plugin id")?;
    assert_eq!(error.kind(), PluginIdErrorKind::SegmentCount);
    Ok(())
}

#[test]
fn a_prelude_on_the_context_defines_a_function_the_section_calls() -> Result<(), Box<dyn Error>> {
    let source = concat!(
        "---\n",
        "name: greeter\n",
        "description: Greets through a prelude helper and reads the greeting back.\n",
        "promptforge: 0\n",
        "---\n\n",
        "# Greeter\n\n## Greet\n\n",
        "```lua\n",
        "store.write('note.md', greet('world'))\n",
        "return store.read('note.md')\n",
        "```\n",
    );
    let (parsed, _parse_events) = Prompt::parse(source, "greeter");
    let prompt = Arc::new(parsed?);

    let web = PluginId::parse("web")?;
    let prelude = Prelude::new(web, "function greet(name) return 'hello ' .. name end");
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).preludes(vec![prelude]);
    let (ctx, _requirements) = ctx.prepare(&prompt);
    let result = run_to_end(Run::new(prompt, "", ctx))?;
    assert!(matches!(result, RunResult::Ok(text) if text == "hello world"));
    Ok(())
}

/// Steps `run` to its end, answering each store effect from the run's
/// store, and fails on any other effect.
fn run_to_end(mut run: Run) -> Result<RunResult, Box<dyn Error>> {
    loop {
        let effects = match run.step() {
            Step::Pending { effects, .. } => effects,
            Step::Done { result, .. } => return Ok(result),
        };
        for (id, _provenance, effect) in effects {
            let Effect::Vfs { access, op } = effect else {
                return Err("these prompts issue only store effects".into());
            };
            run.resume(id, EffectAnswer::Vfs(perform_vfs_op(&access, op)));
        }
    }
}

#[test]
fn two_runs_whose_contexts_carry_different_rosters_each_see_their_own() -> Result<(), Box<dyn Error>>
{
    let source = concat!(
        "---\n",
        "name: roster\n",
        "description: Reports whether the run's Lua has an object for kit.\n",
        "promptforge: 0\n",
        "---\n\n",
        "# Roster\n\n## Look\n\n",
        "```lua\n",
        "return tostring(plugins.get('kit') ~= nil)\n",
        "```\n",
    );
    let (parsed, _parse_events) = Prompt::parse(source, "roster");
    let prompt = Arc::new(parsed?);
    let kit = PluginId::parse("kit")?;

    let usable =
        RunContext::new("roster", 7, Timestamp::UNIX_EPOCH).plugins(vec![kit.clone()], vec![]);
    let unavailable = RunContext::new("roster", 7, Timestamp::UNIX_EPOCH)
        .plugins(vec![], vec![UnavailablePlugin::new(kit, "down")]);

    for (ctx, expected) in [(usable, "true"), (unavailable, "false")] {
        let (ctx, _requirements) = ctx.prepare(&prompt);
        let result = run_to_end(Run::new(Arc::clone(&prompt), "", ctx))?;
        assert!(
            matches!(&result, RunResult::Ok(text) if text == expected),
            "expected {expected}, got {result:?}"
        );
    }
    Ok(())
}
