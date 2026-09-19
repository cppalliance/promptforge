//! Tests for the model's `await_tasks` against its timer: a notice already
//! queued when the call arrives (a task that ended during the round that
//! issued it) is the answer at once, so nothing parks and no timer is
//! allocated even with a live sibling and a timeout given; and when a
//! member ends before a given timeout fires, the wake cancels the unfired
//! timer (its slot is `Cancelled`, not `Running`, `Done`, or `Abandoned`)
//! and no late firing follows. A scripted mock gateway plays the model.

use std::time::Duration;

use super::model_task_notices::{DelayedBroker, NoticeRecorder, loop_owner};
use super::model_tasks::{NeverBroker, PARKED_CHILD, model_task_context_with, owner_prompt, task};
use super::*;
use crate::execute::scheduler::TaskState;

#[tokio::test(flavor = "current_thread")]
async fn await_tasks_answers_at_once_when_a_notice_is_already_pending() {
    // Round 1 starts `Parked`, which never ends. Round 2 starts `Quick`,
    // which ends between the shim's drain and the round-3 chat, so its
    // notice is queued when round 3's `await_tasks` arrives with `Parked`
    // live and a 30s timeout. The call answers with the queued notice
    // instead of parking: no timer is allocated (the owner's third child,
    // 0.2, never exists) and the run does not wait on `Parked` or the
    // timeout.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Parked\"}"),
        resp_tool_call("call_2", "task", "{\"target\":\"## Quick\"}"),
        resp_tool_call("call_3", "await_tasks", "{\"timeout\":30}"),
        resp_text("bye"),
    ])
    .await;
    let md = format!(
        "---\nname: mt\ndescription: d\npromptforge: 0\n---\n\n\
         # ModelTasks\n\n\
         ## Only\n\n\
         ```lua\n{}\n```\n\n\
         ## Parked\n\n\
         ```lua\n{PARKED_CHILD}\n```\n\n\
         ## Quick\n\n\
         ```lua\nreturn 'quick result'\n```\n",
        loop_owner("return msgs[7].content")
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    );
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = tokio::time::timeout(Duration::from_secs(5), scheduler.drive())
        .await
        .expect("a pending notice answers the call without a wait")
        .expect("the owner's end abandons the parked model task quietly");
    assert!(
        out.starts_with("Task id=0.1 (## Quick) completed: ") && out.contains("quick result"),
        "the queued notice is the call's answer: {out}"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.2")),
        None,
        "a call answered from the queue allocates no timer"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned),
        "the live sibling was never waited on; the owner's end abandoned it"
    );
    let round_4 = gateway.requests()[3]["messages"]
        .as_array()
        .expect("messages")
        .len();
    assert_eq!(
        round_4, 7,
        "the notice was consumed by the call, not drained again into round 4"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn await_tasks_cancels_the_timer_when_a_member_ends_first() {
    // The child ends at 300ms under a 1s timeout, and the model's reply
    // round is held for 1.2s, so an uncancelled timer would fire inside
    // the run. The wake cancels it: its slot (the owner's second child,
    // 0.1) is `Cancelled` when the run ends, never `Done` (fired late) or
    // `Abandoned` (still running at the owner's end), and the transcript
    // shows one wake.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "await_tasks", "{\"timeout\":1}"),
        resp_delayed_text("bye", Duration::from_millis(1200)),
    ])
    .await;
    let md = owner_prompt(
        "",
        &loop_owner("return msgs[5].content .. '|' .. #msgs"),
        "user_input()\nreturn 'child result'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        DelayedBroker::new(&[Duration::from_millis(300)]),
    );
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = tokio::time::timeout(Duration::from_secs(5), scheduler.drive())
        .await
        .expect("the run does not wait out the cancelled timer")
        .expect("a cancelled timer is not a leaked task");
    assert!(
        out.starts_with("Task id=0.0 (## Child) completed: ") && out.ends_with("|6"),
        "the member's end answers the wait once and the reply follows: {out}"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.1")),
        Some(TaskState::Cancelled),
        "the member's win cancelled the unfired timer"
    );
    assert_eq!(
        scheduler.task_state_for_test(&task("0.0")),
        Some(TaskState::Done),
        "the member's own slot holds its outcome; the notice carried it"
    );
    assert_eq!(
        recorder.notices().len(),
        1,
        "one notice was queued and read: {:?}",
        recorder.notices()
    );
}
