//! Reading what a prompt declares before it runs: its frontmatter lists
//! every declaration with arguments sorted by name, argument text reaches
//! Lua as JSON or as plain prose, and a role check by hand agrees with
//! prepare.

use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge::prompt::{ModelKeyword, ToolSlot};
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_vfs_op;
use promptforge::{
    Environment, ParseErrorKind, Prompt, RequirementCheck, Run, RunContext, RunResult, Step,
};

/// Declares two files, one Plugin, one tool slot, `times` then `name`,
/// and one model role.
const CONTRACT: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Greets someone by name.\n",
    "promptforge: 0\n",
    "input: { path: names.md, description: The names to greet }\n",
    "output: { path: note.md, description: The note the greeter leaves }\n",
    "plugins: [text]\n",
    "tools: { shout: text/shout }\n",
    "args:\n",
    "  times: { type: integer, optional: true, default: 1 }\n",
    "  name: { type: string }\n",
    "models:\n",
    "  writer: { keywords: [fast] }\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello ' .. argv.name)\n",
    "return store.read('note.md')\n",
    "```\n",
);

/// Runs `prompt` with the argument `text`, answering its store effects.
fn greet(prompt: Prompt, text: &str) -> Result<RunResult, Box<dyn Error>> {
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
    let mut run = Run::new(Arc::new(prompt), text, ctx);
    loop {
        match run.step() {
            Step::Pending { effects, .. } => {
                for (id, _provenance, effect) in effects {
                    let Effect::Vfs { access, op } = effect else {
                        return Err("the greeter only uses the store".into());
                    };
                    run.resume(id, EffectAnswer::Vfs(perform_vfs_op(&access, op)));
                }
            }
            Step::Done { result, .. } => return Ok(result),
        }
    }
}

#[test]
fn the_frontmatter_lists_every_declaration_with_arguments_sorted_by_name()
-> Result<(), Box<dyn Error>> {
    let prompt = Prompt::parse(CONTRACT, "greeter").0?;
    let frontmatter = prompt.frontmatter();
    assert_eq!(frontmatter.name(), "greeter");
    assert_eq!(frontmatter.description(), "Greets someone by name.");

    let files: Vec<_> = [frontmatter.input(), frontmatter.output()]
        .into_iter()
        .flatten()
        .map(|file| (file.path().to_string(), file.description().to_owned()))
        .collect();
    assert_eq!(
        files,
        [
            ("names.md".to_owned(), "The names to greet".to_owned()),
            (
                "note.md".to_owned(),
                "The note the greeter leaves".to_owned()
            ),
        ]
    );
    let plugins: Vec<_> = frontmatter
        .plugins()
        .iter()
        .map(|plugin| (plugin.id().to_string(), plugin.is_optional()))
        .collect();
    assert_eq!(plugins, [("text".to_owned(), false)]);
    let slots: Vec<_> = frontmatter
        .tools()
        .iter()
        .filter_map(|(alias, slot)| match slot {
            ToolSlot::Exact(id) => Some((alias.to_string(), id.to_string())),
            _ => None,
        })
        .collect();
    assert_eq!(slots, [("shout".to_owned(), "text/shout".to_owned())]);
    let roles: Vec<_> = frontmatter
        .models()
        .iter()
        .map(|(label, role)| (label.to_string(), role.min_context()))
        .collect();
    assert_eq!(roles, [("writer".to_owned(), None)]);

    assert!(
        frontmatter
            .args()
            .iter()
            .map(|(name, _)| name)
            .eq(["name", "times"])
    );

    let misspelled = CONTRACT.replace("plugins:", "plugns:");
    let parsed = Prompt::parse(&misspelled, "greeter").0;
    assert_eq!(
        parsed.err().map(|error| error.kind()),
        Some(ParseErrorKind::Frontmatter)
    );
    Ok(())
}

#[test]
fn declared_arguments_take_json_and_an_undeclared_prompt_takes_plain_prose()
-> Result<(), Box<dyn Error>> {
    let declared = concat!(
        "---\n",
        "name: greeter\n",
        "description: Greets someone by name.\n",
        "promptforge: 0\n",
        "args:\n",
        "  times: { type: integer, optional: true, default: 1 }\n",
        "  name: { type: string }\n",
        "---\n\n",
        "# Greeter\n\n",
        "## Greet\n\n",
        "```lua\n",
        "store.write('note.md', 'hello ' .. argv.name)\n",
        "return store.read('note.md')\n",
        "```\n",
    );
    let plain = concat!(
        "---\n",
        "name: greeter\n",
        "description: Greets someone by name.\n",
        "promptforge: 0\n",
        "---\n\n",
        "# Greeter\n\n",
        "## Greet\n\n",
        "```lua\n",
        "store.write('note.md', 'hello ' .. argv.prose)\n",
        "return store.read('note.md')\n",
        "```\n",
    );
    let declared = Prompt::parse(declared, "greeter").0?;
    let plain = Prompt::parse(plain, "greeter").0?;
    assert!(!declared.frontmatter().args().is_default());
    assert!(plain.frontmatter().args().is_default());

    let json = serde_json::json!({ "name": "world" }).to_string();
    assert!(matches!(greet(declared, &json)?, RunResult::Ok(text) if text == "hello world"));
    assert!(matches!(greet(plain, "world")?, RunResult::Ok(text) if text == "hello world"));
    Ok(())
}

#[test]
fn a_role_check_by_hand_finds_the_same_shortfall_prepare_reports() -> Result<(), Box<dyn Error>> {
    let source = concat!(
        "---\n",
        "name: greeter\n",
        "description: Greets someone by name.\n",
        "promptforge: 0\n",
        "models:\n",
        "  writer: { keywords: [fast] }\n",
        "  checker: { keywords: [no-thinking], min_context: 100000 }\n",
        "---\n\n",
        "# Greeter\n\n",
        "## Greet\n\n",
        "```lua\n",
        "store.write('note.md', 'hello ' .. argv.name)\n",
        "return store.read('note.md')\n",
        "```\n",
    );
    let prompt = Prompt::parse(source, "greeter").0?;
    let window = NonZeroU32::new(200_000).ok_or("a context window is never zero")?;
    let id = ModelId::gateway("canned")?;
    let model = ModelDescriptor::new(id, "Thinks on every call", window, ThinkingMode::Always);

    let mut misses = Vec::new();
    for (label, role) in prompt.frontmatter().models().iter() {
        let keyword_miss = role.keywords().iter().any(|keyword| match keyword {
            ModelKeyword::Thinking => model.thinking() == ThinkingMode::Never,
            ModelKeyword::NoThinking => model.thinking() == ThinkingMode::Always,
            _ => false,
        });
        let context_miss = role.min_context().is_some_and(|min| min > model.context());
        if keyword_miss || context_miss {
            misses.push((label, keyword_miss, context_miss));
        }
    }
    assert_eq!(misses, [("checker", true, false)]);

    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
    let (_ctx, requirements) = Environment::new().prepare(&prompt, ctx);
    let [unmet] = requirements.unmet_requirements.as_slice() else {
        return Err("only the checker falls short".into());
    };
    assert_eq!(unmet.role, "checker");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "no-thinking");
    Ok(())
}
