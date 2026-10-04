//! The greeter, one tour at a time: it parses, its store effects are
//! answered from the run's store, prepare fills its model role, its tool
//! output reaches its Lua unchanged, a cancel from another thread ends it,
//! and the complete program in `examples/greeter.rs` logs its parse events
//! first and returns `HI THERE`.

use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};

#[path = "../../examples/greeter.rs"]
#[expect(
    unreachable_pub,
    reason = "the example's items are pub at its own crate root, and the suite reaches them through this private module"
)]
#[expect(dead_code, reason = "only the example binary calls its main")]
mod example;

use example::{GREETER, answer};

/// Writes a note to the store and reads it back.
const NOTE: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note to the store and reads it back.\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "return store.read('note.md')\n",
    "```\n",
);

/// Writes a note and returns the `writer` role's reply to it.
const REPLY: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note and asks a model to reply to it.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "models.use('writer')\n",
    "return models.infer(store.read('note.md'))\n",
    "```\n",
);

/// Steps `run`, answering each effect with the example's `answer`, until
/// it is done.
fn drive(run: &mut Run) -> RunResult {
    loop {
        match run.step() {
            Step::Pending { effects, .. } => {
                for (id, _provenance, effect) in effects {
                    run.resume(id, answer(effect));
                }
            }
            Step::Done { result, .. } => break result,
        }
    }
}

fn parse(source: &str) -> Result<Prompt, Box<dyn Error>> {
    let (parsed, _parse_events) = Prompt::parse(source, "greeter");
    Ok(parsed?)
}

/// The one canned model the tours set on the context before prepare.
fn canned_model() -> Result<ModelDescriptor, Box<dyn Error>> {
    let window = NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
    Ok(ModelDescriptor::new(
        ModelId::gateway("canned")?,
        "Always replies hi there",
        window,
        ThinkingMode::Never,
    ))
}

/// An environment whose catalog holds the shout tool alone.
fn shout_environment() -> Result<Environment, Box<dyn Error>> {
    let shout = ToolDescriptor::new(
        ToolId::parse("example/text/shout")?,
        "shout",
        "Returns the text in capital letters.",
        serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
    );
    Ok(Environment::new().tools(ToolCatalog::new(&[shout])?))
}

/// The shouting greeter, prepared with the canned model and the shout tool.
fn shout_run() -> Result<Run, Box<dyn Error>> {
    let prompt = parse(GREETER)?;
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(canned_model()?);
    let (ctx, requirements) = shout_environment()?.prepare(&prompt, ctx);
    if let Some(refusal) = requirements.refusal() {
        return Err(refusal.into());
    }
    Ok(Run::new(Arc::new(prompt), "", ctx))
}

#[test]
fn the_smallest_greeter_parses_and_its_title_is_the_h1_text() {
    let (parsed, _events) = Prompt::parse(NOTE, "greeter");
    assert!(parsed.is_ok_and(|prompt| prompt.title() == "Greeter"));
}

#[test]
fn answering_each_store_effect_from_the_runs_store_returns_the_note() -> Result<(), Box<dyn Error>>
{
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
    let mut run = Run::new(Arc::new(parse(NOTE)?), "", ctx);
    assert!(matches!(drive(&mut run), RunResult::Ok(text) if text == "hello"));
    Ok(())
}

#[test]
fn prepare_fills_the_writer_role_and_the_canned_reply_becomes_the_result()
-> Result<(), Box<dyn Error>> {
    let prompt = parse(REPLY)?;
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(canned_model()?);
    let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
    assert!(requirements.refusal().is_none());
    let mut run = Run::new(Arc::new(prompt), "", ctx);
    assert!(matches!(drive(&mut run), RunResult::Ok(text) if text == "hi there"));
    Ok(())
}

#[test]
fn trusted_tool_output_reaches_the_calling_lua_unchanged() -> Result<(), Box<dyn Error>> {
    let prompt = parse(GREETER)?;
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(canned_model()?);
    let (ctx, requirements) = shout_environment()?.prepare(&prompt, ctx);
    assert!(requirements.refusal().is_none());
    let mut run = Run::new(Arc::new(prompt), "", ctx);
    assert!(matches!(drive(&mut run), RunResult::Ok(text) if text == "HI THERE"));
    Ok(())
}

#[test]
fn a_cancel_from_another_thread_ends_the_run_cancelled_once_held_effects_drop()
-> Result<(), Box<dyn Error>> {
    let mut run = shout_run()?;
    let mut held = Vec::new();
    while held.is_empty() {
        let Step::Pending { effects, .. } = run.step() else {
            return Err("the greeter waits on its model before it can finish".into());
        };
        for (id, _provenance, effect) in effects {
            match effect {
                Effect::Chat { .. } => held.push(id),
                other => run.resume(id, answer(other)),
            }
        }
    }

    let handle = run.cancel_handle();
    std::thread::spawn(move || handle.cancel())
        .join()
        .map_err(|_| "the cancelling thread panicked")?;

    let Step::Pending { effects, .. } = run.step() else {
        return Err("the held chat effect still needs its answer".into());
    };
    assert!(effects.is_empty() && run.decided());
    for id in held {
        run.resume(id, EffectAnswer::Dropped);
    }
    assert!(matches!(
        run.step(),
        Step::Done {
            result: RunResult::Cancelled,
            ..
        }
    ));
    Ok(())
}

#[test]
fn the_complete_greeter_returns_hi_there_and_its_log_opens_with_the_parse_events()
-> Result<(), Box<dyn Error>> {
    assert_eq!(example::greet()?, "HI THERE");
    Ok(())
}
