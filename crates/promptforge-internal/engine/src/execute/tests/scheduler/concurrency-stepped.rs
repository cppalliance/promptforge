//! Admission cases that pace the run's answers by hand: two tasks' `call`
//! children finishing in any order under every answer batching, and a
//! queue that can never be admitted reported as a stall.

use super::*;
use crate::execute::RunResult;
use crate::test_support::rig::Rig;
use crate::test_support::scenario::{Batching, drive_batched};

#[test]
fn two_tasks_whose_call_children_finish_in_any_order_both_complete() {
    // Both tasks are admitted together and each calls `Leaf`, whose round
    // is issued in call order: A's child first, then B's. Answered in
    // issue order, A's child finishes while B's is still running; answered
    // in reverse, B's finishes first. Every order completes both calls.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Calls\n\n\
        ## Main\n\n\
        ```lua\n\
        var.who = 'a'\n\
        local a = tasks.spawn('### Caller')\n\
        var.who = 'b'\n\
        local b = tasks.spawn('### Caller')\n\
        local results = tasks.join({ a, b })\n\
        return results[1].result .. '|' .. results[2].result\n\
        ```\n\n\
        ### Caller\n\n\
        ```lua\nreturn call('#### Leaf')\n```\n\n\
        #### Leaf\n\n\
        ```lua\nreturn models.infer(var.who)\n```\n";
    for batching in [
        Batching::OnePerStep,
        Batching::AllAtOnce,
        Batching::Reversed,
    ] {
        let run = scheduler_scenario(md).limits(ceiling(2)).into_run();
        let outcome = drive_batched(run, batching);
        match &outcome.result {
            RunResult::Ok(text) => assert_eq!(text, "r(a)|r(b)", "under {batching:?}"),
            other => panic!("both calls complete under {batching:?}: {other:?}"),
        }
    }
}

#[test]
fn a_queued_task_that_can_never_be_admitted_is_reported_as_a_stall() {
    // The walk parks on its store write, and the test fills the walk's one
    // slot with a task that never ends. Once the write is answered the
    // walk spawns its child and parks on the join: nothing is ready,
    // nothing is in flight, and no slot will ever admit the queued child.
    // The step must end the run with a stall report rather than return
    // Pending with nothing to answer.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Limits\n\n\
        ## Main\n\n\
        ```lua\n\
        store.write('park', 'x')\n\
        local t = tasks.spawn('## Child')\n\
        tasks.join_any({ t })\n\
        return 'never'\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\nreturn 'child'\n```\n";
    // The rig answers the store write in the step that issues it, so the
    // wedge lands before the walk resumes from the write and spawns.
    let mut rig = Rig::start(scheduler_scenario(md).limits(ceiling(1)));
    assert!(rig.step(), "the walk parks on its store write");
    rig.scheduler_for_test().wedge_admission_for_test(0);
    let rig = rig.finish();
    assert_eq!(
        rig.issued().len(),
        1,
        "a stalled run must not return Pending with more effects: {:?}",
        rig.issued()
    );
    match rig.trace().result() {
        RunResult::Failure(error) => assert!(
            error.to_string().contains("stalled"),
            "the run reports the stall: {error}"
        ),
        result => panic!("the stalled run must fail, got {result:?}"),
    }
}
