//! Grouping a log by task: provenance groups every effect and event under
//! its task the same way on every run, task ids compare as values, and a
//! task's start and end events say who started it and why it ended.

use std::collections::BTreeMap;

use promptforge::RunResult;
use promptforge::effect::Effect;
use promptforge::event::Event;
use promptforge::ids::{AbandonReason, Provenance, TaskId, TaskOrigin};
use serde_json::json;

use super::support::{Scenario, TestResult, calls, prompt, text};

/// Starts two tasks on `## Reply`, waits for both, and joins their replies.
const GREET: &str = "
    local first = tasks.spawn('## Reply', { input = 'hello' })
    local second = tasks.spawn('## Reply', { input = 'bye' })
    local results = tasks.join({ first, second })
    return results[1].result .. ' ' .. results[2].result
";

/// Replies to its input with the `writer` role.
const REPLY: &str = "
    models.use('writer')
    return models.infer(args)
";

/// Lets its model start tasks on `## Wait`, runs one model loop, and
/// returns without waiting for them.
const LOOPER: &str = "
    models.use('writer')
    tools.allow_tasks({ '## Wait' })
    local msgs = messages.new()
    msgs:user('hello')
    models.loop(msgs)
    return 'done'
";

/// Every effect and event of one run, grouped by task, each group in
/// provenance order.
type Groups = BTreeMap<TaskId, Vec<Provenance>>;

/// Runs the two-task greeter, answering every model round with `hi there`,
/// and returns the started tasks beside the run's groups.
fn run_greeter() -> TestResult<(Vec<TaskId>, Groups)> {
    let source = prompt(
        "models: { writer: {} }",
        &[("Greet", GREET), ("Reply", REPLY)],
    );
    let trace = Scenario::replies([text("hi there"), text("hi there")]).run(&source)?;
    assert!(matches!(trace.result, RunResult::Ok(text) if text == "hi there hi there"));
    let mut groups = Groups::new();
    let effects = trace.effects.iter().map(|logged| logged.provenance.clone());
    let events = trace.events.iter().map(|event| event.provenance().clone());
    for provenance in effects.chain(events) {
        groups
            .entry(provenance.task.clone())
            .or_default()
            .push(provenance);
    }
    for records in groups.values_mut() {
        records.sort();
    }
    let started = trace
        .events
        .iter()
        .filter_map(|event| match event {
            Event::TaskStarted { task, .. } => Some(task.clone()),
            _ => None,
        })
        .collect();
    Ok((started, groups))
}

#[test]
fn provenance_groups_a_log_by_task_in_start_order_and_the_same_on_every_run() -> TestResult {
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
fn parsed_task_ids_compare_as_values_and_a_damaged_id_reports_its_whole_text() -> TestResult {
    assert_eq!("0.01".parse::<TaskId>()?, "0.1".parse::<TaskId>()?);
    let error = "0..1"
        .parse::<TaskId>()
        .err()
        .ok_or("0..1 is not a task id")?;
    assert_eq!(error.input(), "0..1");
    Ok(())
}

#[test]
fn a_model_started_task_is_abandoned_when_its_owning_section_returns() -> TestResult {
    let call = calls("task", json!({ "target": "## Wait" }))?;
    let source = prompt(
        "models: { writer: {} }",
        &[("Greet", LOOPER), ("Wait", "return store.read('note.md')")],
    );
    let trace = Scenario::replies([call, text("bye")])
        .hold(|effect| !matches!(effect, Effect::Chat { .. }))
        .run(&source)?;
    assert!(matches!(trace.result, RunResult::Ok(text) if text == "done"));

    let (task, origin) = trace
        .events
        .iter()
        .find_map(|event| match event {
            Event::TaskStarted { task, origin, .. } => Some((task, *origin)),
            _ => None,
        })
        .ok_or("the model starts a task")?;
    let end = trace
        .events
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
