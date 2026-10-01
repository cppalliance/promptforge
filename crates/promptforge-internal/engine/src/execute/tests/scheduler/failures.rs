//! Fanout failure, cancel, and script tool-arm cases for the scheduler. The
//! script `tools.call` dispatch cases sit in `script_tools`.

use std::num::NonZeroUsize;

use super::*;
use crate::execute::run::{EffectAnswer, EffectId};
use crate::execute::scheduler::test_hooks::TaskState;
use crate::test_support::tokio_driver::TokioDriver;

#[tokio::test(flavor = "current_thread")]
async fn two_arms_appending_one_path_boom_without_any_other_suspension() {
    // The arms append unordered: neither joins the other before the
    // fanout's own rounds, so the second append's claim check meets the
    // first's standing write claim no matter how the blocking pool orders
    // the two ops - claims are never released during a run, so the
    // conflict cannot depend on timing.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha', 'beta'})\n\
        return table.concat(r, ',')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('log.txt', item .. ';')\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("concurrent appends to one path must boom");

    match &error {
        Error::Determinism(detail) => {
            assert!(detail.contains("log.txt"), "error was: {detail}");
        }
        other => panic!("expected the fatal determinism violation, got {other:?}"),
    }
    // The losing arm's append never reached the backend: exactly one arm's
    // append landed.
    let log = store.read("log.txt").expect("one arm appended");
    assert!(
        log == "alpha;" || log == "beta;",
        "exactly one arm's append may land: {log:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_arm_rewriting_its_own_path_succeeds() {
    // The registry records (fanout token, arm index), so the same arm
    // writing the same path again is a rewrite, not a race.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'only'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.write('own.txt', 'first')\n\
        store.write('own.txt', 'second')\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("an arm rewriting its own path must succeed");

    assert_eq!(out, "only");
    assert_eq!(store.read("own.txt").expect("the arm wrote"), "second");
}

#[tokio::test(flavor = "current_thread")]
async fn sequential_fanouts_may_write_one_path() {
    // A later fanout takes a fresh write token, so its write overwrites the
    // earlier fanout's registry record instead of racing against it.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local a = fanout('### Worker', {'one'})\n\
        local b = fanout('### Worker', {'two'})\n\
        return b[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.write('seq.txt', item)\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a sequential fanout may write the same path");

    assert_eq!(out, "two");
    assert_eq!(store.read("seq.txt").expect("both fanouts wrote"), "two");
}

#[tokio::test(flavor = "current_thread")]
async fn fatal_arm_aborts_queued_siblings() {
    // With the ceiling at 1 the siblings stay queued, and once the first
    // arm fails fatally they are cancelled before admission - proven by
    // the store side-channel only the fatal arm ever wrote to, and by the
    // terminal observations: one FAILED, nothing else. The start event
    // fires at admission, so the queued siblings never report one.
    let store = TestStore::new();
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\nfanout('### Worker', {'boom', 'beta', 'gamma'})\n```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('log.txt', item .. '\\n')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_from(
        &prompt,
        &store,
        &test_context(EXECUTION)
            .limits(RunLimits::new().max_concurrency(NonZeroUsize::new(1).expect("1 is non-zero"))),
        RunHost::new().observer(recorder.clone()),
    );
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("a fatal arm must fail the whole fanout");

    assert!(
        !matches!(error, Error::Interrupted),
        "expected a fatal arm error, got {error}"
    );
    let log = store.read("log.txt").expect("the fatal arm wrote its item");
    assert_eq!(log, "boom\n", "blocked siblings must never run: {log:?}");
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        1,
        "only the fatal arm was ever started: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_FAILED),
        1,
        "the fatal arm reports failed: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        0,
        "no arm succeeded: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fatal_arm_aborts_an_in_flight_sibling() {
    // The sibling-abort port: the failing arm and a sibling parked on a
    // slow infer are both live when the failure lands. The fanout shim
    // cancels the live sibling before it re-raises, so the cancel removes
    // the sibling from the pending table, aborts its I/O task, and fires
    // the sibling's TaskCancelled BEFORE the parent's chunk failure - a
    // shim that raised first would leak the sibling into `tasks_live`.
    // The 30-second sibling answer and the timeout guard prove the driver
    // never waits on the aborted arm.
    let gateway = ScriptedGateway::start(vec![
        resp_text("boom-answer"),
        resp_delayed_text("slow-answer", std::time::Duration::from_secs(30)),
    ])
    .await;
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\nfanout('### Worker', {'boom', 'slow'})\n```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':1')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return a\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr()))).drive(),
    )
    .await
    .expect("the aborted sibling must not stall the driver");
    let error = result.expect_err("a fatal arm must fail the whole fanout");

    assert!(
        !matches!(error, Error::Interrupted),
        "expected a fatal arm error, got {error}"
    );
    assert!(
        error.to_string().contains("fatal arm error"),
        "the arm's own error surfaces: {error}"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_FAILED),
        1,
        "the fatal arm reports failed: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_CANCELLED),
        1,
        "the in-flight sibling reports cancelled: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        0,
        "no arm succeeded: {:?}",
        recorder.events()
    );
    let events = recorder.events();
    let cancelled_at = events
        .iter()
        .position(|(_, event)| event == TASK_CANCELLED)
        .expect("the sibling's cancelled event fired");
    let parent_failed_at = events
        .iter()
        .position(|(section, event)| {
            section == "Parent" && event == &detail::LUA_CHUNK_FAILED.to_string()
        })
        .expect("the parent's chunk failed on the fanout error");
    assert!(
        cancelled_at < parent_failed_at,
        "the sibling abort precedes the parent's resume with the error: {events:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_caught_fanout_failure_lets_the_caller_continue() {
    // The fanout error is the call's answer resumed through the envelope,
    // so an author `pcall` catches it; the run then continues - including
    // past a stale answer the aborted sibling's already-completed I/O task
    // may have posted, which the driver must discard rather than fail on.
    let gateway = ScriptedGateway::start(vec![
        resp_text("boom-answer"),
        resp_text("slow-answer"),
        resp_text("after-answer"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local ok, err = pcall(fanout, '### Worker', {'boom', 'slow'})\n\
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
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the caught fanout failure lets the caller continue");

    assert_eq!(out, "caught:after-answer");
    assert_eq!(
        request_prompts(&gateway),
        vec!["boom:1", "slow:1", "after"],
        "both arms dispatched before the failure, then the caller's own infer"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_while_suspended_in_a_fanout_arm_interrupts_the_run() {
    // Cancellation while suspended in an arm: both arms are parked on slow
    // infers when the cancel lands, so the driver aborts the in-flight I/O
    // tasks and fails the run with Error::Interrupted. The arms are task
    // chains stranded by the run's end: they started, and the run's end
    // settles each with one `abandoned` terminal naming the run's end -
    // not a cancel or a failure of the arm's own. The 30-second answers
    // and the timeout guard prove the aborted I/O is never awaited.
    let gateway = ScriptedGateway::start(vec![resp_delayed_text(
        "too late",
        std::time::Duration::from_secs(30),
    )])
    .await;
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'one', 'two'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn models.infer('hang ' .. item)\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let mut driver = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
    let canceller = driver.cancel_handle();
    let calls = Arc::clone(&gateway.calls);
    tokio::spawn(async move {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        canceller.cancel();
    });

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), driver.drive())
        .await
        .expect("cancellation must not wait on the aborted in-flight I/O");

    assert!(
        matches!(result, Err(Error::Interrupted)),
        "cancelling suspended arms must interrupt the run, got {result:?}"
    );
    assert_eq!(
        gateway.call_count(),
        2,
        "both arms were suspended on their infers when the cancel landed"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        2,
        "both arms started: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_CANCELLED) + terminal_count(&recorder, TASK_FAILED),
        0,
        "a stranded arm reports no cancel or failure of its own: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        0,
        "no arm succeeded: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_ABANDONED_BY_RUN_END),
        2,
        "the run's end settles each stranded arm once: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_spawn_failure_mid_fanout_cancels_the_queued_arms() {
    // With the chain-count bound shrunk so the second arm's spawn fails
    // while the shim spawns its arms up front, the shim must cancel the
    // arm it already started before it re-raises: the parent catches the
    // error exactly once, and nothing is left live for the chain-end leak
    // check. The first arm was still queued for admission - the spawn
    // failure lands before the drain admits anything - so it never runs
    // its block, and no TaskStarted or TaskCancelled fires for it: a task
    // that never ran reports no start and no terminal, though its slot
    // still reaches Cancelled.
    let gateway = ScriptedGateway::start(vec![resp_text("after-answer")]).await;
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local ok, err = pcall(fanout, '### Worker', {'one', 'two', 'three'})\n\
        assert(not ok, 'the spawn failure reaches the caller')\n\
        return 'caught:' .. models.infer('after')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn 'worked:' .. item\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let mut scheduler = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
    // The root walk chain is id 0 and the first arm id 1; the second arm's
    // start trips the bound.
    scheduler.set_max_chains_for_test(2);
    let out = scheduler
        .drive()
        .await
        .expect("the caught spawn failure lets the caller continue");

    assert_eq!(out, "caught:after-answer");
    assert_eq!(
        request_prompts(&gateway),
        vec!["after"],
        "no arm ran an infer; only the caller's own request fired"
    );
    assert_eq!(
        scheduler.task_state_for_test(&"0.0".parse().expect("a task id parses")),
        Some(TaskState::Cancelled),
        "the shim cancelled the spawned arm before it re-raised"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        0,
        "the arm was never admitted, so no start fired: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_CANCELLED),
        0,
        "a task that never started reports no terminal: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        0,
        "no arm ran to completion: {:?}",
        recorder.events()
    );
    assert!(
        !recorder
            .events()
            .iter()
            .any(|(section, event)| section == "Worker" && event == "Lua chunk started"),
        "the cancelled arm never ran its block: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_for_an_unknown_request_id_fails_loudly() {
    // An answer arriving for an id the run never issued (and that is not an
    // orphan) means the Harness lost track of its effects: the run must fail
    // with Error::Internal rather than silently discard the answer. Only an
    // orphaned id (a fatal sibling's late I/O answer, covered by
    // `a_caught_fanout_failure_lets_the_caller_continue`) may be
    // discarded.
    let gateway = ScriptedGateway::start(vec![resp_text("real-answer")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Infer\n\n\
        ## Only\n\n\
        ```lua\nreturn models.infer('ask')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let mut scheduler = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
    // Handed to the run before the drive: the phantom answer lands ahead
    // of the real infer's, on a run that has issued nothing.
    scheduler
        .run_for_test()
        .resume(EffectId(u64::MAX), EffectAnswer::Timer);
    let error = scheduler
        .drive()
        .await
        .expect_err("an answer for an unissued effect must fail the run");

    assert!(
        matches!(error, Error::Internal { message, .. } if message.contains("did not issue")),
        "the unknown answer is a loud invariant failure: {error}"
    );
}

#[path = "failures-script-tools.rs"]
mod script_tools;
