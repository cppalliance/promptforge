//! Checkpoint acceptance tests for the shim-driven fanout over the task
//! protocol: the window refills on any arm's completion (not the
//! lowest-index arm's), a fatal arm gives every started arm exactly one
//! terminal task observation, a nested fanout nests its arm ids under the
//! outer arm's chain, hierarchical ids are identical across two runs
//! whose arms finish in different orders, and three arms each running
//! `models.loop` hold three model rounds in flight at once. The exhausted
//! stub, empty collection, list-section worker, claims violation across
//! arms, and the fanout-inside-a-`call`-child id nesting are pinned in
//! `scheduler`.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::time::Duration;

use promptforge_api_types::ids::TaskId;

use super::models_loop::{echo_tools, loop_context_observed};
use super::scheduler::{request_prompts, scheduler_context_from, scheduler_context_on};
use super::tasks::TaskRecorder;
use super::*;
use crate::execute::tokio_driver::TokioDriver;

/// The gateway delay that keeps one arm parked while its siblings finish.
/// The arms it orders against complete in milliseconds on the loopback
/// gateway, so the margin is wide; the test's wall time is this delay.
const PARKED: Duration = Duration::from_secs(1);

/// Every task observation the recorder saw, as `(label, task id)` pairs in
/// order, so a test can pair each started arm with its terminals.
fn task_events(recorder: &TaskRecorder) -> Vec<(&'static str, TaskId)> {
    recorder
        .records()
        .into_iter()
        .filter_map(|(_, observation)| match observation {
            Observation::TaskStarted { task, .. } => Some(("started", task)),
            Observation::TaskSucceeded { task } => Some(("succeeded", task)),
            Observation::TaskFailed { task } => Some(("failed", task)),
            Observation::TaskCancelled { task } => Some(("cancelled", task)),
            Observation::TaskAbandoned { task, .. } => Some(("abandoned", task)),
            _ => None,
        })
        .collect()
}

/// The terminal labels recorded per started task, in order. Every started
/// task appears (with an empty list when it has no terminal); a terminal
/// for a task that never started fails the test.
fn terminals_per_started_task(recorder: &TaskRecorder) -> BTreeMap<TaskId, Vec<&'static str>> {
    let events = task_events(recorder);
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

fn task(id: &str) -> TaskId {
    id.parse().expect("a task id parses")
}

/// A scheduler context on the given observer with the fanout window
/// narrowed to `window` live arms.
fn windowed_context(prompt: &Prompt, window: usize, observer: Arc<dyn Observer>) -> RunState {
    scheduler_context_from(
        prompt,
        &TestStore::new(),
        &RunContext::new(EXECUTION)
            .limits(
                RunLimits::new().max_fanout_concurrency(
                    NonZeroUsize::new(window).expect("the window is non-zero"),
                ),
            )
            .observer(observer),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn the_window_refills_on_any_arms_completion_not_the_first_arms() {
    // Window 2 over three arms. Arm `a` parks on a delayed first answer
    // while arm `b` completes immediately; the shim must refill with `c`
    // on `b`'s completion, so `c:1` reaches the gateway before `a`'s
    // second infer. A refill keyed to the lowest-index arm would hold `c`
    // until `a` finished: `[a:1, b:1, a:2, c:1]`.
    let gateway = ScriptedGateway::start(vec![
        resp_delayed_text("A1", PARKED),
        resp_text("B"),
        resp_text("C"),
        resp_text("A2"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'a', 'b', 'c'})\n\
        return r[1].text .. '|' .. r[2].text .. '|' .. r[3].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local first = models.infer(item .. ':1')\n\
        if item == 'a' then return first .. models.infer('a:2') end\n\
        return first\n\
        ```\n";
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = windowed_context(&prompt, 2, Arc::clone(&recorder) as Arc<dyn Observer>);
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the windowed fanout completes");

    assert_eq!(out, "A1A2|B|C", "results land by collection index");
    assert_eq!(
        request_prompts(&gateway),
        vec!["a:1", "b:1", "c:1", "a:2"],
        "arm c starts on b's completion while a is still parked"
    );
    let terminals = terminals_per_started_task(&recorder);
    assert_eq!(
        terminals,
        BTreeMap::from([
            (task("0.0"), vec!["succeeded"]),
            (task("0.1"), vec!["succeeded"]),
            (task("0.2"), vec!["succeeded"]),
        ]),
        "three arms start in collection order and each succeeds once"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_fatal_arm_gives_every_started_arm_exactly_one_terminal() {
    // Fail-fast under a window of 2 over three arms: `boom` fails after
    // its infer while `slow` is parked on a 30-second answer and `queued`
    // has not started. The shim cancels the live sibling and re-raises,
    // so the started arms report exactly one terminal each (`failed`,
    // `cancelled`), the queued arm never starts and reports nothing, and
    // the driver never waits on the aborted answer.
    let gateway = ScriptedGateway::start(vec![
        resp_text("boom-answer"),
        resp_delayed_text("slow-answer", Duration::from_secs(30)),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\nfanout('### Worker', {'boom', 'slow', 'queued'})\n```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':1')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return a\n\
        ```\n";
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = windowed_context(&prompt, 2, Arc::clone(&recorder) as Arc<dyn Observer>);
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        TokioDriver::new(&ctx, Some(gateway_client(gateway.addr()))).drive(),
    )
    .await
    .expect("the aborted sibling must not stall the driver");
    let error = result.expect_err("a fatal arm fails the fanout");

    assert!(
        error.to_string().contains("fatal arm error"),
        "the arm's own error surfaces: {error}"
    );
    assert_eq!(
        request_prompts(&gateway),
        vec!["boom:1", "slow:1"],
        "the queued arm never reached the gateway"
    );
    let terminals = terminals_per_started_task(&recorder);
    assert_eq!(
        terminals,
        BTreeMap::from([
            (task("0.0"), vec!["failed"]),
            (task("0.1"), vec!["cancelled"]),
        ]),
        "each started arm has exactly one terminal and the queued arm never started: {:?}",
        task_events(&recorder)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_nested_fanout_nests_its_arm_ids_under_the_outer_arm() {
    // Each outer arm (`0.K`, worker entry `0.K.0`) runs its own fanout, so
    // the inner arms are the outer arm chain's children (`0.K.J`, entry
    // `0.K.J.0`) with their own 1-based `sys.index`; results place by
    // collection index at both levels, and all six arms start and succeed
    // exactly once.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Outer', {'a', 'b'})\n\
        return r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ### Outer\n\n\
        ```lua\n\
        local r = fanout('### Inner', {'x', 'y'})\n\
        return sys.id .. '(' .. r[1].text .. ',' .. r[2].text .. ')'\n\
        ```\n\n\
        ### Inner\n\n\
        ```lua\n\
        return sys.id .. ':' .. item .. sys.index\n\
        ```\n";
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, None)
        .drive()
        .await
        .expect("the nested fanout completes");

    assert_eq!(
        out, "0.0.0(0.0.0.0:x1,0.0.1.0:y2)|0.1.0(0.1.0.0:x1,0.1.1.0:y2)",
        "inner arms nest under their outer arm's chain id with a per-fanout index"
    );
    let terminals = terminals_per_started_task(&recorder);
    assert_eq!(
        terminals,
        BTreeMap::from([
            (task("0.0"), vec!["succeeded"]),
            (task("0.0.0"), vec!["succeeded"]),
            (task("0.0.1"), vec!["succeeded"]),
            (task("0.1"), vec!["succeeded"]),
            (task("0.1.0"), vec!["succeeded"]),
            (task("0.1.1"), vec!["succeeded"]),
        ]),
        "two outer and four inner arms each start and succeed once: {:?}",
        task_events(&recorder)
    );
}

/// Drives the identity prompt against a scripted gateway and returns the
/// run's output, the gateway's request order, and the order in which the
/// arms reported success.
async fn identity_run(script: Vec<GatewayReply>) -> (String, Vec<String>, Vec<TaskId>) {
    let gateway = ScriptedGateway::start(script).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Identity\n\n\
        ## Main\n\n\
        ```lua\n\
        local r = fanout('## Worker', {'a', 'b'})\n\
        local sub = call('## Sub')\n\
        return r[1].text .. '|' .. r[2].text .. '|' .. sub .. '|' .. sys.id\n\
        ```\n\n\
        ## Worker\n\n\
        ```lua\n\
        local first = models.infer(item .. ':1')\n\
        local second = models.infer(item .. ':2')\n\
        return sys.id .. '=' .. first .. second\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\nreturn sys.id\n```\n";
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the identity prompt completes");
    let succeeded = task_events(&recorder)
        .into_iter()
        .filter(|(label, _)| *label == "succeeded")
        .map(|(_, task)| task)
        .collect();
    (out, request_prompts(&gateway), succeeded)
}

#[tokio::test(flavor = "current_thread")]
async fn ids_are_identical_across_runs_whose_arms_finish_in_different_orders() {
    // Run one parks arm `a`'s first answer so `b` finishes first; run two
    // parks `b`'s so `a` finishes first. The request orders and the
    // success orders prove the finish orders differ; the arm ids, the
    // post-fanout `call` child's id, and the caller's entry id are
    // byte-identical because every id is allocated from chain-local
    // counters at spawn, never from completion order.
    let (first_out, first_requests, first_succeeded) = identity_run(vec![
        resp_delayed_text("A1", PARKED),
        resp_text("B1"),
        resp_text("B2"),
        resp_text("A2"),
    ])
    .await;
    let (second_out, second_requests, second_succeeded) = identity_run(vec![
        resp_text("A1"),
        resp_delayed_text("B1", PARKED),
        resp_text("A2"),
        resp_text("B2"),
    ])
    .await;

    assert_eq!(
        first_requests,
        vec!["a:1", "b:1", "b:2", "a:2"],
        "run one: b finishes while a is parked"
    );
    assert_eq!(
        second_requests,
        vec!["a:1", "b:1", "a:2", "b:2"],
        "run two: a finishes while b is parked"
    );
    // A `call` child is a chain, not a task, so only the two arms report.
    assert_eq!(
        first_succeeded,
        vec![task("0.1"), task("0.0")],
        "run one: arm b succeeds before arm a"
    );
    assert_eq!(
        second_succeeded,
        vec![task("0.0"), task("0.1")],
        "run two: arm a succeeds before arm b"
    );
    assert_eq!(first_out, second_out, "finish order must not change any id");
    assert_eq!(
        first_out, "0.0.0=A1A2|0.1.0=B1B2|0.2.0|0.1",
        "arms are the caller's children 0 and 1, the call child is child 2, \
         and the caller keeps the walk's first entry id"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn three_arms_running_models_loop_hold_three_model_rounds_in_flight_at_once() {
    // Each arm's loop runs two rounds: a tool call the loop dispatches,
    // then the terminal reply. All three first-round requests reach the
    // gateway before any arm's second round does, so three model rounds
    // are outstanding at once; arms driven one loop at a time would send
    // `[a, a, b, b, c, c]`. The second-round bodies carry the round-one
    // exchange (user, assistant tool call, tool result) so the loop, not a
    // bare infer, is what ran in every arm.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_a", "echo", "{\"value\":\"a\"}"),
        resp_tool_call("call_b", "echo", "{\"value\":\"b\"}"),
        resp_tool_call("call_c", "echo", "{\"value\":\"c\"}"),
        resp_text("final"),
        resp_text("final"),
        resp_text("final"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'a', 'b', 'c'})\n\
        return r[1].text .. '|' .. r[2].text .. '|' .. r[3].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local msgs = messages.new()\n\
        msgs:user(item)\n\
        models.loop(msgs)\n\
        assert(#msgs == 4, 'user, the tool call, its result, and the terminal reply')\n\
        return msgs[#msgs].content\n\
        ```\n";
    let prompt = parse(md);
    let recorder = Arc::new(TaskRecorder::default());
    let ctx = loop_context_observed(
        &prompt,
        echo_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("three looping arms complete");

    assert_eq!(out, "final|final|final");
    let bodies = gateway.requests();
    let message_counts: Vec<usize> = bodies
        .iter()
        .map(|body| {
            body["messages"]
                .as_array()
                .expect("a chat request carries messages")
                .len()
        })
        .collect();
    assert_eq!(
        message_counts,
        vec![1, 1, 1, 3, 3, 3],
        "all three first rounds are in flight before any second round: {:?}",
        request_prompts(&gateway)
    );
    let mut first_round_prompts = request_prompts(&gateway)[..3].to_vec();
    first_round_prompts.sort_unstable();
    assert_eq!(
        first_round_prompts,
        vec!["a", "b", "c"],
        "each arm opened its own round one"
    );
    for body in &bodies[3..] {
        let roles: Vec<&str> = body["messages"]
            .as_array()
            .expect("a chat request carries messages")
            .iter()
            .map(|message| message["role"].as_str().expect("a message has a role"))
            .collect();
        assert_eq!(
            roles,
            vec!["user", "assistant", "tool"],
            "round two replays the round-one exchange"
        );
    }
    let terminals = terminals_per_started_task(&recorder);
    assert_eq!(
        terminals,
        BTreeMap::from([
            (task("0.0"), vec!["succeeded"]),
            (task("0.1"), vec!["succeeded"]),
            (task("0.2"), vec!["succeeded"]),
        ]),
        "three arms each succeed once: {:?}",
        task_events(&recorder)
    );
}
