//! Logging a run's events: every event round-trips through one JSON line
//! with a unique provenance, content events pair into a transcript, and
//! debug mode alone adds the raw model request and response.

use std::collections::HashSet;
use std::error::Error;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::event::{DebugMode, Event};
use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};
use promptforge::vfs::perform_vfs_op;
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};

/// Writes `hello` and the run's argument to a note and reads it back.
const NOTE: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note to the store and reads it back.\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello ' .. args)\n",
    "return store.read('note.md')\n",
    "```\n",
);

/// Asks the `writer` role to reply to the note, and shouts the reply.
const SHOUT: &str = concat!(
    "---\nname: greeter\ndescription: Replies to a note and shouts the reply.\npromptforge: 0\n",
    "models:\n  writer: {}\ntools:\n  shout: example/text/shout\n---\n\n# Greeter\n\n## Greet\n\n",
    "```lua\nstore.write('note.md', 'hello ' .. args)\nmodels.use('writer')\n",
    "return tools.call('shout', { text = models.infer(store.read('note.md')) })\n```\n",
);

/// Answers the store from the run's store, the model with `hi there`, and
/// the shout tool with `HI THERE`; no greeter here waits on a timer.
fn answer(effect: Effect) -> EffectAnswer {
    match effect {
        Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
        Effect::Chat { .. } => EffectAnswer::Chat(
            Completion::from_result(CompletionResult::Text("hi there".to_owned()), "canned")
                .map(Box::new),
        ),
        Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
        Effect::Timer { .. } => EffectAnswer::Dropped,
    }
}

/// Drives `run` to its result, handing each step's events to `record`.
fn drive(mut run: Run, mut record: impl FnMut(Vec<Event>)) -> RunResult {
    loop {
        match run.step() {
            Step::Pending { effects, events } => {
                record(events);
                for (id, _provenance, effect) in effects {
                    run.resume(id, answer(effect));
                }
            }
            Step::Done { result, events } => {
                record(events);
                return result;
            }
        }
    }
}

/// The canned model and an environment that offers the shout tool.
fn shout_offer() -> Result<(ModelDescriptor, Environment), Box<dyn Error>> {
    let model = ModelDescriptor::new(
        ModelId::gateway("canned")?,
        "Replies hi there",
        8_192u32.try_into()?,
        ThinkingMode::Never,
    );
    let schema = serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}});
    let shout = ToolDescriptor::new(
        ToolId::parse("example/text/shout")?,
        "shout",
        "Shouts the text.",
        schema,
    );
    Ok((model, Environment::new().tools(ToolCatalog::new(&[shout])?)))
}

#[test]
fn every_event_round_trips_as_a_json_line_and_no_two_share_a_provenance()
-> Result<(), Box<dyn Error>> {
    let (parsed, parse_events) = Prompt::parse(NOTE, "greeter");
    let mut log: Vec<Event> = parse_events;
    let prompt = Arc::new(parsed?);

    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH)
        .provenance_start(u32::try_from(log.len())?);
    let result = drive(Run::new(Arc::clone(&prompt), "world", ctx), |events| {
        log.extend(events);
    });
    assert!(matches!(result, RunResult::Ok(text) if text == "hello world"));

    for event in &log {
        let line = serde_json::to_string(event)?;
        assert_eq!(&serde_json::from_str::<Event>(&line)?, event);
        let record: serde_json::Value = serde_json::from_str(&line)?;
        assert!(record["kind"].is_string(), "{line} names its kind");
    }

    let keys: HashSet<_> = log.iter().map(Event::provenance).collect();
    assert_eq!(keys.len(), log.len());
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
    let quiet = drive(Run::new(prompt, "world", ctx), drop);
    assert!(matches!(quiet, RunResult::Ok(text) if text == "hello world"));
    Ok(())
}

#[test]
fn content_events_pair_each_tool_result_with_its_caller_into_a_transcript()
-> Result<(), Box<dyn Error>> {
    let (parsed, _parse_events) = Prompt::parse(SHOUT, "greeter");
    let prompt = parsed?;
    let (model, environment) = shout_offer()?;
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
    let (ctx, requirements) = environment.prepare(&prompt, ctx);
    assert!(requirements.refusal().is_none());

    let mut log = Vec::new();
    let result = drive(Run::new(Arc::new(prompt), "world", ctx), |events| {
        log.extend(events);
    });
    assert!(matches!(result, RunResult::Ok(text) if text == "HI THERE"));

    let (mut asked, mut transcript) = (HashSet::new(), Vec::new());
    for event in &log {
        match event {
            Event::AssistantToolCalls { turn, calls, .. } => {
                asked.extend(calls.iter().map(|call| (*turn, call.id.clone())));
            }
            Event::AssistantReply { text, origin, .. } => {
                transcript.push(format!("{origin:?} reply: {text}"));
            }
            Event::ToolResult {
                turn,
                tool_call_id,
                alias,
                content,
                ..
            } => {
                let caller = if asked.contains(&(*turn, tool_call_id.clone())) {
                    "model"
                } else {
                    "script"
                };
                transcript.push(format!("{alias} for the {caller}: {content}"));
            }
            _ => {}
        }
    }
    assert_eq!(
        transcript,
        ["Infer reply: hi there", "shout for the script: HI THERE"]
    );
    Ok(())
}

#[test]
fn debug_mode_alone_adds_one_request_and_one_response_per_model_round() -> Result<(), Box<dyn Error>>
{
    let (parsed, _parse_events) = Prompt::parse(SHOUT, "greeter");
    let prompt = Arc::new(parsed?);
    let (model, environment) = shout_offer()?;

    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH)
        .model(model.clone())
        .report_debug(DebugMode::On);
    let (ctx, _requirements) = environment.prepare(&prompt, ctx);
    let mut log = Vec::new();
    let captured = drive(Run::new(Arc::clone(&prompt), "world", ctx), |events| {
        log.extend(events);
    });

    let (mut requests, mut responses) = (Vec::new(), Vec::new());
    for event in &log {
        match event {
            Event::Request { turn, .. } => requests.push(*turn),
            Event::Response { turn, .. } => responses.push(*turn),
            _ => {}
        }
    }
    assert_eq!(requests.len(), 1);
    assert_eq!(requests, responses);

    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
    let (ctx, _requirements) = environment.prepare(&prompt, ctx);
    let mut quiet_log = Vec::new();
    let quiet = drive(Run::new(prompt, "world", ctx), |events| {
        quiet_log.extend(events);
    });
    assert!(
        !quiet_log
            .iter()
            .any(|event| matches!(event, Event::Request { .. } | Event::Response { .. }))
    );
    assert!(
        matches!((captured, quiet), (RunResult::Ok(on), RunResult::Ok(off)) if on == off && on == "HI THERE")
    );
    Ok(())
}
