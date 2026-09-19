//! The wait, status, note, and cancel arms: `tasks.when_any` is the one
//! scheduler wait primitive and `tasks.when_all` is Lua over it (reporting
//! a failed member without raising); `tasks.status` reads a parked and a
//! finished task; `tasks.ready`, `tasks.pending`, `tasks.note`, and
//! `tasks.cancel` round-trip; ownership is enforced (`task_not_owned`,
//! with the self exception for `status` and `note`); a delivered task's
//! second wait raises `task_consumed`; cancel is idempotent and reports
//! `TaskCancelled` exactly once.

use std::time::Duration;

use promptforge_api_types::ids::TaskId;

use super::scheduler::scheduler_context_on;
use super::*;
use crate::execute::scheduler::{Scheduler, TaskState};

/// A recorder that keeps the typed observation, so a payload-carrying
/// variant can be matched whole.
#[derive(Default)]
struct WaitRecorder(Mutex<Vec<(String, Observation)>>);

impl Observer for WaitRecorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .push((section.to_owned(), event));
    }
}

impl WaitRecorder {
    fn records(&self) -> Vec<(String, Observation)> {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .clone()
    }

    /// The `log` messages recorded under `section`, in order.
    fn logs(&self, section: &str) -> Vec<String> {
        self.records()
            .into_iter()
            .filter(|(seen, _)| seen == section)
            .filter_map(|(_, event)| match event {
                Observation::Lua(message) => Some(message),
                _ => None,
            })
            .collect()
    }
}

fn task(id: &str) -> TaskId {
    id.parse().expect("a task id parses")
}

/// A prompt whose first section drives the tasks it spawns over the
/// remaining sections.
fn tasks_prompt(main: &str, sections: &[(&str, &str)]) -> String {
    let mut md = format!(
        "---\nname: waits\ndescription: d\npromptforge: 0\n---\n\n\
         # Waits\n\n\
         ## Main\n\n\
         ```lua\n{main}\n```\n"
    );
    for (name, body) in sections {
        md.push_str("\n## ");
        md.push_str(name);
        md.push_str("\n\n```lua\n");
        md.push_str(body);
        md.push_str("\n```\n");
    }
    md
}

/// Drives `md` offline and returns the run's result with the recorder.
async fn drive(md: &str) -> (Result<String>, Arc<WaitRecorder>) {
    let prompt = parse(md);
    let recorder = Arc::new(WaitRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = Scheduler::new(&ctx, None).drive().await;
    (out, recorder)
}

#[tokio::test(flavor = "current_thread")]
async fn when_all_reports_a_failed_member_without_raising() {
    // One member returns, one raises: `when_all` returns both outcomes in
    // input order and the caller decides; the failed member's result is
    // the error table, and nothing leaks at chain end.
    let md = tasks_prompt(
        "local a = tasks.spawn('## Alpha')\n\
         local b = tasks.spawn('## Beta')\n\
         local results = tasks.when_all({ b, a })\n\
         assert(#results == 2, 'two results')\n\
         assert(results[1].task == b.task, 'input order: beta first')\n\
         assert(results[1].ok == false, 'beta failed')\n\
         assert(results[1].result.kind == 'lua', results[1].result.kind)\n\
         assert(tostring(results[1].result):find('beta boom', 1, true), tostring(results[1].result))\n\
         assert(results[2].task == a.task, 'input order: alpha second')\n\
         assert(results[2].ok == true, 'alpha succeeded')\n\
         assert(results[2].result == 'alpha text', results[2].result)\n\
         return 'done'",
        &[
            ("Alpha", "return 'alpha text'"),
            ("Beta", "error('beta boom')"),
        ],
    );
    let (out, _) = drive(&md).await;
    assert_eq!(out.expect("when_all never raises for a member"), "done");
}

#[tokio::test(flavor = "current_thread")]
async fn when_all_fills_every_position_of_a_member_named_twice() {
    // A set naming one task twice: the task is waited on once and its
    // outcome lands at both positions, so the result sequence has no hole
    // and `#results` is the input's length; the second wait would have
    // raised `task_consumed` had the shim waited twice.
    let md = tasks_prompt(
        "local a = tasks.spawn('## Alpha')\n\
         local b = tasks.spawn('## Beta')\n\
         local results = tasks.when_all({ a, b, a })\n\
         assert(#results == 3, 'three positions, got ' .. #results)\n\
         local count = 0\n\
         for _ in ipairs(results) do count = count + 1 end\n\
         assert(count == 3, 'ipairs walks every position, got ' .. count)\n\
         assert(results[1].task == a.task and results[3].task == a.task, 'alpha at both ends')\n\
         assert(results[1].ok and results[1].result == 'alpha', tostring(results[1].result))\n\
         assert(results[3].ok and results[3].result == 'alpha', tostring(results[3].result))\n\
         assert(results[1] ~= results[3], 'each position is its own handle')\n\
         assert(results[2].task == b.task and results[2].result == 'beta', tostring(results[2].result))\n\
         return 'done'",
        &[("Alpha", "return 'alpha'"), ("Beta", "return 'beta'")],
    );
    let (out, _) = drive(&md).await;
    assert_eq!(out.expect("a duplicated member is waited on once"), "done");
}

#[tokio::test(flavor = "current_thread")]
async fn when_any_returns_the_first_finished_member_and_the_rest_keep_running() {
    // Alpha finishes at once; Beta parks on a store write. `when_any` over
    // both delivers Alpha and leaves Beta live, so the caller must still
    // wait on Beta before it ends - which it does.
    let md = tasks_prompt(
        "local a = tasks.spawn('## Alpha')\n\
         local b = tasks.spawn('## Beta')\n\
         local first, ok, result = tasks.when_any({ a, b })\n\
         assert(first.task == a.task, 'alpha finishes first, got ' .. first.task)\n\
         assert(ok and result == 'alpha', tostring(result))\n\
         assert(tasks.ready(a), 'alpha is ready')\n\
         local second, ok2, result2 = tasks.when_any({ b })\n\
         assert(second.task == b.task and ok2 and result2 == 'beta', tostring(result2))\n\
         return 'done'",
        &[
            ("Alpha", "return 'alpha'"),
            ("Beta", "store.write('park', 'x')\nreturn 'beta'"),
        ],
    );
    let (out, _) = drive(&md).await;
    assert_eq!(out.expect("both members are delivered"), "done");
}

#[tokio::test(flavor = "current_thread")]
async fn status_reports_a_parked_task_and_then_a_finished_one() {
    // The child parks on a slow model round; the spawner, resumed from a
    // fast store write, reads its status mid-flight (running, blocked on
    // `chat`, inside its section, with its note), waits on it, then reads
    // the terminal status (done, ok).
    let gateway = ScriptedGateway::start(vec![resp_delayed_text(
        "slow answer",
        Duration::from_millis(400),
    )])
    .await;
    let md = tasks_prompt(
        "local t = tasks.spawn('## Child')\n\
         local fresh = tasks.status(t)\n\
         log('fresh state=' .. fresh.state .. ' section=' .. tostring(fresh.section)\n\
           .. ' blocked=' .. tostring(fresh.blocked))\n\
         store.write('park', 'x')\n\
         local s = tasks.status(t)\n\
         log('parked target=' .. s.target .. ' origin=' .. s.origin .. ' state=' .. s.state\n\
           .. ' ok=' .. tostring(s.ok) .. ' section=' .. tostring(s.section)\n\
           .. ' blocked=' .. tostring(s.blocked) .. ' turns=' .. s.turns\n\
           .. ' tasks=' .. #s.tasks .. ' depth=' .. s.depth .. ' note=' .. tostring(s.note))\n\
         local _, ok, result = tasks.when_any({ t })\n\
         assert(ok and result == 'slow answer', tostring(result))\n\
         local d = tasks.status(t)\n\
         log('done state=' .. d.state .. ' ok=' .. tostring(d.ok) .. ' section=' .. tostring(d.section)\n\
           .. ' blocked=' .. tostring(d.blocked) .. ' turns=' .. d.turns .. ' note=' .. tostring(d.note))\n\
         return 'done'",
        &[(
            "Child",
            "tasks.note('working')\nreturn models.infer('slow please')",
        )],
    );
    let prompt = parse(&md);
    let recorder = Arc::new(WaitRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the run completes");
    assert_eq!(out, "done");

    let logs = recorder.logs("Main");
    assert_eq!(
        logs,
        vec![
            "fresh state=running section=nil blocked=nil".to_owned(),
            "parked target=Child origin=author state=running ok=nil section=Child blocked=chat \
             turns=0 tasks=0 depth=1 note=working"
                .to_owned(),
            "done state=done ok=true section=nil blocked=nil turns=1 note=working".to_owned(),
        ],
        "status fields before the child runs, while it is parked, and after it finished"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_non_owner_is_refused_with_task_not_owned() {
    // The child may read and annotate its own task (the self exception),
    // but waiting on or cancelling it is the owner's alone, and a task id
    // the caller never spawned is refused the same way.
    let md = tasks_prompt(
        "local t = tasks.spawn('## Child')\n\
         local ok, err = pcall(tasks.cancel, '9.9')\n\
         assert(not ok and err.kind == 'task_not_owned', tostring(err))\n\
         assert(err.task == '9.9', tostring(err.task))\n\
         local _, ok2, result = tasks.when_any({ t })\n\
         assert(ok2, tostring(result))\n\
         return result",
        &[(
            "Child",
            "local me = sys.taskid\n\
             tasks.note('hello from ' .. me)\n\
             local s = tasks.status(me)\n\
             assert(s.note == 'hello from ' .. me, tostring(s.note))\n\
             local ok, err = pcall(tasks.cancel, me)\n\
             assert(not ok and err.kind == 'task_not_owned', tostring(err))\n\
             local ok2, err2 = pcall(tasks.when_any, { me })\n\
             assert(not ok2 and err2.kind == 'task_not_owned', tostring(err2))\n\
             return 'refused:' .. tostring(err) .. '|' .. tostring(err2)",
        )],
    );
    let (out, _) = drive(&md).await;
    let out = out.expect("the caught refusals end the run normally");
    assert!(
        out.starts_with("refused:"),
        "the child's refusals are the run's result: {out}"
    );
    assert!(
        out.contains("0.0"),
        "the refusal names the task the caller reached for: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn waiting_on_a_delivered_task_raises_task_consumed() {
    let md = tasks_prompt(
        "local t = tasks.spawn('## Child')\n\
         local _, ok, result = tasks.when_any({ t })\n\
         assert(ok and result == 'once', tostring(result))\n\
         assert(tasks.ready(t), 'a delivered task is ready')\n\
         local ok2, err = pcall(tasks.when_any, { t })\n\
         assert(not ok2, 'the second wait fails')\n\
         return err.kind .. '|' .. err.task .. '|' .. tostring(err)",
        &[("Child", "return 'once'")],
    );
    let (out, _) = drive(&md).await;
    let out = out.expect("the caught error ends the run normally");
    let (kind, rest) = out.split_once('|').expect("kind|task|message");
    let (task_field, message) = rest.split_once('|').expect("task|message");
    assert_eq!(kind, "task_consumed");
    assert_eq!(task_field, "0.0");
    assert!(
        message.contains("0.0"),
        "the message names the consumed task: {message}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_ends_a_parked_task_idempotently_and_reports_task_cancelled_once() {
    // The child parks on a store write; the owner cancels it twice (the
    // second is a no-op), reads the terminal state, and ends with no live
    // task - so no `tasks_live`. A wait on the cancelled slot delivers
    // `ok = false` with a `cancelled` error value.
    let md = tasks_prompt(
        "local t = tasks.spawn('## Child')\n\
         store.write('park', 'x')\n\
         tasks.cancel(t)\n\
         tasks.cancel(t)\n\
         local s = tasks.status(t)\n\
         log('state=' .. s.state .. ' ok=' .. tostring(s.ok))\n\
         assert(tasks.ready(t), 'a cancelled task is ready')\n\
         local _, ok, err = tasks.when_any({ t })\n\
         assert(not ok and err.kind == 'cancelled', tostring(err))\n\
         assert(err.task == t.task, tostring(err.task))\n\
         assert(err.reason == nil, 'a cancelled delivery carries no reason field')\n\
         assert(#tasks.pending() == 0, 'nothing is pending')\n\
         return 'done'",
        &[("Child", "store.write('child-park', 'x')\nreturn 'never'")],
    );
    let prompt = parse(&md);
    let recorder = Arc::new(WaitRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = Scheduler::new(&ctx, None);
    let out = scheduler
        .drive()
        .await
        .expect("a cancelled task is not a leaked one");
    assert_eq!(out, "done");
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Cancelled)
    );
    assert_eq!(
        recorder.logs("Main"),
        vec!["state=cancelled ok=false".to_owned()]
    );
    let records = recorder.records();
    assert_eq!(
        records
            .iter()
            .filter(|(section, event)| {
                section == "Child" && *event == Observation::TaskCancelled { task: task("0.0") }
            })
            .count(),
        1,
        "the cancellation reports exactly once under the target: {records:?}"
    );
    assert!(
        !records.iter().any(|(_, event)| matches!(
            event,
            Observation::TaskSucceeded { .. }
                | Observation::TaskFailed { .. }
                | Observation::TaskAbandoned { .. }
        )),
        "a cancelled task reports no other terminal event: {records:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pending_lists_the_callers_live_tasks_in_spawn_order() {
    // Two live children and one finished: `pending` names the live two in
    // spawn order as handles every `tasks.*` accepts, and the origin filter
    // narrows to the author's own.
    let md = tasks_prompt(
        "local a = tasks.spawn('## Parked')\n\
         local b = tasks.spawn('## Quick')\n\
         local c = tasks.spawn('## Parked')\n\
         tasks.when_any({ b })\n\
         local live = tasks.pending()\n\
         assert(#live == 2, 'two live tasks, got ' .. #live)\n\
         assert(live[1].task == a.task and live[2].task == c.task, live[1].task .. ',' .. live[2].task)\n\
         assert(#tasks.pending({ origin = 'author' }) == 2, 'both are author tasks')\n\
         assert(#tasks.pending({ origin = 'model' }) == 0, 'no model tasks')\n\
         for _, t in ipairs(live) do tasks.cancel(t) end\n\
         return 'done'",
        &[
            (
                "Parked",
                "store.write('park-' .. sys.id, 'x')\nreturn 'never'",
            ),
            ("Quick", "return 'quick'"),
        ],
    );
    let (out, _) = drive(&md).await;
    assert_eq!(out.expect("the cancelled tasks do not leak"), "done");
}

#[tokio::test(flavor = "current_thread")]
async fn the_wait_shims_validate_their_arguments_at_the_call_site() {
    let md = tasks_prompt(
        "local ok1, e1 = pcall(tasks.when_any, {})\n\
         assert(not ok1 and e1.kind == 'lua', tostring(e1))\n\
         local ok2, e2 = pcall(tasks.when_any, 'nope')\n\
         assert(not ok2 and e2.kind == 'lua', tostring(e2))\n\
         local ok3, e3 = pcall(tasks.cancel, 42)\n\
         assert(not ok3 and e3.kind == 'lua', tostring(e3))\n\
         local ok4, e4 = pcall(tasks.status, 'not-an-id')\n\
         assert(not ok4 and e4.kind == 'lua', tostring(e4))\n\
         local ok5, e5 = pcall(tasks.note, 7)\n\
         assert(not ok5 and e5.kind == 'lua', tostring(e5))\n\
         local ok6, e6 = pcall(tasks.pending, { origin = 'robot' })\n\
         assert(not ok6 and e6.kind == 'lua', tostring(e6))\n\
         return tostring(e1) .. '|' .. tostring(e2) .. '|' .. tostring(e3) .. '|' .. tostring(e4)\n\
           .. '|' .. tostring(e5) .. '|' .. tostring(e6)",
        &[],
    );
    let (out, _) = drive(&md).await;
    let out = out.expect("every argument error is caught at the call site");
    let parts: Vec<&str> = out.split('|').collect();
    assert_eq!(parts.len(), 6, "{out}");
    assert!(parts[0].contains("at least one task"), "{out}");
    assert!(parts[1].contains("set of tasks"), "{out}");
    assert!(parts[2].contains("Task handle or task id"), "{out}");
    assert!(parts[3].contains("not-an-id"), "{out}");
    assert!(parts[4].contains("must be a string"), "{out}");
    assert!(parts[5].contains("robot"), "{out}");
}
