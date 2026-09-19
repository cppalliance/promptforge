//! The task arena and `tasks.spawn`: a spawn returns to its caller before
//! the child chain runs, a finished child moves its slot to `Done` and
//! reports its terminal task observation, `TaskStarted` carries the spawn
//! seeds the child then sees (`args`, `item`, `sys.index`, `var`, its own
//! `sys.taskid`), and spawn shares `call`'s target resolution and depth cap.
//! The chain-end rules: a chain ending with live author tasks fails as
//! `tasks_live` naming the leaked ids (as the run's error at the root, as
//! the call's answer for a `call` chain), the leaked tasks are abandoned;
//! a task spawned in H1 belongs to the walk after the hand-off; an aborted
//! chain's owned tasks abort with it.

use promptforge_api_types::ids::{AbandonReason, TaskId, TaskOrigin};

use super::scheduler::scheduler_context_on;
use super::*;
use crate::execute::scheduler::TaskState;

/// A recorder that keeps the typed observation, so a payload-carrying
/// variant (`TaskStarted`) can be matched whole.
#[derive(Default)]
pub(super) struct TaskRecorder(Mutex<Vec<(String, Observation)>>);

impl Observer for TaskRecorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .push((section.to_owned(), event));
    }
}

impl TaskRecorder {
    pub(super) fn records(&self) -> Vec<(String, Observation)> {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .clone()
    }

    /// The position of the first record whose section and observation
    /// match, or a panic naming what was recorded.
    fn position(&self, section: &str, event: &Observation) -> usize {
        let records = self.records();
        records
            .iter()
            .position(|(seen_section, seen)| seen_section == section && seen == event)
            .unwrap_or_else(|| panic!("no record ({section}, {event:?}) in {records:?}"))
    }
}

fn task(id: &str) -> TaskId {
    id.parse().expect("a task id parses")
}

/// A two-section prompt whose first section spawns the second and then
/// parks on a store write, so the child runs to completion while the
/// spawner is suspended, before the spawner's scalar return ends the run.
fn spawner_prompt(spawner_body: &str, child_body: &str) -> String {
    format!(
        "---\nname: tasks\ndescription: d\npromptforge: 0\n---\n\n\
         # Tasks\n\n\
         ## Spawner\n\n\
         ```lua\n{spawner_body}\n```\n\n\
         ## Child\n\n\
         ```lua\n{child_body}\n```\n"
    )
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_returns_to_its_caller_before_the_child_runs() {
    let md = spawner_prompt(
        "local t = tasks.spawn('## Child')\n\
         log('spawned ' .. t.task)\n\
         store.write('park', 'x')\n\
         return 'done'",
        "log('child ran')",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, None)
        .drive()
        .await
        .expect("the spawner's return ends the run");
    assert_eq!(out, "done");

    let spawned = recorder.position("Spawner", &Observation::Lua("spawned 0.0".to_owned()));
    let child_ran = recorder.position("Child", &Observation::Lua("child ran".to_owned()));
    assert!(
        spawned < child_ran,
        "the spawner continues past `spawn` before the child's first block runs: {:?}",
        recorder.records()
    );
    let child_started = recorder.position("Child", &detail::SECTION_STARTED);
    assert!(
        spawned < child_started,
        "the child's section is not even entered until the spawner suspends: {:?}",
        recorder.records()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_finished_child_moves_its_slot_to_done_and_reports_task_succeeded() {
    let md = spawner_prompt(
        "tasks.spawn('## Child')\n\
         store.write('park', 'x')\n\
         return 'done'",
        "return 'child result'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
    scheduler.drive().await.expect("the run completes");

    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Done),
        "the child's completion moves its slot to Done"
    );
    let started = recorder.position(
        "Spawner",
        &Observation::TaskStarted {
            task: task("0.0"),
            target: "Child".to_owned(),
            origin: TaskOrigin::Author,
            input: None,
            item: None,
            index: None,
            var: json!({}),
        },
    );
    let succeeded = recorder.position("Child", &Observation::TaskSucceeded { task: task("0.0") });
    assert!(started < succeeded, "started precedes succeeded");
    // The store op's own observation fires on the blocking pool, so the
    // spawner's resume point is its chunk's close, reported by the driver.
    let resumed = recorder.position("Spawner", &detail::LUA_CHUNK_SUCCEEDED);
    assert!(
        succeeded < resumed,
        "the child ran to completion while the spawner was parked: {:?}",
        recorder.records()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_child_moves_its_slot_to_done_and_reports_task_failed() {
    let md = spawner_prompt(
        "tasks.spawn('## Child')\n\
         store.write('park', 'x')\n\
         return 'done'",
        "error('child boom')",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
    let out = scheduler
        .drive()
        .await
        .expect("a task's failure is the task's outcome, not the run's");
    assert_eq!(out, "done");

    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Done),
        "a failed child's slot is Done with a failed outcome"
    );
    recorder.position("Child", &Observation::TaskFailed { task: task("0.0") });
    let records = recorder.records();
    assert!(
        !records
            .iter()
            .any(|(_, event)| matches!(event, Observation::TaskSucceeded { .. })),
        "a failed task never reports success: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn task_started_carries_the_spawn_seeds_and_the_child_sees_them() {
    let md = spawner_prompt(
        "var.k = 1\n\
         log('root taskid=' .. sys.taskid)\n\
         tasks.spawn('## Child', { input = 'child args', item = { name = 'alpha' }, index = 7 })\n\
         store.write('park', 'x')\n\
         return 'done'",
        "log('taskid=' .. sys.taskid .. ' id=' .. sys.id .. ' index=' .. sys.index\n\
           .. ' item=' .. item.name .. ' args=' .. args .. ' k=' .. tostring(var.k))",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    TokioDriver::new(&ctx, None)
        .drive()
        .await
        .expect("the run completes");

    recorder.position(
        "Spawner",
        &Observation::TaskStarted {
            task: task("0.0"),
            target: "Child".to_owned(),
            origin: TaskOrigin::Author,
            input: Some("child args".to_owned()),
            item: Some(json!({ "name": "alpha" })),
            index: Some(7),
            var: json!({ "k": 1 }),
        },
    );
    recorder.position("Spawner", &Observation::Lua("root taskid=0".to_owned()));
    recorder.position(
        "Child",
        &Observation::Lua("taskid=0.0 id=0.0.0 index=7 item=alpha args=child args k=1".to_owned()),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_shares_calls_target_resolution_and_raises_at_the_call_site() {
    // An unresolvable target is the call's answer: `pcall` catches it, and
    // the message is exactly the one `call` produces for the same heading.
    let md = spawner_prompt(
        "local ok_s, err_s = pcall(tasks.spawn, '## Missing')\n\
         local ok_c, err_c = pcall(call, '## Missing')\n\
         assert(not ok_s and not ok_c, 'both resolutions fail')\n\
         assert(err_s.kind == 'lua', err_s.kind)\n\
         assert(tostring(err_s) == tostring(err_c), tostring(err_s) .. ' vs ' .. tostring(err_c))\n\
         return tostring(err_s)",
        "return 'unused'",
    );
    let prompt = parse(&md);
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, None)
        .drive()
        .await
        .expect("the caught errors end the run normally");
    assert!(
        out.contains("section heading `## Missing` not found"),
        "unexpected message: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_shares_calls_depth_cap() {
    // Alpha and Beta spawn each other, each spawned chain one level
    // deeper (a section's visible set excludes itself, as for `call`); the
    // chain at depth 8 is refused with `call`'s own cap message, as the
    // spawn's answer, so the deepest chain catches and logs it.
    let md = "---\nname: depth\ndescription: d\npromptforge: 0\n---\n\n\
        # Depth\n\n\
        ## Alpha\n\n\
        ```lua\n\
        local ok, err = pcall(tasks.spawn, '## Beta')\n\
        if not ok then log(tostring(err)) end\n\
        store.write('park-' .. sys.id, 'x')\n\
        return 'done'\n\
        ```\n\n\
        ## Beta\n\n\
        ```lua\n\
        local ok, err = pcall(tasks.spawn, '## Alpha')\n\
        if not ok then log(tostring(err)) end\n\
        store.write('park-' .. sys.id, 'x')\n\
        return 'done'\n\
        ```\n";
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    // Every spawner parks on a store write after spawning, so whether a
    // spawner resumes before or after its child ends depends on the
    // blocking pool's answer order: the run ends `done` or `tasks_live`.
    // Either way the whole spawn cascade and the one refusal ran before
    // any answer arrived, which is what this test measures.
    let outcome = TokioDriver::new(&ctx, None).drive().await;
    assert!(
        matches!(outcome, Ok(_) | Err(Error::TasksLive { .. })),
        "unexpected outcome: {outcome:?}"
    );

    let records = recorder.records();
    let refusals = records
        .iter()
        .filter(|(_, event)| {
            *event == Observation::Lua("call recursion exceeded cap of 8".to_owned())
        })
        .count();
    assert_eq!(refusals, 1, "exactly one spawn is refused: {records:?}");
    let starts = records
        .iter()
        .filter(|(_, event)| matches!(event, Observation::TaskStarted { .. }))
        .count();
    assert_eq!(starts, 8, "depths 1 through 8 start; depth 9 is refused");
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_rejects_a_list_section_target_with_the_worker_message() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Lists\n\n\
        ## Parent\n\n\
        ```lua\n\
        local ok, err = pcall(tasks.spawn, '### Items')\n\
        assert(not ok, 'a list section is not a worker template')\n\
        return tostring(err)\n\
        ```\n\n\
        ### Items\n\n\
        - a\n\
        - b\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, None)
        .drive()
        .await
        .expect("the caught error ends the run normally");
    assert_eq!(
        out,
        "section `Items` is a list section, not a worker template"
    );
}

/// A child body that parks on a store write and never returns on its own,
/// so the task stays live until something ends it.
const PARKED_CHILD: &str = "store.write('park-' .. sys.id, 'x')\nreturn 'never'";

#[tokio::test(flavor = "current_thread")]
async fn a_chain_ending_with_live_author_tasks_fails_as_tasks_live_naming_the_ids() {
    // The spawner ends while both children are live (the first parked on
    // its store write, the second not yet started): the run fails with
    // `tasks_live` naming both ids in spawn order, and both tasks are
    // abandoned - slot and terminal observation - because their owner
    // returned.
    let md = spawner_prompt(
        "tasks.spawn('## Child')\n\
         tasks.spawn('## Child')\n\
         return 'done'",
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
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
        recorder.position(
            "Child",
            &Observation::TaskAbandoned {
                task: task(id),
                reason: AbandonReason::OwnerReturned,
            },
        );
    }
    let records = recorder.records();
    assert!(
        !records.iter().any(|(_, event)| matches!(
            event,
            Observation::TaskSucceeded { .. } | Observation::TaskFailed { .. }
        )),
        "an abandoned task reports no other terminal event: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_chain_failing_with_a_live_task_keeps_its_own_error_and_abandons_the_task() {
    // The spawner errors after spawning: the leak is the lesser fault, so
    // the run's error is the spawner's own, not `tasks_live`; the task
    // still ends with its owner, its slot Abandoned and its terminal
    // observation naming the failed owner.
    let md = spawner_prompt(
        "tasks.spawn('## Child')\n\
         error('spawner boom')",
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
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
    recorder.position(
        "Child",
        &Observation::TaskAbandoned {
            task: task("0.0"),
            reason: AbandonReason::OwnerFailed,
        },
    );
    let records = recorder.records();
    assert!(
        !records.iter().any(|(_, event)| matches!(
            event,
            Observation::TaskSucceeded { .. } | Observation::TaskFailed { .. }
        )),
        "an abandoned task reports no other terminal event: {records:?}"
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
/// alone, with the task's slot Abandoned because its owner returned.
async fn assert_task_outlives_movement(md: &str) {
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
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
    recorder.position(
        "Child",
        &Observation::TaskAbandoned {
            task: task("0.0"),
            reason: AbandonReason::OwnerReturned,
        },
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
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
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
    // to a chain that never finishes and the run would end `done`.
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
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
    let error = scheduler
        .drive()
        .await
        .expect_err("the walk owns H1's task and leaks it");

    match &error {
        Error::TasksLive { tasks } => assert_eq!(tasks, &[task("0.0")]),
        other => panic!("expected tasks_live, got {other:?}"),
    }
    recorder.position(
        "Tasks",
        &Observation::TaskStarted {
            task: task("0.0"),
            target: "Child".to_owned(),
            origin: TaskOrigin::Author,
            input: None,
            item: None,
            index: None,
            var: json!({}),
        },
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn aborting_a_chain_abandons_the_tasks_it_owns() {
    // A fatal sibling arm makes the fanout shim cancel the spawning arm
    // before it resumes; the abort takes the arm's task with it: the
    // task's slot is Abandoned because its owner was aborted, its terminal
    // observation fires, and its chain never runs its block (it gets at
    // most the one step that enters its section before the cancel lands).
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
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
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
    recorder.position(
        "Child",
        &Observation::TaskAbandoned {
            task: task("0.0.0"),
            reason: AbandonReason::OwnerAborted,
        },
    );
    let records = recorder.records();
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
