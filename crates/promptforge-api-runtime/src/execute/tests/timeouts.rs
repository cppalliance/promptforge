//! Timeouts on the wait shims: `opts.timeout` on `tasks.when_any` returns
//! `nil` when the internal timer wins and the members keep running (no
//! `tasks_live` at chain end); on `tasks.when_all` it returns `results,
//! timed_out` with the unfinished members absent. When a member wins the
//! shim cancels the timer and its leaf work is dropped. The timer is an
//! effect-backed slot the author never sees: `tasks.pending` and a status
//! table's `tasks` list omit it, and it reports no task observations.

use std::time::Duration;

use super::scheduler::scheduler_context_on;
use super::waits::{WaitRecorder, task, tasks_prompt};
use super::*;
use crate::execute::scheduler::TaskState;

/// The gateway reply a slow child parks on: long enough that a short
/// timeout wins, short enough that the test then waits it out.
const SLOW: Duration = Duration::from_millis(400);

#[tokio::test(flavor = "current_thread")]
async fn when_any_returns_nil_when_the_timer_wins_and_the_member_keeps_running() {
    // The child parks on a slow model round; a 50ms wait times out and
    // returns nil, the child is still running, and a second untimed wait
    // delivers it - so nothing leaks at chain end.
    let gateway = ScriptedGateway::start(vec![resp_delayed_text("slow answer", SLOW)]).await;
    let md = tasks_prompt(
        "local t = tasks.spawn('## Child')\n\
         local first, ok, result = tasks.when_any({ t }, { timeout = 0.05 })\n\
         log('timed out first=' .. tostring(first) .. ' ok=' .. tostring(ok) .. ' result=' .. tostring(result))\n\
         local s = tasks.status(t)\n\
         log('after state=' .. s.state .. ' blocked=' .. tostring(s.blocked))\n\
         assert(#tasks.pending() == 1, 'the child is the only pending task')\n\
         local second, ok2, result2 = tasks.when_any({ t })\n\
         assert(second.task == t.task and ok2 and result2 == 'slow answer', tostring(result2))\n\
         return 'done'",
        &[("Child", "return models.infer('slow please')")],
    );
    let prompt = parse(&md);
    let recorder = Arc::new(WaitRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler
        .drive()
        .await
        .expect("a timed-out wait leaks nothing");
    assert_eq!(out, "done");
    assert_eq!(
        recorder.logs("Main"),
        vec![
            "timed out first=nil ok=nil result=nil".to_owned(),
            "after state=running blocked=chat".to_owned(),
        ],
        "the timed-out wait returns nil and the member keeps running"
    );
    // The timer took the owner's next child index after the child, and
    // its firing was delivered to the wait.
    assert_eq!(
        scheduler.task_state_for_test(&task("0.1")),
        Some(TaskState::Delivered),
        "the fired timer's slot was delivered to the wait"
    );
    assert!(
        recorder.task_events(&task("0.1")).is_empty(),
        "the internal timer reports no task observations: {:?}",
        recorder.records()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn when_any_cancels_the_timer_when_a_member_wins() {
    // The child returns at once; the wait's 30s timer never fires: the
    // shim cancels it, its slot is `Cancelled`, and the run ends without
    // waiting on it or leaking it.
    let md = tasks_prompt(
        "local t = tasks.spawn('## Child')\n\
         local first, ok, result = tasks.when_any({ t }, { timeout = 30 })\n\
         assert(first.task == t.task and ok and result == 'quick', tostring(result))\n\
         assert(#tasks.pending() == 0, 'nothing is pending')\n\
         return 'done'",
        &[("Child", "return 'quick'")],
    );
    let prompt = parse(&md);
    let recorder = Arc::new(WaitRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
    let out = tokio::time::timeout(Duration::from_secs(5), scheduler.drive())
        .await
        .expect("the run does not wait out the cancelled timer")
        .expect("a cancelled timer is not a leaked task");
    assert_eq!(out, "done");
    assert_eq!(
        scheduler.task_state_for_test(&task("0.1")),
        Some(TaskState::Cancelled),
        "the member's win cancelled the timer"
    );
    assert!(
        recorder.task_events(&task("0.1")).is_empty(),
        "the internal timer reports no task observations: {:?}",
        recorder.records()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn when_all_returns_timed_out_with_the_unfinished_members_absent() {
    // Quick returns at once; Slow parks on a slow model round. A 50ms
    // `when_all` returns Quick's entry, no entry for Slow, and
    // `timed_out = true`; Slow keeps running and a second untimed
    // `when_all` delivers it with `timed_out = false`.
    let gateway = ScriptedGateway::start(vec![resp_delayed_text("slow answer", SLOW)]).await;
    let md = tasks_prompt(
        "local q = tasks.spawn('## Quick')\n\
         local s = tasks.spawn('## Slow')\n\
         local results, timed_out = tasks.when_all({ q, s }, { timeout = 0.05 })\n\
         log('timed_out=' .. tostring(timed_out) .. ' n=' .. #results\n\
           .. ' quick=' .. tostring(results[1] and results[1].result)\n\
           .. ' slow=' .. tostring(results[2]))\n\
         assert(tasks.status(s).state == 'running', 'slow keeps running')\n\
         local rest, timed_out2 = tasks.when_all({ s })\n\
         log('rest timed_out=' .. tostring(timed_out2) .. ' slow=' .. tostring(rest[1].result))\n\
         return 'done'",
        &[
            ("Quick", "return 'quick'"),
            ("Slow", "return models.infer('slow please')"),
        ],
    );
    let prompt = parse(&md);
    let recorder = Arc::new(WaitRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a timed-out when_all leaks nothing");
    assert_eq!(out, "done");
    assert_eq!(
        recorder.logs("Main"),
        vec![
            "timed_out=true n=1 quick=quick slow=nil".to_owned(),
            "rest timed_out=false slow=slow answer".to_owned(),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn when_all_cancels_the_timer_when_every_member_finishes() {
    // Both members return at once under a 30s timeout: every entry is
    // present, `timed_out` is false, and the timer (the owner's third
    // child) is cancelled rather than waited out or leaked.
    let md = tasks_prompt(
        "local a = tasks.spawn('## Alpha')\n\
         local b = tasks.spawn('## Beta')\n\
         local results, timed_out = tasks.when_all({ a, b }, { timeout = 30 })\n\
         assert(timed_out == false, 'no timeout')\n\
         assert(#results == 2 and results[1].result == 'alpha' and results[2].result == 'beta')\n\
         assert(#tasks.pending() == 0, 'nothing is pending')\n\
         return 'done'",
        &[("Alpha", "return 'alpha'"), ("Beta", "return 'beta'")],
    );
    let prompt = parse(&md);
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
    let out = tokio::time::timeout(Duration::from_secs(5), scheduler.drive())
        .await
        .expect("the run does not wait out the cancelled timer")
        .expect("a cancelled timer is not a leaked task");
    assert_eq!(out, "done");
    assert_eq!(
        scheduler.task_state_for_test(&task("0.2")),
        Some(TaskState::Cancelled)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_timer_is_invisible_to_pending_and_to_a_status_tasks_list() {
    // The child waits on its grandchild with a timeout: the parent reads
    // the child's status mid-wait and sees one owned task (the grandchild),
    // never the timer; the child's own `pending` mid-wait cannot be read,
    // so the parent's view is the proof.
    let gateway = ScriptedGateway::start(vec![resp_delayed_text("slow answer", SLOW)]).await;
    let md = tasks_prompt(
        "local c = tasks.spawn('## Child')\n\
         store.write('park', 'x')\n\
         local s = tasks.status(c)\n\
         log('child blocked=' .. tostring(s.blocked) .. ' tasks=' .. #s.tasks .. ' first=' .. tostring(s.tasks[1]))\n\
         local _, ok, result = tasks.when_any({ c })\n\
         assert(ok, tostring(result))\n\
         return result",
        &[
            (
                "Child",
                "local g = tasks.spawn('## Grandchild')\n\
                 local first = tasks.when_any({ g }, { timeout = 30 })\n\
                 assert(first.task == g.task, 'the grandchild wins')\n\
                 return 'child done'",
            ),
            ("Grandchild", "return models.infer('slow please')"),
        ],
    );
    let prompt = parse(&md);
    let recorder = Arc::new(WaitRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the run completes");
    assert_eq!(out, "child done");
    assert_eq!(
        recorder.logs("Main"),
        vec!["child blocked=tasks tasks=1 first=0.0.0".to_owned()],
        "the child's status lists the grandchild alone, not its timer"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_timeout_option_is_validated_at_the_call_site() {
    let md = tasks_prompt(
        "local t = tasks.spawn('## Child')\n\
         local ok1, e1 = pcall(tasks.when_any, { t }, { timeout = 'soon' })\n\
         assert(not ok1 and e1.kind == 'lua', tostring(e1))\n\
         local ok2, e2 = pcall(tasks.when_all, { t }, { timeout = -1 })\n\
         assert(not ok2 and e2.kind == 'lua', tostring(e2))\n\
         local ok3, e3 = pcall(tasks.when_any, { t }, 'opts')\n\
         assert(not ok3 and e3.kind == 'lua', tostring(e3))\n\
         local ok4, e4 = pcall(tasks.when_any, { t }, { timeout = 0/0 })\n\
         assert(not ok4 and e4.kind == 'lua', tostring(e4))\n\
         local _, ok, result = tasks.when_any({ t })\n\
         assert(ok and result == 'quick', tostring(result))\n\
         return tostring(e1) .. '|' .. tostring(e2) .. '|' .. tostring(e3) .. '|' .. tostring(e4)",
        &[("Child", "return 'quick'")],
    );
    let prompt = parse(&md);
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let mut scheduler = TokioDriver::new(&ctx, None);
    let out = scheduler
        .drive()
        .await
        .expect("every option error is caught at the call site");
    let parts: Vec<&str> = out.split('|').collect();
    assert_eq!(parts.len(), 4, "{out}");
    assert!(parts[0].contains("timeout must be a number"), "{out}");
    assert!(parts[1].contains("-1"), "{out}");
    assert!(parts[2].contains("opts must be a table"), "{out}");
    assert!(parts[3].contains("timeout"), "{out}");
    // A rejected option starts no timer: the child is the run's only task.
    assert_eq!(scheduler.task_state_for_test(&task("0.1")), None);
}
