//! Naming Plugins and adding their Lua: a Plugin id holds exactly
//! its own tools, and a prelude on the environment defines a function
//! every section can call.

use std::error::Error;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::plugins::{PluginId, PluginIdErrorKind, Prelude};
use promptforge::timestamp::Timestamp;
use promptforge::tools::ToolId;
use promptforge::vfs::perform_vfs_op;
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};

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
fn a_prelude_on_the_environment_defines_a_function_the_section_calls() -> Result<(), Box<dyn Error>>
{
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
    let env = Environment::new().preludes(vec![prelude]);
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
    let (ctx, _requirements) = env.prepare(&prompt, ctx);
    let mut run = Run::new(prompt, "", ctx);

    let result = loop {
        let effects = match run.step() {
            Step::Pending { effects, .. } => effects,
            Step::Done { result, .. } => break result,
        };
        for (id, _provenance, effect) in effects {
            let Effect::Vfs { access, op } = effect else {
                return Err("the greeter issues only store effects".into());
            };
            run.resume(id, EffectAnswer::Vfs(perform_vfs_op(&access, op)));
        }
    };
    assert!(matches!(result, RunResult::Ok(text) if text == "hello world"));
    Ok(())
}
