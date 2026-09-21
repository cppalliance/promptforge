//! Checkpoint acceptance tests for model tasks, driven end to end by a
//! scripted mock model: one task's whole lifecycle reads as a single
//! transcript (start, status, notice ahead of the next round, reply) with
//! exactly one terminal per started task; the author adopts a model task
//! through `tasks.pending({ origin = "model" })` and collects its result,
//! so the task is delivered and never abandoned; two waits deliver two
//! notices once each, in finish order; and a timed-out wait followed by
//! the model's own cancel leaves the task cancelled with no notice and no
//! abandonment. The id-determinism and scope-gate acceptance cases are in
//! `model_task_ids_and_scope`, which shares the helpers here. Each
//! built-in's own answers, the notice texts, the timer cases, and a
//! sibling chain stepping during a parked wait are pinned in
//! `model_tasks`, `model_task_answers`, `model_task_awaits`, and
//! `model_task_notices`.

use std::collections::BTreeMap;
use std::time::Duration;

use promptforge_api_types::ids::{TaskId, TaskOrigin};

use super::model_task_notices::{DelayedBroker, NoticeRecorder, loop_owner};
use super::model_tasks::{NeverBroker, PARKED_CHILD, model_task_context_with, owner_prompt, task};
use super::*;
use crate::execute::scheduler::test_hooks::TaskState;

/// A broker delay that orders one child's end against another's. The
/// scripted rounds between them complete in milliseconds on the loopback
/// gateway, so the margin is wide; a test's wall time is its longest delay.
pub(super) const SOON: Duration = Duration::from_millis(300);
pub(super) const LATER: Duration = Duration::from_millis(900);

/// Every task observation in `records`, as `(label, task id)` pairs in
/// order, so a test can pair each started task with its terminals.
pub(super) fn task_events(records: &[(String, Observation)]) -> Vec<(&'static str, TaskId)> {
    records
        .iter()
        .filter_map(|(_, observation)| match observation {
            Observation::TaskStarted { task, .. } => Some(("started", task.clone())),
            Observation::TaskSucceeded { task } => Some(("succeeded", task.clone())),
            Observation::TaskFailed { task } => Some(("failed", task.clone())),
            Observation::TaskCancelled { task } => Some(("cancelled", task.clone())),
            Observation::TaskAbandoned { task, .. } => Some(("abandoned", task.clone())),
            _ => None,
        })
        .collect()
}

/// The terminal labels recorded per started task, in order. Every started
/// task appears (with an empty list when it has no terminal); a terminal
/// for a task that never started, or a second start, fails the test.
pub(super) fn terminals_per_started_task(
    records: &[(String, Observation)],
) -> BTreeMap<TaskId, Vec<&'static str>> {
    let events = task_events(records);
    let mut terminals: BTreeMap<TaskId, Vec<&'static str>> = BTreeMap::new();
    for (label, task) in &events {
        if *label == "started" {
            assert!(
                terminals.insert(task.clone(), Vec::new()).is_none(),
                "task {task} started twice: {events:?}"
            );
        }
    }
    for (label, task) in &events {
        if *label != "started" {
            terminals
                .get_mut(task)
                .unwrap_or_else(|| {
                    panic!("task {task} reported `{label}` without starting: {events:?}")
                })
                .push(label);
        }
    }
    terminals
}

/// The `(task, target)` pair of every model-origin `TaskStarted`, in order.
pub(super) fn model_starts(records: &[(String, Observation)]) -> Vec<(TaskId, String)> {
    records
        .iter()
        .filter_map(|(_, observation)| match observation {
            Observation::TaskStarted {
                task,
                origin: TaskOrigin::Model,
                target,
                ..
            } => Some((task.clone(), target.clone())),
            _ => None,
        })
        .collect()
}

/// The number of messages the gateway saw in its `round`th (0-based)
/// request.
fn message_count(gateway: &ScriptedGateway, round: usize) -> usize {
    gateway.requests()[round]["messages"]
        .as_array()
        .expect("a chat request includes messages")
        .len()
}

/// The count of `event` recorded under `section`.
pub(super) fn count_under(
    records: &[(String, Observation)],
    section: &str,
    event: &Observation,
) -> usize {
    records
        .iter()
        .filter(|(seen, observation)| seen == section && observation == event)
        .count()
}

/// A three-section prompt: `Only` runs `owner_body`; the two children are
/// the model's task targets, each `(heading, body)`.
pub(super) fn two_child_prompt(
    owner_body: &str,
    first: (&str, &str),
    second: (&str, &str),
) -> String {
    format!(
        "---\nname: mt\ndescription: d\npromptforge: 0\n---\n\n\
         # ModelTasks\n\n\
         ## Only\n\n\
         ```lua\n{owner_body}\n```\n\n\
         ## {}\n\n\
         ```lua\n{}\n```\n\n\
         ## {}\n\n\
         ```lua\n{}\n```\n",
        first.0, first.1, second.0, second.1
    )
}

#[tokio::test(flavor = "current_thread")]
async fn one_model_task_reads_as_a_single_transcript_with_one_terminal() {
    // Round 1 starts the task; the child ends between round 2's drain and
    // its chat, so round 2's status read sees it done and round 3 includes
    // its notice as a user record ahead of the reply. The author's list
    // holds the whole exchange in order, each tool record correlated to
    // its call, and the task starts once and succeeds once.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "task_status", "{\"id\":\"0.0\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        &loop_owner(
            "assert(msgs[3].tool_call_id == 'call_1', 'the start answers its call')\n\
             assert(msgs[5].tool_call_id == 'call_2', 'the status answers its call')\n\
             local roles = {}\n\
             for i = 1, #msgs do roles[i] = msgs[i].role end\n\
             return table.concat(roles, ',') .. '|' .. msgs[5].content .. '|' .. msgs[6].content",
        ),
        "return 'child result'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    );
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler
        .drive()
        .await
        .expect("a completed model task leaves a clean owner");

    // A notice's nonce-wrapped result spans lines, so the pieces are split
    // on a separator no piece contains.
    let lines: Vec<&str> = out.split('|').collect();
    assert_eq!(lines.len(), 3, "roles, status, notice: {out}");
    assert_eq!(
        lines[0], "user,assistant,tool,assistant,tool,user,assistant",
        "the transcript is the user turn, two answered calls, the notice, and the reply"
    );
    assert!(
        lines[1].starts_with("Task id=0.0 (## Child): done, ok"),
        "the status read in round 2 sees the finished task: {}",
        lines[1]
    );
    assert!(
        lines[2].starts_with("Task id=0.0 (## Child) completed: ")
            && lines[2].contains("child result"),
        "the notice ahead of round 3 reports the task's result: {}",
        lines[2]
    );
    assert_eq!(gateway.requests().len(), 3);
    assert_eq!(message_count(&gateway, 1), 3, "round 2 predates the notice");
    assert_eq!(message_count(&gateway, 2), 6, "round 3 holds the notice");
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Done),
        "the model's task holds its outcome; the notice delivered it"
    );
    let records = recorder.events();
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([(task("0.0"), vec!["succeeded"])]),
        "one start, one terminal: {:?}",
        task_events(&records)
    );
    assert_eq!(
        count_under(&records, "Only", &Observation::ToolCallSucceeded),
        2,
        "the start and the status read are each one succeeded call: {records:?}"
    );
    assert_eq!(
        recorder.notices().len(),
        1,
        "one notice was queued and read"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_author_adopts_a_model_task_and_collects_its_result() {
    // The model starts a task and replies without waiting on it. The
    // author finds it through the model-origin filter, waits on it as its
    // own, and reads the raw result. The slot is delivered (not abandoned
    // at the owner's end), the notice was queued when the task ended but
    // no round ever read it, and the author's own filter stays empty.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        &loop_owner(
            "assert(#tasks.pending({ origin = 'author' }) == 0, 'the author started nothing')\n\
             local adopted = tasks.pending({ origin = 'model' })\n\
             assert(#adopted == 1, 'one model task is live: ' .. #adopted)\n\
             local results = tasks.when_all(adopted)\n\
             return tostring(results[1].ok) .. '|' .. results[1].result .. '|' .. #msgs",
        ),
        "user_input()\nreturn 'child result'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        DelayedBroker::new(&[SOON]),
    );
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler
        .drive()
        .await
        .expect("an adopted task is neither leaked nor abandoned");

    assert_eq!(
        out, "true|child result|4",
        "the author reads the task's raw result; the model's list ends at the reply"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Delivered),
        "the author's wait consumed the outcome"
    );
    assert_eq!(
        gateway.requests().len(),
        2,
        "the model never ran a round after the reply"
    );
    let records = recorder.events();
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([(task("0.0"), vec!["succeeded"])]),
        "the adopted task succeeds once and is never abandoned: {:?}",
        task_events(&records)
    );
    let notices = recorder.notices();
    assert_eq!(
        notices.len(),
        1,
        "the completion notice was queued: {notices:?}"
    );
    assert!(
        notices[0]
            .2
            .starts_with("Task id=0.0 (## Child) completed: "),
        "the queued notice names the completion even though no round read it: {}",
        notices[0].2
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_waits_deliver_two_notices_once_each_in_finish_order() {
    // Two tasks, `Quick` ending at 300ms and `Slow` at 900ms, then two
    // waits with no timeout. The first wait answers with `Quick`'s notice
    // alone (the wait wakes on the first end, not on both), the second
    // with `Slow`'s alone (a delivered notice is never drained again), and
    // the reply round holds only the answered calls. Neither wait
    // allocates a timer.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Quick\"}"),
        resp_tool_call("call_2", "task", "{\"target\":\"## Slow\"}"),
        resp_tool_call("call_3", "await_tasks", "{}"),
        resp_tool_call("call_4", "await_tasks", "{}"),
        resp_text("bye"),
    ])
    .await;
    let md = two_child_prompt(
        &loop_owner("return msgs[7].content .. '|' .. msgs[9].content .. '|' .. #msgs"),
        ("Quick", "user_input()\nreturn 'quick result'"),
        ("Slow", "user_input()\nreturn 'slow result'"),
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        DelayedBroker::new(&[SOON, LATER]),
    );
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler
        .drive()
        .await
        .expect("both tasks end inside the two waits");

    let lines: Vec<&str> = out.split('|').collect();
    assert_eq!(lines.len(), 3, "two wait answers and the count: {out}");
    assert!(
        lines[0].starts_with("Task id=0.0 (## Quick) completed: ") && !lines[0].contains("0.1"),
        "the first wait answers with the first end alone: {}",
        lines[0]
    );
    assert!(
        lines[1].starts_with("Task id=0.1 (## Slow) completed: ") && !lines[1].contains("0.0"),
        "the second wait answers with the second end alone: {}",
        lines[1]
    );
    assert_eq!(
        lines[2], "10",
        "one user turn, four answered calls, the reply"
    );
    assert_eq!(
        message_count(&gateway, 4),
        9,
        "the reply round drains no notice a wait already delivered"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Done)
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.1")),
        Some(TaskState::Done)
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.2")),
        None,
        "a wait without a timeout allocates no timer"
    );
    let records = recorder.events();
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([
            (task("0.0"), vec!["succeeded"]),
            (task("0.1"), vec!["succeeded"]),
        ]),
        "each task succeeds once: {:?}",
        task_events(&records)
    );
    assert_eq!(
        count_under(&records, "Only", &Observation::ToolCallSucceeded),
        4,
        "two starts and two waits are four succeeded calls: {records:?}"
    );
    assert_eq!(recorder.notices().len(), 2, "one notice per task end");
}

#[tokio::test(flavor = "current_thread")]
async fn a_timed_out_wait_then_the_models_cancel_leaves_the_task_cancelled_without_a_notice() {
    // The task never ends on its own. The wait's timer fires first and
    // names it as still running; the model then cancels it and reads the
    // confirmation. At the owner's end nothing is live, so nothing is
    // abandoned: the task's one terminal is `cancelled`, the fired timer
    // is an internal slot that starts nothing observable, and the model's
    // own cancel queues no notice.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "await_tasks", "{\"timeout\":0.1}"),
        resp_tool_call("call_3", "task_cancel", "{\"id\":\"0.0\"}"),
        resp_text("bye"),
    ])
    .await;
    let md = owner_prompt(
        "",
        &loop_owner(
            "assert(#tasks.pending() == 0, 'nothing is live after the cancel')\n\
             return msgs[5].content .. '|' .. msgs[7].content",
        ),
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    );
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler
        .drive()
        .await
        .expect("a cancelled task is not a leak");

    assert_eq!(
        out, "timed out; tasks 0.0 still running|Task id=0.0 cancelled",
        "the timeout names the task and the cancel confirms it"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Cancelled),
        "the model's cancel ended the task, not the owner's end"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.1")),
        Some(TaskState::Done),
        "the wait's timer fired"
    );
    let records = recorder.events();
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([(task("0.0"), vec!["cancelled"])]),
        "the task is cancelled once and the timer is never a started task: {:?}",
        task_events(&records)
    );
    assert!(
        recorder.notices().is_empty(),
        "the model read the confirmation; no notice repeats it: {:?}",
        recorder.notices()
    );
}
