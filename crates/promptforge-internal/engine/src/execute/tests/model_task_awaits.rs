//! Tests for the model's `await_tasks` against its timer: a notice already
//! queued when the call arrives (a task that ended during the round that
//! issued it) is the answer at once, so nothing parks and no timer is
//! allocated even with a live sibling and a timeout given; and when a
//! member ends before a given timeout fires, the wake cancels the unfired
//! timer (its slot is `Cancelled`, not `Running`, `Done`, or `Abandoned`)
//! and no late firing follows. A scripted mock gateway plays the model.

use promptforge_parser::test_support::prompt;

use super::model_task_notices::{
    NoticeRecorder, calls_delayed_tool, delayed_call_from, delayed_task_scenario, loop_owner,
};
use super::model_tasks::{CHILD_PARKED_ON_SLOW_TOOL, model_task_scenario};
use super::scheduler::is_timer;
use super::*;
use crate::execute::scheduler::test_hooks::TaskState;
use crate::test_support::rig::Rig;

#[test]
fn await_tasks_answers_at_once_when_a_notice_is_already_pending() {
    // Round 1 starts `Parked`, which never ends. Round 2 starts `Quick`,
    // which ends after round 3's dispatch took the notice queue, so its
    // notice is queued when round 3's `await_tasks` arrives with `Parked`
    // live and a 30s timeout. The call answers with the queued notice
    // instead of parking: no timer is allocated (the owner's third child,
    // 0.2, never exists) and the run does not wait on `Parked` or the
    // timeout.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Parked\"}")]),
        calls(&[("call_2", "task", "{\"target\":\"## Quick\"}")]),
        calls(&[("call_3", "await_tasks", "{\"timeout\":30}")]),
        text("bye"),
    ]);
    let md = prompt(
        "",
        &[
            ("Only", &loop_owner("return msgs[7].content")),
            ("Parked", CHILD_PARKED_ON_SLOW_TOOL),
            ("Quick", "return 'quick result'"),
        ],
    );
    let mut rig = Rig::run(model_task_scenario(md).chat(&gateway));
    let out = rig.trace().text();
    assert!(
        out.starts_with("Task id=0.1 (## Quick) completed: ") && out.contains("quick result"),
        "the queued notice is the call's answer: {out}"
    );
    assert_eq!(
        rig.task_state_for_test(&task("0.2")),
        None,
        "a call answered from the queue allocates no timer"
    );
    assert_eq!(
        rig.task_state_for_test(&task("0.0")),
        Some(TaskState::Abandoned),
        "the live sibling was never waited on; the owner's end abandoned it"
    );
    let round_4 = gateway.requests()[3].messages.len();
    assert_eq!(
        round_4, 7,
        "the notice was consumed by the call, not drained again into round 4"
    );
}

#[test]
fn await_tasks_cancels_the_timer_when_a_member_ends_first() {
    // The child parks on a held delayed-tool call under a 1s timeout,
    // whose timer the rig holds too, as it holds the model's reply round.
    // Releasing the child's call ends it first; then the timer is released
    // while the reply is still held, so an uncancelled timer would fire
    // inside the run. The wake cancelled it: its slot (the owner's second
    // child, 0.1) is `Cancelled` when the run ends, never `Done` (fired
    // late) or `Abandoned` (still running at the owner's end), and the
    // transcript shows one wake.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_2", "await_tasks", "{\"timeout\":1}")]),
        text("bye"),
    ]);
    let md = prompt(
        "",
        &[
            (
                "Only",
                &loop_owner("return msgs[5].content .. '|' .. #msgs"),
            ),
            (
                "Child",
                "tools.call('tools/delayed', { from = 'Child' })\nreturn 'child result'",
            ),
        ],
    );
    // The reply round is the one that sends the start, the wait, and both
    // answers after the user turn.
    let is_reply =
        |effect: &Effect| matches!(effect, Effect::Chat { messages, .. } if messages.len() == 5);
    let mut rig = Rig::start(
        delayed_task_scenario(md)
            .chat(&gateway)
            .hold(move |effect| calls_delayed_tool(effect) || is_timer(effect) || is_reply(effect)),
    );
    rig.settle();
    rig.release(delayed_call_from("Child"));
    rig.settle();
    rig.release(is_timer);
    rig.settle();
    rig.release(is_reply);
    let mut rig = rig.finish();
    let out = rig.trace().text();
    assert!(
        out.starts_with("Task id=0.0 (## Child) completed: ") && out.ends_with("|6"),
        "the member's end answers the wait once and the reply follows: {out}"
    );
    assert_eq!(
        rig.task_state_for_test(&task("0.1")),
        Some(TaskState::Cancelled),
        "the member's win cancelled the unfired timer"
    );
    assert_eq!(
        rig.task_state_for_test(&task("0.0")),
        Some(TaskState::Done { ok: true }),
        "the member's own slot holds its outcome; the notice delivered it"
    );
    let notices = NoticeRecorder::replayed(rig.trace()).notices();
    assert_eq!(
        notices.len(),
        1,
        "one notice was queued and read: {notices:?}"
    );
}
