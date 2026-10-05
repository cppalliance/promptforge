//! The greeter: one Harness loop that parses a prompt, prepares it with a
//! canned model and a shout tool, steps the run, answers every effect it
//! hands back, logs every event, and prints the result.
//!
//! The facade suite includes this file as a module and runs `greet`, so
//! the items it reaches are `pub`.

use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::event::Event;
use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};
use promptforge::vfs::perform_vfs_op;
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};

/// Writes a note, asks the `writer` role to reply, and shouts the reply.
pub const GREETER: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note, asks a model to reply, and shouts the reply.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "tools:\n",
    "  shout: example/text/shout\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "models.use('writer')\n",
    "local reply = models.infer(store.read('note.md'))\n",
    "return tools.call('shout', { text = reply })\n",
    "```\n",
);

/// Answers a store effect from the run's store, the model with `hi there`,
/// and the shout tool with the trusted `HI THERE`; the greeter never waits
/// on a timer.
#[must_use]
pub fn answer(effect: Effect) -> EffectAnswer {
    match effect {
        Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
        Effect::Chat { .. } => {
            let reply = CompletionResult::Text("hi there".to_owned());
            EffectAnswer::Chat(Completion::from_result(reply, "canned").map(Box::new))
        }
        Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
        Effect::Timer { .. } => EffectAnswer::Dropped,
    }
}

/// Runs the greeter to its end and returns its text.
///
/// # Errors
///
/// Returns an error when the greeter fails to parse, prepare refuses it,
/// or the run ends without text.
///
/// # Panics
///
/// Panics when the log does not open with the parse events, or when the
/// run was cancelled.
pub fn greet() -> Result<String, Box<dyn Error>> {
    // 1. Parse the greeter, and start the log with its parse events before
    //    checking the result.
    let (parsed, parse_events) = Prompt::parse(GREETER, "greeter");
    let mut log: Vec<Event> = parse_events.clone();
    let prompt = parsed?;

    // 2. Build the context: run events number on from the parse events, and
    //    the model is set.
    let model = ModelDescriptor::new(
        ModelId::gateway("canned")?,
        "Always replies hi there",
        NonZeroU32::new(8_192).ok_or("a context window is never zero")?,
        ThinkingMode::Never,
    );
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH)
        .provenance_start(u32::try_from(parse_events.len())?)
        .model(model);

    // 3. Offer the shout tool, prepare, and refuse to run when prepare
    //    reports a gap.
    let shout = ToolDescriptor::new(
        ToolId::parse("example/text/shout")?,
        "shout",
        "Returns the text in capital letters.",
        serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
    );
    let environment = Environment::new().tools(ToolCatalog::new(&[shout])?);
    let (ctx, requirements) = environment.prepare(&prompt, ctx);
    if let Some(refusal) = requirements.refusal() {
        return Err(refusal.into());
    }

    // 4. Create the run, and keep a cancel handle for anything that may
    //    need to stop it.
    let mut run = Run::new(Arc::new(prompt), "", ctx);
    let cancel = run.cancel_handle();

    // 5. Step until Done, logging each step's events and answering each
    //    effect exactly once.
    let result = loop {
        match run.step() {
            Step::Pending { effects, events } => {
                log.extend(events);
                for (id, _provenance, effect) in effects {
                    if run.decided() {
                        run.resume(id, EffectAnswer::Dropped);
                    } else {
                        run.resume(id, answer(effect));
                    }
                }
            }
            Step::Done { result, events } => {
                log.extend(events);
                break result;
            }
        }
    };

    // 6. The tool's output is the result, and the log opens with the parse
    //    events.
    assert!(log.starts_with(&parse_events));
    assert!(!cancel.is_cancelled());
    let RunResult::Ok(text) = result else {
        return Err(format!("the greeter did not finish: {result:?}").into());
    };
    Ok(text)
}

fn main() -> Result<(), Box<dyn Error>> {
    let text = greet()?;
    assert_eq!(text, "HI THERE");
    println!("{text}");
    Ok(())
}
