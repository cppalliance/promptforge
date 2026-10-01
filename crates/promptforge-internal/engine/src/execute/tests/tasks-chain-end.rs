//! The chain-end rules: a chain ending with live author tasks fails as
//! `tasks_live` naming the leaked ids (as the run's error at the root, as
//! the call's answer for a `call` chain), the leaked tasks are abandoned;
//! a task spawned in H1 belongs to the walk after the hand-off; an aborted
//! chain's owned tasks abort with it.

use super::*;

/// A child body that parks on a store write and never returns on its own,
/// so the task stays live until something ends it.
const PARKED_CHILD: &str = "store.write('park-' .. sys.id, 'x')\nreturn 'never'";

#[tokio::test(flavor = "current_thread")]
async fn a_chain_ending_with_live_author_tasks_fails_as_tasks_live_naming_the_ids() {
    // The spawner ends while both children are live - neither was ever
    // admitted, since the spawner returned without suspending - so the
    // run fails with `tasks_live` naming both ids in spawn order, and
    // both tasks are abandoned because their owner returned. A task that
    // never ran reports no start and no terminal, though its slot still
    // reaches Abandoned.
    let md = spawner_prompt(
        "tasks.spawn('## Child')\n\
         tasks.spawn('## Child')\n\
         return 'done'",
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let (ctx, harness) = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, harness, None);
    let error = scheduler
        .drive()
        .await
        .expect_err("live author tasks fail their owner's chain");

    match &error {
        Error::TasksLive { tasks } => {
            assert_eq!(
                tasks,
                &[task("0.0"), task("0.1")],
                "leaked ids in spawn order"
            );
        }
        other => panic!("expected tasks_live, got {other:?}"),
    }
    let text = error.to_string();
    assert!(
        text.contains("0.0, 0.1"),
        "the message names the leaked ids: {text}"
    );
    for id in ["0.0", "0.1"] {
        assert_eq!(
            scheduler.task_state_for_test(&task(id)),
            Some(TaskState::Abandoned),
            "a leaked task's slot is Abandoned, not Done or Cancelled"
        );
    }
    let records = recorder.records();
    assert!(
        !records.iter().any(|(_, event)| matches!(
            event,
            Observation::TaskStarted { .. }
                | Observation::TaskSucceeded { .. }
                | Observation::TaskFailed { .. }
                | Observation::TaskCancelled { .. }
                | Observation::TaskAbandoned { .. }
        )),
        "a task that never ran reports no start and no terminal: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_chain_failing_with_a_live_task_keeps_its_own_error_and_abandons_the_task() {
    // The spawner errors after spawning: the leak is the lesser fault, so
    // the run's error is the spawner's own, not `tasks_live`; the task
    // still ends with its owner, its slot Abandoned. The task was never
    // admitted - the spawner failed without suspending - so it reports no
    // start and no terminal.
    let md = spawner_prompt(
        "tasks.spawn('## Child')\n\
         error('spawner boom')",
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let (ctx, harness) = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, harness, None);
    let error = scheduler
        .drive()
        .await
        .expect_err("the spawner's error fails the run");

    assert!(
        !matches!(error, Error::TasksLive { .. }),
        "a failing owner keeps its own error over the leak: {error:?}"
    );
    assert!(
        error.to_string().contains("spawner boom"),
        "the run's error is the spawner's: {error}"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned),
        "the task ended with its failed owner"
    );
    let records = recorder.records();
    assert!(
        !records.iter().any(|(_, event)| matches!(
            event,
            Observation::TaskStarted { .. }
                | Observation::TaskSucceeded { .. }
                | Observation::TaskFailed { .. }
                | Observation::TaskCancelled { .. }
                | Observation::TaskAbandoned { .. }
        )),
        "a task that never ran reports no start and no terminal: {records:?}"
    );
}

/// A three-section prompt whose first section spawns the parked third and
/// then leaves itself by `movement` (a fall-through or a `jump`) to the
/// second, which returns; the task must still belong to the chain when
/// the second section's return ends it.
fn moving_spawner_prompt(movement: &str) -> String {
    format!(
        "---\nname: tasks\ndescription: d\npromptforge: 0\n---\n\n\
         # Tasks\n\n\
         ## Spawner\n\n\
         ```lua\n\
         tasks.spawn('## Child')\n\
         {movement}\
         ```\n\n\
         ## Sibling\n\n\
         ```lua\n\
         return 'done'\n\
         ```\n\n\
         ## Child\n\n\
         ```lua\n{PARKED_CHILD}\n```\n"
    )
}

/// Drives `md` and asserts that the run fails `tasks_live` naming `0.0`
/// alone, with the task's slot Abandoned because its owner returned. The
/// task never ran - neither chain suspended between the spawn and the
/// end - so it reports no start and no terminal.
async fn assert_task_outlives_movement(md: &str) {
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let (ctx, harness) = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, harness, None);
    let error = scheduler
        .drive()
        .await
        .expect_err("the walk still owns the task when the sibling returns");

    match &error {
        Error::TasksLive { tasks } => assert_eq!(tasks, &[task("0.0")]),
        other => panic!("expected tasks_live, got {other:?}"),
    }
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned),
        "the task ended with the chain, not with the section that spawned it"
    );
    let records = recorder.records();
    assert!(
        !records.iter().any(|(_, event)| matches!(
            event,
            Observation::TaskStarted { .. }
                | Observation::TaskSucceeded { .. }
                | Observation::TaskFailed { .. }
                | Observation::TaskCancelled { .. }
                | Observation::TaskAbandoned { .. }
        )),
        "a task that never ran reports no start and no terminal: {records:?}"
    );
    recorder.position("Sibling", &detail::SECTION_STARTED);
}

#[tokio::test(flavor = "current_thread")]
async fn a_task_survives_its_spawning_sections_fall_through() {
    // The spawner's chunk ends without a scalar, so the walk falls through
    // to the sibling: the task is the chain's, not the section's, and the
    // sibling's return leaks it.
    assert_task_outlives_movement(&moving_spawner_prompt("")).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_task_survives_its_spawning_sections_jump() {
    // A `jump` moves the walk within the same chain, so it settles nothing:
    // the jumped-to sibling's return leaks the task.
    assert_task_outlives_movement(&moving_spawner_prompt("jump('## Sibling')\n")).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_chain_ending_with_a_live_task_answers_tasks_live_to_its_caller() {
    // The leak is the call's answer, not the run's error: the caller's
    // `pcall` sees the `tasks_live` kind with the leaked ids in `tasks`,
    // and the task ended with the call chain that owned it.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Tasks\n\n\
        ## Main\n\n\
        ```lua\n\
        local ok, err = pcall(call, '## Leaky')\n\
        assert(not ok, 'the leaky call fails')\n\
        return err.kind .. '|' .. err.tasks .. '|' .. tostring(err)\n\
        ```\n\n\
        ## Leaky\n\n\
        ```lua\n\
        tasks.spawn('## Child')\n\
        return 'leaked'\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\n\
        store.write('park', 'x')\n\
        return 'never'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let mut scheduler = TokioDriver::new(&ctx, harness, None);
    let out = scheduler
        .drive()
        .await
        .expect("the caught leak ends the run normally");

    let (kind, rest) = out.split_once('|').expect("kind|tasks|message");
    let (tasks, message) = rest.split_once('|').expect("tasks|message");
    assert_eq!(kind, "tasks_live");
    assert_eq!(tasks, "0.0.0", "the call chain's spawn is its first child");
    assert!(
        message.contains("0.0.0"),
        "the message names the leaked id: {message}"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0.0")),
        Some(TaskState::Abandoned),
        "the task ended with its owner's call chain"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_task_spawned_in_h1_belongs_to_the_main_walk() {
    // H1 spawns a child that parks; the walk's first section returns at
    // once. The hand-off made the walk the task's owner, so the walk's end
    // is the leak: without the reassignment the pass's task would belong
    // to a chain that never finishes and the run would end `done`. The
    // task was never admitted - neither the pass nor the walk suspended
    // between the spawn and the walk's end - so it reports no start and
    // no terminal, though its slot ends Abandoned.
    let md = "---\nname: h1\ndescription: d\npromptforge: 0\n---\n\n\
        # Tasks\n\n\
        ```lua\n\
        tasks.spawn('## Child')\n\
        ```\n\n\
        ## Main\n\n\
        ```lua\n\
        return 'done'\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\n\
        store.write('park', 'x')\n\
        return 'never'\n\
        ```\n";
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let (ctx, harness) = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, harness, None);
    let error = scheduler
        .drive()
        .await
        .expect_err("the walk owns H1's task and leaks it");

    match &error {
        Error::TasksLive { tasks } => assert_eq!(tasks, &[task("0.0")]),
        other => panic!("expected tasks_live, got {other:?}"),
    }
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned)
    );
    let records = recorder.records();
    assert!(
        !records.iter().any(|(_, event)| matches!(
            event,
            Observation::TaskStarted { .. }
                | Observation::TaskSucceeded { .. }
                | Observation::TaskFailed { .. }
                | Observation::TaskCancelled { .. }
                | Observation::TaskAbandoned { .. }
        )),
        "a task that never ran reports no start and no terminal: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn aborting_a_chain_abandons_the_tasks_it_owns() {
    // A fatal sibling arm makes the fanout shim cancel the spawning arm
    // before it resumes; the abort takes the arm's task with it: the
    // task's slot is Abandoned because its owner was aborted. The task
    // was never admitted - the arm was cancelled while its spawn was
    // still queued - so it reports no start and no terminal, and its
    // chain never runs its block.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Tasks\n\n\
        ## Main\n\n\
        ```lua\n\
        local ok, err = pcall(fanout, '## Worker', {'spawner', 'boom'})\n\
        assert(not ok, 'the fatal arm fails the fanout')\n\
        return tostring(err)\n\
        ```\n\n\
        ## Worker\n\n\
        ```lua\n\
        if item == 'spawner' then\n\
          tasks.spawn('## Child')\n\
          store.write('park-' .. sys.id, 'x')\n\
          return 'never'\n\
        end\n\
        error('boom')\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\n\
        store.write('child-park', 'x')\n\
        return 'never'\n\
        ```\n";
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let (ctx, harness) = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, harness, None);
    let out = scheduler
        .drive()
        .await
        .expect("the caught fanout failure ends the run normally");
    assert!(out.contains("boom"), "the fatal arm's error: {out}");

    assert_eq!(
        scheduler.task_state_for_test(&task("0.0.0")),
        Some(TaskState::Abandoned),
        "the aborted arm's task is Abandoned"
    );
    let records = recorder.records();
    assert!(
        !records.iter().any(|(section, event)| section == "Child"
            && matches!(
                event,
                Observation::TaskStarted { .. }
                    | Observation::TaskSucceeded { .. }
                    | Observation::TaskFailed { .. }
                    | Observation::TaskCancelled { .. }
                    | Observation::TaskAbandoned { .. }
            )),
        "the abandoned task never ran, so it reports nothing: {records:?}"
    );
    assert!(
        !records
            .iter()
            .any(|(section, event)| section == "Child" && *event == detail::LUA_CHUNK_STARTED),
        "the abandoned task's chain never ran its block: {records:?}"
    );
    assert_eq!(
        records
            .iter()
            .filter(|(_, event)| *event == Observation::TaskCancelled { task: task("0.0") })
            .count(),
        1,
        "the fanout shim cancels the spawning arm exactly once: {records:?}"
    );
}
