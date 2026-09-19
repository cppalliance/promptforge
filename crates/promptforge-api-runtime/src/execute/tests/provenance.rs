//! Events as values: every report the engine makes is an [`Event`] pushed
//! into the run-level buffer, stamped with a [`Provenance`] - the nearest
//! enclosing task and a per-task sequence number. The main walk is task
//! `0`; a `call` child reports under its caller's task; a spawned task (a
//! fanout arm included) is its own task, and its counter starts at zero
//! independently of every other task's.

use std::collections::BTreeMap;

use promptforge_api_types::event::Event;
use promptforge_api_types::ids::TaskId;

use super::scheduler::scheduler_context_on;
use super::*;
use crate::execute::scheduler::Scheduler;

fn task(id: &str) -> TaskId {
    id.parse().expect("a task id parses")
}

/// The `seq` of every event, grouped by task in emission order.
fn seqs_by_task(events: &[Event]) -> BTreeMap<TaskId, Vec<u32>> {
    let mut grouped: BTreeMap<TaskId, Vec<u32>> = BTreeMap::new();
    for event in events {
        let provenance = event.provenance();
        grouped
            .entry(provenance.task.clone())
            .or_default()
            .push(provenance.seq);
    }
    grouped
}

/// Asserts `seqs` starts at zero and grows by exactly one per event: the
/// property that lets a log order one task's records without a clock.
fn assert_dense_from_zero(task: &TaskId, seqs: &[u32]) {
    assert!(!seqs.is_empty(), "task {task} reported nothing");
    for (position, seq) in seqs.iter().enumerate() {
        assert_eq!(
            *seq,
            u32::try_from(position).expect("a test emits fewer than u32::MAX events"),
            "task {task}'s sequence must be dense from zero: {seqs:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn provenance_seq_is_strictly_increasing_within_one_task_across_a_fanout() {
    // Three arms interleave at their infer yields, so events from four
    // tasks (the walk and three arms) land in the buffer in a shuffled
    // order. Each task's own sequence must still be dense from zero, and
    // an arm's events must never borrow the walk's counter.
    let gateway =
        ScriptedGateway::start(vec![resp_text("A"), resp_text("B"), resp_text("C")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'a', 'b', 'c'})\n\
        return r[1].text .. r[2].text .. r[3].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.write(item, item)\n\
        return models.infer(item)\n\
        ```\n";
    let prompt = parse(md);
    let mut ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let events = ctx.record_events_for_test();
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the fanout completes");
    assert_eq!(out.len(), 3, "three one-letter answers: {out}");

    let events = events.lock().expect("the tap mutex is not poisoned");
    let grouped = seqs_by_task(&events);
    assert_eq!(
        grouped.keys().cloned().collect::<Vec<_>>(),
        vec![task("0"), task("0.0"), task("0.1"), task("0.2")],
        "the walk and each arm is its own task: {grouped:?}"
    );
    for (task, seqs) in &grouped {
        assert_dense_from_zero(task, seqs);
    }
    // The arms' events - their section boundaries, store writes, and
    // model turns - are stamped with the arm's task, not the spawner's.
    for arm in ["0.0", "0.1", "0.2"] {
        let arm_kinds: Vec<&Event> = events
            .iter()
            .filter(|event| event.provenance().task == task(arm))
            .collect();
        assert!(
            arm_kinds
                .iter()
                .any(|event| matches!(event, Event::StoreWriteSucceeded { .. })),
            "arm {arm}'s store write is stamped with its own task: {arm_kinds:?}"
        );
        assert!(
            arm_kinds
                .iter()
                .any(|event| matches!(event, Event::ModelTurnCompleted { .. })),
            "arm {arm}'s model turn is stamped with its own task: {arm_kinds:?}"
        );
        assert!(
            arm_kinds.iter().any(
                |event| matches!(event, Event::TaskSucceeded { task: ended, .. } if *ended == task(arm))
            ),
            "arm {arm}'s terminal is stamped with its own task: {arm_kinds:?}"
        );
    }
    // The spawn itself is the spawner's act: `TaskStarted` rides the
    // walk's counter.
    let starts: Vec<&TaskId> = events
        .iter()
        .filter_map(|event| match event {
            Event::TaskStarted { provenance, .. } => Some(&provenance.task),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        vec![&task("0"), &task("0"), &task("0")],
        "each arm's start is stamped with the spawning walk's task"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_child_reports_under_its_callers_task() {
    // A `call` blocks its caller, so the two never interleave: the child's
    // events continue the caller's one sequence under task `0`.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Call\n\n\
        ## Outer\n\n\
        ```lua\nreturn call('## Inner')\n```\n\n\
        ## Inner\n\n\
        ```lua\nlog('inner ran')\nreturn 'hello'\n```\n";
    let prompt = parse(md);
    let mut ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let events = ctx.record_events_for_test();
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the call completes");
    assert_eq!(out, "hello");

    let events = events.lock().expect("the tap mutex is not poisoned");
    let grouped = seqs_by_task(&events);
    assert_eq!(
        grouped.keys().cloned().collect::<Vec<_>>(),
        vec![task("0")],
        "a call child is not a task of its own: {grouped:?}"
    );
    assert_dense_from_zero(&task("0"), &grouped[&task("0")]);
    let inner_log = events.iter().find(|event| {
        matches!(event, Event::Lua { section, message, .. } if section == "Inner" && message == "inner ran")
    });
    assert!(
        inner_log.is_some(),
        "the child's Lua checkpoint reaches the buffer under its section: {events:?}"
    );
}

#[tokio::test]
async fn the_run_forwards_every_buffered_event_to_the_host_observer_in_order() {
    // The adapter path end to end: a two-section run through `execute::run`
    // reaches the host's observer with the exact sequence the buffered
    // events carry, run boundaries included.
    let (result, records) = run_recorded(TWO_SECTIONS).await;
    assert_eq!(result.unwrap(), "second");
    let observed = events(&records);
    assert_eq!(
        observed.first(),
        Some(&("Test prompt".to_owned(), detail::RUN_STARTED.to_string()))
    );
    assert_eq!(
        observed.last(),
        Some(&("Test prompt".to_owned(), detail::RUN_SUCCEEDED.to_string()))
    );
    let first_started = observed
        .iter()
        .position(|(section, event)| {
            section == "First" && *event == detail::SECTION_STARTED.to_string()
        })
        .expect("the first section starts");
    let second_started = observed
        .iter()
        .position(|(section, event)| {
            section == "Second" && *event == detail::SECTION_STARTED.to_string()
        })
        .expect("the second section starts");
    assert!(
        first_started < second_started,
        "forwarding keeps the buffer's order: {observed:?}"
    );
}
