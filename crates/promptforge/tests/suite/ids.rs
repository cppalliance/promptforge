//! Grouping a log by task: provenance groups every effect and event under
//! its task the same way on every run, task ids compare as values, and a
//! task's start and end events say who started it and why it ended.

use std::collections::BTreeMap;
use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::event::Event;
use promptforge::ids::{AbandonReason, Provenance, TaskId, TaskOrigin};
use promptforge::model::{
    Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode, ToolCall,
};
use promptforge::timestamp::Timestamp;
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};

/// Starts two tasks on `## Reply`, waits for both, and joins their replies.
const GREETER: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Greets through tasks.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "local first = tasks.spawn('## Reply', { input = 'hello' })\n",
    "local second = tasks.spawn('## Reply', { input = 'bye' })\n",
    "local results = tasks.join({ first, second })\n",
    "return results[1].result .. ' ' .. results[2].result\n",
    "```\n\n",
    "## Reply\n\n",
    "```lua\n",
    "models.use('writer')\n",
    "return models.infer(args)\n",
    "```\n",
);

/// Lets its model start tasks on `## Wait`, runs one model loop, and
/// returns without waiting for them.
const LOOPER: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Greets through tasks.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "models.use('writer')\n",
    "tools.allow_tasks({ '## Wait' })\n",
    "local msgs = messages.new()\n",
    "msgs:user('hello')\n",
    "models.loop(msgs)\n",
    "return 'done'\n",
    "```\n\n",
    "## Wait\n\n",
    "```lua\n",
    "return store.read('note.md')\n",
    "```\n",
);

/// Parses `source` and prepares a run of it whose events number on from
/// its parse events, returned beside it.
fn start(source: &str) -> Result<(Run, Vec<Event>), Box<dyn Error>> {
    let (parsed, parse_events) = Prompt::parse(source, "greeter");
    let prompt = parsed?;
    let window = NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
    let model = ModelDescriptor::new(
        ModelId::gateway("canned")?,
        "Always replies hi there",
        window,
        ThinkingMode::Never,
    );
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH)
        .provenance_start(u32::try_from(parse_events.len())?)
        .model(model);
    let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
    if let Some(refusal) = requirements.refusal() {
        return Err(refusal.into());
    }
    Ok((Run::new(Arc::new(prompt), "", ctx), parse_events))
}

/// Drives `run` to its result, holding each effect `answer` returns `None`
/// for until the run has decided, and returns the result, the run's
/// events, and every effect's provenance.
fn drive(
    mut run: Run,
    mut answer: impl FnMut(Effect) -> Option<EffectAnswer>,
) -> (RunResult, Vec<Event>, Vec<Provenance>) {
    let (mut events, mut effects, mut held) = (Vec::new(), Vec::new(), Vec::new());
    loop {
        match run.step() {
            Step::Pending {
                effects: batch,
                events: reported,
            } => {
                assert!(
                    !batch.is_empty() || run.decided(),
                    "the run waits on an effect this test driver holds"
                );
                events.extend(reported);
                for (id, provenance, effect) in batch {
                    effects.push(provenance);
                    match answer(effect) {
                        Some(reply) => run.resume(id, reply),
                        None => held.push(id),
                    }
                }
                if run.decided() {
                    for id in held.drain(..) {
                        run.resume(id, EffectAnswer::Dropped);
                    }
                }
            }
            Step::Done {
                result,
                events: reported,
            } => {
                events.extend(reported);
                return (result, events, effects);
            }
        }
    }
}

fn reply(result: CompletionResult) -> EffectAnswer {
    EffectAnswer::Chat(Completion::from_result(result, "canned").map(Box::new))
}

/// Every effect and event of one run, grouped by task, each group in
/// provenance order.
type Groups = BTreeMap<TaskId, Vec<Provenance>>;

/// Runs [`GREETER`], answering every model round with `hi there` and
/// dropping everything else, and returns the started tasks beside the
/// run's groups.
fn run_greeter() -> Result<(Vec<TaskId>, Groups), Box<dyn Error>> {
    let (run, parse_events) = start(GREETER)?;
    let (result, run_events, effects) = drive(run, |effect| {
        Some(match effect {
            Effect::Chat { .. } => reply(CompletionResult::Text("hi there".to_owned())),
            _ => EffectAnswer::Dropped,
        })
    });
    assert!(matches!(result, RunResult::Ok(text) if text == "hi there hi there"));
    let events: Vec<Event> = parse_events.into_iter().chain(run_events).collect();
    let mut groups = Groups::new();
    let provenances = events.iter().map(|event| event.provenance().clone());
    for provenance in effects.into_iter().chain(provenances) {
        groups
            .entry(provenance.task.clone())
            .or_default()
            .push(provenance);
    }
    for records in groups.values_mut() {
        records.sort();
    }
    let started = events
        .iter()
        .filter_map(|event| match event {
            Event::TaskStarted { task, .. } => Some(task.clone()),
            _ => None,
        })
        .collect();
    Ok((started, groups))
}

#[test]
fn provenance_groups_a_log_by_task_in_start_order_and_the_same_on_every_run()
-> Result<(), Box<dyn Error>> {
    let (started, groups) = run_greeter()?;
    assert_eq!(groups.len(), 3);
    let mut sorted = started.clone();
    sorted.sort();
    assert_eq!(sorted, started);
    assert_eq!(
        started,
        ["0.0".parse::<TaskId>()?, "0.1".parse::<TaskId>()?]
    );
    assert_eq!(run_greeter()?.1, groups);
    Ok(())
}

#[test]
fn parsed_task_ids_compare_as_values_and_a_damaged_id_reports_its_whole_text()
-> Result<(), Box<dyn Error>> {
    assert_eq!("0.01".parse::<TaskId>()?, "0.1".parse::<TaskId>()?);
    let error = "0..1"
        .parse::<TaskId>()
        .err()
        .ok_or("0..1 is not a task id")?;
    assert_eq!(error.input(), "0..1");
    Ok(())
}

#[test]
fn a_model_started_task_is_abandoned_when_its_owning_section_returns() -> Result<(), Box<dyn Error>>
{
    let call = ToolCall::from_parts("call_1", "task", serde_json::json!({ "target": "## Wait" }))?;
    let mut replies = vec![
        CompletionResult::Text("bye".to_owned()),
        CompletionResult::ToolCalls(vec![call]),
    ];
    let (run, _parse_events) = start(LOOPER)?;
    let (result, events, _effects) = drive(run, |effect| match effect {
        Effect::Chat { .. } => replies.pop().map(reply),
        _ => None,
    });
    assert!(matches!(result, RunResult::Ok(text) if text == "done"));

    let (task, origin) = events
        .iter()
        .find_map(|event| match event {
            Event::TaskStarted { task, origin, .. } => Some((task, *origin)),
            _ => None,
        })
        .ok_or("the model starts a task")?;
    let end = events
        .iter()
        .find(|event| {
            matches!(event,
                Event::TaskSucceeded { task: t, .. } | Event::TaskFailed { task: t, .. }
                    | Event::TaskCancelled { task: t, .. } | Event::TaskAbandoned { task: t, .. } if t == task
            )
        })
        .ok_or("the task ends")?;
    let Event::TaskAbandoned { reason, .. } = end else {
        return Err(format!("the task ended as {end:?}").into());
    };

    assert_eq!((origin, origin.tag()), (TaskOrigin::Model, "model"));
    assert_eq!(*reason, AbandonReason::OwnerReturned);
    assert_eq!(reason.why(), "the section ended");
    Ok(())
}
