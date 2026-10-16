//! Handing Plugins to a run.

use runvil::UnavailablePlugin;
use runvil::plugins::PluginId;

use super::support::{Scenario, TestResult, greeter};

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
        assert_eq!(scenario.run(&source)?.text()?, expected);
    }
    Ok(())
}
