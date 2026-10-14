//! Naming Plugins and adding their Lua: a Plugin id holds exactly
//! its own tools, a prelude on the context defines a function every
//! section can call, and each run's Lua sees its own context's roster.

use promptforge::plugins::{PluginId, PluginIdErrorKind, Prelude};
use promptforge::tools::ToolId;
use promptforge::{RunResult, UnavailablePlugin};

use super::support::{Scenario, TestResult, greeter};

#[test]
fn a_plugin_holds_its_own_tools_and_a_two_part_name_is_not_one() -> TestResult {
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
fn a_prelude_on_the_context_defines_a_function_the_section_calls() -> TestResult {
    let source = greeter(
        "
        store.write('note.md', greet('world'))
        return store.read('note.md')
    ",
    );
    let prelude = Prelude::new(
        PluginId::parse("web")?,
        "function greet(name) return 'hello ' .. name end",
    );
    let scenario = Scenario::default().context(move |ctx| ctx.preludes(vec![prelude.clone()]));
    let trace = scenario.run(&source)?;
    assert!(matches!(trace.result, RunResult::Ok(text) if text == "hello world"));
    Ok(())
}

#[test]
fn two_runs_whose_contexts_carry_different_rosters_each_see_their_own() -> TestResult {
    let source = greeter("return tostring(plugins.get('kit') ~= nil)");
    let kit = PluginId::parse("kit")?;
    let usable = {
        let kit = kit.clone();
        Scenario::default().context(move |ctx| ctx.plugins(vec![kit.clone()], vec![]))
    };
    let unavailable = Scenario::default()
        .context(move |ctx| ctx.plugins(vec![], vec![UnavailablePlugin::new(kit.clone(), "down")]));

    for (scenario, expected) in [(usable, "true"), (unavailable, "false")] {
        let result = scenario.run(&source)?.result;
        assert!(
            matches!(&result, RunResult::Ok(text) if text == expected),
            "expected {expected}, got {result:?}"
        );
    }
    Ok(())
}
