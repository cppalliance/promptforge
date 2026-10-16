//! Fanout failure, cancel, and script tool-arm cases for the scheduler. The
//! script `tools.call` dispatch cases sit in `script_tools`.

use super::*;
use crate::execute::run::{EffectAnswer, EffectId};
use crate::execute::scheduler::test_hooks::TaskState;
use crate::execute::tests::context::task;
use crate::test_support::rig::Rig;

#[test]
fn two_arms_appending_one_path_boom_without_any_other_suspension() {
    // The arms append unordered: neither joins the other before the
    // fanout's own rounds, so the second append's claim check meets the
    // first's standing write claim no matter how the answers are ordered -
    // claims are never released during a run, so the conflict cannot
    // depend on timing.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = tasks.fanout('### Worker', {'alpha', 'beta'})\n\
        return table.concat(r, ',')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('log.txt', item .. ';')\n\
        return item\n\
        ```\n";
    let trace = scheduler_scenario(md).run();

    match trace.error().inner() {
        Error::Determinism(detail) => {
            assert!(detail.contains("log.txt"), "error was: {detail}");
        }
        other => panic!("expected the fatal determinism violation, got {other:?}"),
    }
    // The losing arm's append never reached the backend: exactly one arm's
    // append landed.
    let log = trace.read("log.txt").expect("one arm appended");
    assert!(
        log == "alpha;" || log == "beta;",
        "exactly one arm's append may land: {log:?}"
    );
}

#[test]
fn an_arm_rewriting_its_own_path_succeeds() {
    // The registry records (fanout token, arm index), so the same arm
    // writing the same path again is a rewrite, not a race.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = tasks.fanout('### Worker', {'only'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.write('own.txt', 'first')\n\
        store.write('own.txt', 'second')\n\
        return item\n\
        ```\n";
    let trace = scheduler_scenario(md).run();

    assert_eq!(trace.text(), "only");
    assert_eq!(trace.read("own.txt").expect("the arm wrote"), "second");
}

#[test]
fn sequential_fanouts_may_write_one_path() {
    // A later fanout takes a fresh write token, so its write overwrites the
    // earlier fanout's registry record instead of racing against it.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local a = tasks.fanout('### Worker', {'one'})\n\
        local b = tasks.fanout('### Worker', {'two'})\n\
        return b[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.write('seq.txt', item)\n\
        return item\n\
        ```\n";
    let trace = scheduler_scenario(md).run();

    assert_eq!(trace.text(), "two");
    assert_eq!(trace.read("seq.txt").expect("both fanouts wrote"), "two");
}

#[test]
fn fatal_arm_aborts_queued_siblings() {
    // With the ceiling at 1 the siblings stay queued, and once the first
    // arm fails fatally they are cancelled before admission - proven by
    // the store side-channel only the fatal arm ever wrote to, and by the
    // terminal observations: one FAILED, nothing else. The start event
    // fires at admission, so the queued siblings never report one.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\ntasks.fanout('### Worker', {'boom', 'beta', 'gamma'})\n```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('log.txt', item .. '\\n')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return item\n\
        ```\n";
    let trace = scheduler_scenario(md).limits(ceiling(1)).run();

    let error = trace.error().inner();
    assert!(
        !matches!(error, Error::Interrupted),
        "expected a fatal arm error, got {error}"
    );
    let log = trace.read("log.txt").expect("the fatal arm wrote its item");
    assert_eq!(log, "boom\n", "blocked siblings must never run: {log:?}");
    let recorder = recorded(&trace);
    assert_eq!(
        terminal_count(&trace, task_started),
        1,
        "only the fatal arm was ever started: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&trace, task_failed),
        1,
        "the fatal arm reports failed: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&trace, task_succeeded),
        0,
        "no arm succeeded: {:?}",
        recorder.events()
    );
}

#[test]
fn fatal_arm_aborts_an_in_flight_sibling() {
    // The sibling-abort port: the failing arm and a sibling parked on a
    // held infer are both live when the failure lands. The fanout shim
    // cancels the live sibling before it re-raises, so the cancel removes
    // the sibling from the pending table, drops its round, and fires the
    // sibling's TaskCancelled BEFORE the parent's chunk failure - a shim
    // that raised first would leak the sibling into `tasks_live`. The
    // sibling's round is never answered, so the run cannot have waited
    // on it.
    let gateway = ScriptedChat::new(vec![text("boom-answer")]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\ntasks.fanout('### Worker', {'boom', 'slow'})\n```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':1')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return a\n\
        ```\n";
    let rig = Rig::run(scheduler_scenario(md).chat(&gateway).hold(asks("slow:1")));
    let error = rig.trace().error().inner();

    assert!(
        !matches!(error, Error::Interrupted),
        "expected a fatal arm error, got {error}"
    );
    assert!(
        error.to_string().contains("fatal arm error"),
        "the arm's own error surfaces: {error}"
    );
    let recorder = recorded(rig.trace());
    assert_eq!(
        terminal_count(rig.trace(), task_failed),
        1,
        "the fatal arm reports failed: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(rig.trace(), task_cancelled),
        1,
        "the in-flight sibling reports cancelled: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(rig.trace(), task_succeeded),
        0,
        "no arm succeeded: {:?}",
        recorder.events()
    );
    let events = rig.trace().events();
    let cancelled_at = events
        .iter()
        .position(task_cancelled)
        .expect("the sibling's cancelled event fired");
    let parent_failed_at = events
        .iter()
        .position(|event| {
            matches!(event, Event::LuaChunkFailed { .. }) && event.section() == "Parent"
        })
        .expect("the parent's chunk failed on the fanout error");
    assert!(
        cancelled_at < parent_failed_at,
        "the sibling abort precedes the parent's resume with the error: {:?}",
        recorder.events()
    );
}

#[test]
fn a_caught_fanout_failure_lets_the_caller_continue() {
    // The fanout error is the call's answer resumed through the envelope,
    // so an author `pcall` catches it; the run then continues - including
    // past the answer the aborted sibling's round was already given in the
    // same wave, which the run must discard rather than fail on.
    let gateway = ScriptedChat::new(vec![
        text("boom-answer"),
        text("slow-answer"),
        text("after-answer"),
    ]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local ok, err = pcall(tasks.fanout, '### Worker', {'boom', 'slow'})\n\
        assert(not ok, 'the fatal arm error reaches the caller')\n\
        local a = models.infer('after')\n\
        return 'caught:' .. a\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':1')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return a\n\
        ```\n";
    let trace = scheduler_scenario(md).chat(&gateway).run();

    assert_eq!(trace.text(), "caught:after-answer");
    assert_eq!(
        request_prompts(&gateway),
        vec!["boom:1", "slow:1", "after"],
        "both arms dispatched before the failure, then the caller's own infer"
    );
}

#[test]
fn cancellation_while_suspended_in_a_fanout_arm_interrupts_the_run() {
    // Cancellation while suspended in an arm: both arms are parked on held
    // infers when the cancel lands between steps, so the run drops their
    // rounds and ends cancelled. The arms are task chains stranded by the
    // run's end: they started, and the run's end settles each with one
    // `abandoned` terminal naming the run's end - not a cancel or a
    // failure of the arm's own.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = tasks.fanout('### Worker', {'one', 'two'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn models.infer('hang ' .. item)\n```\n";
    let mut rig =
        Rig::start(scheduler_scenario(md).hold(|effect| matches!(effect, Effect::Chat { .. })));
    rig.settle();
    rig.cancel();
    let rig = rig.finish();

    assert!(
        matches!(rig.trace().result(), RunResult::Cancelled),
        "cancelling suspended arms must interrupt the run, got {:?}",
        rig.trace().result()
    );
    assert_eq!(
        issued_prompts(&rig),
        vec!["hang one", "hang two"],
        "both arms were suspended on their infers when the cancel landed"
    );
    let recorder = recorded(rig.trace());
    assert_eq!(
        terminal_count(rig.trace(), task_started),
        2,
        "both arms started: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(rig.trace(), task_cancelled) + terminal_count(rig.trace(), task_failed),
        0,
        "a stranded arm reports no cancel or failure of its own: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(rig.trace(), task_succeeded),
        0,
        "no arm succeeded: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(rig.trace(), task_abandoned_by_run_end),
        2,
        "the run's end settles each stranded arm once: {:?}",
        recorder.events()
    );
}

#[test]
fn a_spawn_failure_mid_fanout_cancels_the_queued_arms() {
    // With the chain-count bound shrunk so the second arm's spawn fails
    // while the shim spawns its arms up front, the shim must cancel the
    // arm it already started before it re-raises: the parent catches the
    // error exactly once, and nothing is left live for the chain-end leak
    // check. The first arm was still queued for admission - the spawn
    // failure lands before the drain admits anything - so it never runs
    // its block, and no TaskStarted or TaskCancelled fires for it: a task
    // that never ran reports no start and no terminal, though its slot
    // still reaches Cancelled.
    let gateway = ScriptedChat::new(vec![text("after-answer")]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local ok, err = pcall(tasks.fanout, '### Worker', {'one', 'two', 'three'})\n\
        assert(not ok, 'the spawn failure reaches the caller')\n\
        return 'caught:' .. models.infer('after')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn 'worked:' .. item\n```\n";
    let mut rig = Rig::start(scheduler_scenario(md).chat(&gateway));
    // The root walk chain is id 0 and the first arm id 1; the second arm's
    // start trips the bound.
    rig.scheduler_for_test().set_max_chains_for_test(2);
    let mut rig = rig.finish();

    assert_eq!(rig.trace().text(), "caught:after-answer");
    assert_eq!(
        request_prompts(&gateway),
        vec!["after"],
        "no arm ran an infer; only the caller's own request fired"
    );
    assert_eq!(
        rig.task_state_for_test(&task("0.0")),
        Some(TaskState::Cancelled),
        "the shim cancelled the spawned arm before it re-raised"
    );
    let recorder = recorded(rig.trace());
    assert_eq!(
        terminal_count(rig.trace(), task_started),
        0,
        "the arm was never admitted, so no start fired: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(rig.trace(), task_cancelled),
        0,
        "a task that never started reports no terminal: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(rig.trace(), task_succeeded),
        0,
        "no arm ran to completion: {:?}",
        recorder.events()
    );
    assert!(
        !recorder
            .events()
            .iter()
            .any(|(section, event)| section == "Worker"
                && event == &detail::LUA_CHUNK_STARTED.to_string()),
        "the cancelled arm never ran its block: {:?}",
        recorder.events()
    );
}

#[test]
fn an_answer_for_an_unknown_request_id_fails_loudly() {
    // An answer arriving for an id the run never issued (and that is not an
    // orphan) means the caller lost track of its effects: the run must fail
    // with Error::Internal rather than silently discard the answer. Only an
    // orphaned id (a fatal sibling's late I/O answer, covered by
    // `a_caught_fanout_failure_lets_the_caller_continue`) may be
    // discarded.
    let gateway = ScriptedChat::new(vec![text("real-answer")]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Infer\n\n\
        ## Only\n\n\
        ```lua\nreturn models.infer('ask')\n```\n";
    let mut rig = Rig::start(scheduler_scenario(md).chat(&gateway));
    // Handed to the run before its first step: the phantom answer lands
    // ahead of the real infer's, on a run that has issued nothing.
    rig.run_for_test()
        .resume(EffectId(u64::MAX), EffectAnswer::Timer);
    let rig = rig.finish();

    assert!(
        matches!(rig.trace().error().inner(), Error::Internal { message, .. } if message.contains("did not issue")),
        "the unknown answer is a loud invariant failure: {:?}",
        rig.trace().error()
    );
}

#[path = "failures-script-tools.rs"]
mod script_tools;
