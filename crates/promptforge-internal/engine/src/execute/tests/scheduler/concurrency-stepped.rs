//! Admission cases that step the run by hand instead of through the
//! tokio driver: two tasks' `call` children finishing in any order under
//! every answer batching, and a queue that can never be admitted
//! reported as a stall.

use super::super::super::serial_driver::{Batching, drive_batched, perform_locally};
use super::*;
use crate::execute::RunResult;
use crate::execute::run::{Run, Step};

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
    let prompt = parse(md);
    for batching in [
        Batching::OnePerStep,
        Batching::AllAtOnce,
        Batching::Reversed,
    ] {
        let (state, _harness) = limited_context(
            &prompt,
            &TestStore::new(),
            ceiling(2),
            Arc::new(NullObserver::default()),
        );
        let outcome = drive_batched(Run::from_state(state), batching);
        let RunResult::Ok(text) = &outcome.result else {
            panic!(
                "both calls complete under {batching:?}: {:?}",
                outcome.result
            );
        };
        assert_eq!(text, "r(a)|r(b)", "under {batching:?}");
    }
}

#[test]
fn a_queued_task_that_can_never_be_admitted_is_reported_as_a_stall() {
    // The walk parks on its store write, and the test wedges the walk's
    // limit at zero. Once the write is answered the walk spawns its child
    // and parks on the join: nothing is ready, nothing is in flight, and
    // no slot will ever admit the queued child. The step must end the run
    // with a stall report rather than return Pending with nothing to
    // answer.
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
    let prompt = parse(md);
    let (state, _harness) = limited_context(
        &prompt,
        &TestStore::new(),
        ceiling(1),
        Arc::new(NullObserver::default()),
    );
    let mut run = Run::from_state(state);
    let Step::Pending { effects, .. } = run.step() else {
        panic!("the walk parks on its store write");
    };
    run.scheduler_for_test().wedge_admission_for_test(0);
    for (id, _, effect) in effects {
        let answer = perform_locally(&effect, &mut |effect| {
            panic!("the fixture issues no model round: {effect:?}")
        });
        run.resume(id, answer);
    }
    match run.step() {
        Step::Done {
            result: RunResult::Failure(error),
            ..
        } => assert!(
            error.to_string().contains("stalled"),
            "the run reports the stall: {error}"
        ),
        Step::Done { result, .. } => panic!("the stalled run must fail, got {result:?}"),
        Step::Pending { effects, .. } => panic!(
            "a stalled run must not return Pending, got {} effects",
            effects.len()
        ),
    }
}
