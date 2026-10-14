//! Grouping a log by task, and comparing task ids.

use std::collections::BTreeMap;

use promptforge::event::Event;
use promptforge::ids::{Provenance, TaskId};

use super::support::{Scenario, TestResult, prompt, text};

const GREET: &str = "
    local first = tasks.spawn('## Reply', { input = 'hello' })
    local second = tasks.spawn('## Reply', { input = 'bye' })
    local results = tasks.join({ first, second })
    return results[1].result .. ' ' .. results[2].result
";

const REPLY: &str = "
    models.use('writer')
    return models.infer(args)
";

type Groups = BTreeMap<TaskId, Vec<Provenance>>;

/// Returns the started tasks beside the run's groups.
fn run_greeter() -> TestResult<(Vec<TaskId>, Groups)> {
    let source = prompt(
        "models: { writer: {} }",
        &[("Greet", GREET), ("Reply", REPLY)],
    );
    let trace = Scenario::replies([text("hi there"), text("hi there")]).run(&source)?;
    assert_eq!(trace.text()?, "hi there hi there");
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
