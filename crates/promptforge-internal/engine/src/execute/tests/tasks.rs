//! The task arena and `tasks.spawn`: a spawn returns to its caller before
//! the child chain runs, a finished child moves its slot to `Done` and
//! reports its terminal task observation, `TaskStarted` reports the spawn
//! seeds the child then sees (`args`, `item`, `sys.index`, `var`, its own
//! `sys.taskid`), and spawn shares `call`'s target resolution and depth cap.
//! The chain-end rules sit in `chain_end`.

use promptforge_types::ids::{TaskId, TaskOrigin};

use super::scheduler::scheduler_scenario;
use super::*;
use crate::execute::scheduler::test_hooks::TaskState;
use crate::test_support::rig::Rig;

/// A recorder that keeps the typed observation, so a variant with a
/// payload (`TaskStarted`) can be matched whole.
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
    /// A recorder holding what `trace`'s events report, parse events
    /// first.
    pub(super) fn replayed(trace: &Trace) -> TaskRecorder {
        let recorder = TaskRecorder::default();
        trace.replay(&recorder);
        recorder
    }

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

    /// The `log` messages recorded under `section`, in order.
    pub(super) fn logs(&self, section: &str) -> Vec<String> {
        self.records()
            .into_iter()
            .filter(|(seen, _)| seen == section)
            .filter_map(|(_, event)| match event {
                Observation::Lua(message) => Some(message),
                _ => None,
            })
            .collect()
    }

    /// Every task observation naming `task`, in order.
    pub(super) fn observations_of(&self, task: &TaskId) -> Vec<Observation> {
        self.records()
            .into_iter()
            .map(|(_, event)| event)
            .filter(|event| match event {
                Observation::TaskStarted { task: seen, .. }
                | Observation::TaskSucceeded { task: seen }
                | Observation::TaskFailed { task: seen }
                | Observation::TaskCancelled { task: seen }
                | Observation::TaskAbandoned { task: seen, .. } => seen == task,
                _ => false,
            })
            .collect()
    }
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

#[test]
fn spawn_returns_to_its_caller_before_the_child_runs() {
    let md = spawner_prompt(
        "local t = tasks.spawn('## Child')\n\
         log('spawned ' .. t.task)\n\
         store.write('park', 'x')\n\
         return 'done'",
        "log('child ran')",
    );
    let rig = Rig::run(scheduler_scenario(md));
    assert_eq!(
        rig.trace().text(),
        "done",
        "the spawner's return ends the run"
    );

    let recorder = TaskRecorder::replayed(rig.trace());
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

#[test]
fn a_finished_child_moves_its_slot_to_done_and_reports_task_succeeded() {
    let md = spawner_prompt(
        "tasks.spawn('## Child')\n\
         store.write('park', 'x')\n\
         return 'done'",
        "return 'child result'",
    );
    let mut rig = Rig::run(scheduler_scenario(md));
    assert_eq!(rig.trace().text(), "done", "the run completes");

    assert_eq!(
        rig.task_state_for_test(&task("0.0")),
        Some(TaskState::Done { ok: true }),
        "the child's completion moves its slot to Done"
    );
    let recorder = TaskRecorder::replayed(rig.trace());
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
    // The spawner's resume point is its chunk's close, after the store
    // write's answer.
    let resumed = recorder.position("Spawner", &detail::LUA_CHUNK_SUCCEEDED);
    assert!(
        succeeded < resumed,
        "the child ran to completion while the spawner was parked: {:?}",
        recorder.records()
    );
}

#[test]
fn a_failed_child_moves_its_slot_to_done_and_reports_task_failed() {
    let md = spawner_prompt(
        "tasks.spawn('## Child')\n\
         store.write('park', 'x')\n\
         return 'done'",
        "error('child boom')",
    );
    let mut rig = Rig::run(scheduler_scenario(md));
    assert_eq!(
        rig.trace().text(),
        "done",
        "a task's failure is the task's outcome, not the run's"
    );

    assert_eq!(
        rig.task_state_for_test(&task("0.0")),
        Some(TaskState::Done { ok: false }),
        "a failed child's slot is Done with a failed outcome"
    );
    let recorder = TaskRecorder::replayed(rig.trace());
    recorder.position("Child", &Observation::TaskFailed { task: task("0.0") });
    let records = recorder.records();
    assert!(
        !records
            .iter()
            .any(|(_, event)| matches!(event, Observation::TaskSucceeded { .. })),
        "a failed task never reports success: {records:?}"
    );
}

#[test]
fn task_started_includes_the_spawn_seeds_and_the_child_sees_them() {
    let md = spawner_prompt(
        "var.k = 1\n\
         log('root taskid=' .. sys.taskid)\n\
         tasks.spawn('## Child', { input = 'child args', item = { name = 'alpha' }, index = 7 })\n\
         store.write('park', 'x')\n\
         return 'done'",
        "log('taskid=' .. sys.taskid .. ' id=' .. sys.id .. ' index=' .. sys.index\n\
           .. ' item=' .. item.name .. ' args=' .. args .. ' k=' .. tostring(var.k))",
    );
    let rig = Rig::run(scheduler_scenario(md));
    assert_eq!(rig.trace().text(), "done", "the run completes");

    let recorder = TaskRecorder::replayed(rig.trace());
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

#[test]
fn spawn_shares_calls_target_resolution_and_raises_at_the_call_site() {
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
    let rig = Rig::run(scheduler_scenario(md));
    let out = rig.trace().text();
    assert!(
        out.contains("section heading `## Missing` not found"),
        "unexpected message: {out}"
    );
}

#[test]
fn spawn_shares_calls_depth_cap() {
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
    // Every spawner parks on a store write after spawning, so whether a
    // spawner resumes before or after its child ends depends on the order
    // the writes are answered in: the run ends `done` or `tasks_live`.
    // Either way the whole spawn cascade and the one refusal ran before
    // any answer arrived, which is what this test measures.
    let rig = Rig::run(scheduler_scenario(md));
    let outcome = rig.trace().result();
    assert!(
        matches!(outcome, RunResult::Ok(_))
            || matches!(outcome, RunResult::Failure(error)
                if matches!(error.inner(), Error::TasksLive { .. })),
        "unexpected outcome: {outcome:?}"
    );

    let records = TaskRecorder::replayed(rig.trace()).records();
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

#[test]
fn spawn_rejects_a_list_section_target_with_the_worker_message() {
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
    let rig = Rig::run(scheduler_scenario(md));
    assert_eq!(
        rig.trace().text(),
        "section `Items` is a list section, not a worker template",
        "the caught error ends the run normally"
    );
}

#[path = "tasks-chain-end.rs"]
mod chain_end;
