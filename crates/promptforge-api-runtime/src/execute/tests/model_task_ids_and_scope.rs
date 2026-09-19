//! Checkpoint acceptance tests for model-task identity and scope, driven
//! end to end by a scripted mock model: task ids, their targets, and the
//! owner's entry id are byte-identical across two runs whose tasks finish
//! in opposite orders (every id is allocated at spawn from the owner's
//! local counter, never from completion order); and a `task` call in a
//! section without `tools.allow_tasks` is refused by the round's scope
//! gate as `out_of_scope_tool` before the built-in arm can start anything.
//! The delivery and lifecycle acceptance cases, and the helpers used here,
//! are in `model_task_acceptance`.

use std::time::Duration;

use promptforge_api_types::ids::TaskId;

use super::model_task_acceptance::{
    LATER, SOON, count_under, model_starts, task_events, two_child_prompt,
};
use super::model_task_notices::{DelayedBroker, NoticeRecorder, loop_owner};
use super::model_tasks::{NeverBroker, model_task_context_with, owner_prompt, task};
use super::*;
use crate::execute::tokio_driver::TokioDriver;

/// Drives the two-task prompt with `A` released after `delays[0]` and `B`
/// after `delays[1]`, and returns the run's output (the owner's `sys.id`
/// and the task ids the two waits answered with, in that order), the
/// model-origin starts, and the order in which the tasks succeeded.
async fn ordered_run(delays: [Duration; 2]) -> (String, Vec<(TaskId, String)>, Vec<TaskId>) {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## A\"}"),
        resp_tool_call("call_2", "task", "{\"target\":\"## B\"}"),
        resp_tool_call("call_3", "await_tasks", "{}"),
        resp_tool_call("call_4", "await_tasks", "{}"),
        resp_text("bye"),
    ])
    .await;
    let md = two_child_prompt(
        &loop_owner(
            "local first = msgs[7].content:match('^Task id=(%S+)')\n\
             local second = msgs[9].content:match('^Task id=(%S+)')\n\
             return sys.id .. '|' .. first .. '|' .. second",
        ),
        ("A", "user_input()\nreturn sys.id"),
        ("B", "user_input()\nreturn sys.id"),
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        DelayedBroker::new(&delays),
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("both tasks end inside the two waits");
    let records = recorder.events();
    let succeeded = task_events(&records)
        .into_iter()
        .filter(|(label, _)| *label == "succeeded")
        .map(|(_, task)| task)
        .collect();
    (out, model_starts(&records), succeeded)
}

#[tokio::test(flavor = "current_thread")]
async fn ids_are_identical_across_runs_whose_model_tasks_finish_in_different_orders() {
    // Run one releases `B` first, run two releases `A` first. The wait
    // answers and the success orders prove the finish orders differ; the
    // task ids, their targets, and the owner's entry id are byte-identical
    // because every id is allocated at spawn from the owner's local
    // counter, never from completion order.
    let (first_out, first_starts, first_succeeded) = ordered_run([LATER, SOON]).await;
    let (second_out, second_starts, second_succeeded) = ordered_run([SOON, LATER]).await;

    assert_eq!(
        first_succeeded,
        vec![task("0.1"), task("0.0")],
        "run one: B ends before A"
    );
    assert_eq!(
        second_succeeded,
        vec![task("0.0"), task("0.1")],
        "run two: A ends before B"
    );
    let starts = vec![(task("0.0"), "A".to_owned()), (task("0.1"), "B".to_owned())];
    assert_eq!(first_starts, starts, "run one: ids follow spawn order");
    assert_eq!(
        second_starts, starts,
        "run two: the same ids for the same spawns"
    );
    let (first_owner, first_waits) = first_out.split_once('|').expect("owner|first|second");
    let (second_owner, second_waits) = second_out.split_once('|').expect("owner|first|second");
    assert_eq!(
        first_owner, second_owner,
        "finish order must not change the owner's id"
    );
    assert_eq!(first_waits, "0.1|0.0", "run one's waits answered B then A");
    assert_eq!(second_waits, "0.0|0.1", "run two's waits answered A then B");
}

#[tokio::test(flavor = "current_thread")]
async fn a_task_call_without_an_allowlist_is_refused_by_the_scope_gate() {
    // Without `tools.allow_tasks` the round advertises nothing, so a
    // `task` call is a name outside the round's scope: the chat arm fails
    // the round as `out_of_scope_tool` naming the call, under one failed
    // tool-call observation, before the built-in arm can start anything.
    // The loop raises at the call site with nothing appended.
    let gateway = ScriptedGateway::start(vec![resp_tool_call(
        "call_1",
        "task",
        "{\"target\":\"## Child\"}",
    )])
    .await;
    let md = owner_prompt(
        "",
        "local msgs = messages.new()\n\
         msgs:user('go')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         assert(not ok, 'a call outside the round scope raises')\n\
         return err.kind .. '|' .. err.name .. '|' .. #msgs",
        "return 'never started'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(NoticeRecorder::default());
    let ctx = model_task_context_with(
        &prompt,
        Arc::clone(&recorder) as Arc<dyn Observer>,
        Arc::new(NeverBroker),
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");

    assert_eq!(
        out, "out_of_scope_tool|task|1",
        "the gate names the call and the loop appended nothing"
    );
    assert_eq!(
        gateway.requests().len(),
        1,
        "the failed round is the only round"
    );
    assert!(
        gateway.requests()[0]["tools"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "no allowlist, nothing advertised: {:?}",
        gateway.requests()[0]
    );
    let records = recorder.events();
    assert!(
        model_starts(&records).is_empty(),
        "the gate refuses before the arm starts anything: {records:?}"
    );
    assert_eq!(
        count_under(&records, "Only", &Observation::ToolCallFailed),
        1,
        "the rejected call is one failed tool call: {records:?}"
    );
}
