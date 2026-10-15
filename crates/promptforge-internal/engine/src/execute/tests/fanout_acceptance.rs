//! Checkpoint acceptance tests for the shim-driven fanout over the task
//! protocol: a queued arm is admitted when any arm frees its slot (not
//! the
//! lowest-index arm's), a fatal arm gives every started arm exactly one
//! terminal task observation, a nested fanout nests its arm ids under the
//! outer arm's chain, hierarchical ids are identical across two runs
//! whose arms finish in different orders, and three arms each running
//! `models.loop` hold three model rounds in flight at once. The exhausted
//! stub, empty collection, list-section worker, claims violation across
//! arms, and the fanout-inside-a-`call`-child id nesting are pinned in
//! `scheduler`. An arm parks on a model round the rig holds until the
//! test releases it.

use std::collections::BTreeMap;

use promptforge_types::ids::TaskId;

use super::model_task_acceptance::{task_lifecycle, terminals_per_started_task};
use super::models_loop::loop_scenario;
use super::scheduler::{asks, ceiling, issued_prompts, request_prompts, scheduler_scenario};
use super::tasks::TaskRecorder;
use super::*;
use crate::test_support::rig::Rig;
use crate::test_support::scenario::{chat_answer, infer_prompt};

/// A model that answers each `models.infer` round with its prompt in
/// capitals, colons dropped: `a:1` gets `A1`, whatever order the rounds
/// are answered in.
fn capitals(effect: &Effect) -> EffectAnswer {
    chat_answer(&text(&infer_prompt(effect).to_uppercase().replace(':', "")))
}

#[test]
fn a_queued_arm_is_admitted_when_any_arm_frees_its_slot() {
    // Ceiling 2 over three arms, all spawned up front. Arm `a` parks on a
    // held first round while arm `b` completes immediately; the scheduler
    // admits `c` when `b`'s end frees its slot, so `c:1` is issued before
    // `a`'s second infer. Admission keyed to the lowest-index arm's end
    // would hold `c` until `a` finished: `[a:1, b:1, a:2, c:1]`.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = tasks.fanout('### Worker', {'a', 'b', 'c'})\n\
        return r[1].text .. '|' .. r[2].text .. '|' .. r[3].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local first = models.infer(item .. ':1')\n\
        if item == 'a' then return first .. models.infer('a:2') end\n\
        return first\n\
        ```\n";
    let mut rig = Rig::start(
        scheduler_scenario(md)
            .limits(ceiling(2))
            .chat_with(capitals)
            .hold(asks("a:1")),
    );
    rig.settle();
    rig.release(asks("a:1"));
    let rig = rig.finish();

    assert_eq!(
        rig.trace().text(),
        "A1A2|B1|C1",
        "results land by collection index"
    );
    assert_eq!(
        issued_prompts(&rig),
        vec!["a:1", "b:1", "c:1", "a:2"],
        "arm c starts on b's completion while a is still parked"
    );
    let records = TaskRecorder::replayed(rig.trace()).records();
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([
            (task("0.0"), vec!["succeeded"]),
            (task("0.1"), vec!["succeeded"]),
            (task("0.2"), vec!["succeeded"]),
        ]),
        "three arms start in collection order and each succeeds once"
    );
}

#[test]
fn a_fatal_arm_gives_every_started_arm_exactly_one_terminal() {
    // Fail-fast under a ceiling of 2 over three arms: `boom` fails after
    // its infer while `slow` is parked on a held round and `queued` has
    // not been admitted. The shim cancels the live sibling and re-raises,
    // so the started arms report exactly one terminal each (`failed`,
    // `cancelled`), the queued arm never starts and reports nothing, and
    // the run never waits on the held round.
    let gateway = ScriptedChat::new(vec![text("boom-answer")]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\ntasks.fanout('### Worker', {'boom', 'slow', 'queued'})\n```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':1')\n\
        if item == 'boom' then error('fatal arm error') end\n\
        return a\n\
        ```\n";
    let rig = Rig::run(
        scheduler_scenario(md)
            .limits(ceiling(2))
            .chat(&gateway)
            .hold(asks("slow:1")),
    );
    let error = rig.trace().error();

    assert!(
        error.to_string().contains("fatal arm error"),
        "the arm's own error surfaces: {error}"
    );
    assert_eq!(
        issued_prompts(&rig),
        vec!["boom:1", "slow:1"],
        "the queued arm never reached the gateway"
    );
    let records = TaskRecorder::replayed(rig.trace()).records();
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([
            (task("0.0"), vec!["failed"]),
            (task("0.1"), vec!["cancelled"]),
        ]),
        "each started arm has exactly one terminal and the queued arm never started: {:?}",
        task_lifecycle(&records)
    );
}

#[test]
fn a_nested_fanout_nests_its_arm_ids_under_the_outer_arm() {
    // Each outer arm (`0.K`, worker entry `0.K.0`) runs its own fanout, so
    // the inner arms are the outer arm chain's children (`0.K.J`, entry
    // `0.K.J.0`) with their own 1-based `sys.index`; results place by
    // collection index at both levels, and all six arms start and succeed
    // exactly once.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = tasks.fanout('### Outer', {'a', 'b'})\n\
        return r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ### Outer\n\n\
        ```lua\n\
        local r = tasks.fanout('### Inner', {'x', 'y'})\n\
        return sys.id .. '(' .. r[1].text .. ',' .. r[2].text .. ')'\n\
        ```\n\n\
        ### Inner\n\n\
        ```lua\n\
        return sys.id .. ':' .. item .. sys.index\n\
        ```\n";
    let rig = Rig::run(scheduler_scenario(md));

    assert_eq!(
        rig.trace().text(),
        "0.0.0(0.0.0.0:x1,0.0.1.0:y2)|0.1.0(0.1.0.0:x1,0.1.1.0:y2)",
        "inner arms nest under their outer arm's chain id with a per-fanout index"
    );
    let records = TaskRecorder::replayed(rig.trace()).records();
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([
            (task("0.0"), vec!["succeeded"]),
            (task("0.0.0"), vec!["succeeded"]),
            (task("0.0.1"), vec!["succeeded"]),
            (task("0.1"), vec!["succeeded"]),
            (task("0.1.0"), vec!["succeeded"]),
            (task("0.1.1"), vec!["succeeded"]),
        ]),
        "two outer and four inner arms each start and succeed once: {:?}",
        task_lifecycle(&records)
    );
}

/// Two arms that each infer twice, a `call` child after the fanout, and
/// the caller's own id: every id the identity test compares.
const IDENTITY: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Identity\n\n\
    ## Main\n\n\
    ```lua\n\
    local r = tasks.fanout('## Worker', {'a', 'b'})\n\
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

#[test]
fn ids_are_identical_across_runs_whose_arms_finish_in_different_orders() {
    // Run one holds arm `a`'s first round so `b` finishes first; run two
    // holds `b`'s so `a` finishes first. The request orders and the
    // success orders prove the finish orders differ; the arm ids, the
    // post-fanout `call` child's id, and the caller's entry id are
    // byte-identical because every id is allocated from chain-local
    // counters at spawn, never from completion order.
    let parked_on = |round: &str| {
        let mut rig = Rig::start(
            scheduler_scenario(IDENTITY)
                .chat_with(capitals)
                .hold(asks(round)),
        );
        rig.settle();
        rig.release(asks(round));
        let rig = rig.finish();
        let succeeded: Vec<TaskId> = task_lifecycle(&TaskRecorder::replayed(rig.trace()).records())
            .into_iter()
            .filter(|(label, _)| *label == "succeeded")
            .map(|(_, task)| task)
            .collect();
        (
            rig.trace().text().to_owned(),
            issued_prompts(&rig),
            succeeded,
        )
    };
    let (first_out, first_requests, first_succeeded) = parked_on("a:1");
    let (second_out, second_requests, second_succeeded) = parked_on("b:1");

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

#[test]
fn three_arms_running_models_loop_hold_three_model_rounds_in_flight_at_once() {
    // Each arm's loop runs two rounds: a tool call the loop dispatches,
    // then the terminal reply. All three first-round requests reach the
    // gateway before any arm's second round does, so three model rounds
    // are outstanding at once; arms driven one loop at a time would send
    // `[a, a, b, b, c, c]`. The second-round bodies include the round-one
    // exchange (user, assistant tool call, tool result) so the loop, not a
    // bare infer, is what ran in every arm.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_a", "echo", "{\"value\":\"a\"}")]),
        calls(&[("call_b", "echo", "{\"value\":\"b\"}")]),
        calls(&[("call_c", "echo", "{\"value\":\"c\"}")]),
        text("final"),
        text("final"),
        text("final"),
    ]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = tasks.fanout('### Worker', {'a', 'b', 'c'})\n\
        return r[1].text .. '|' .. r[2].text .. '|' .. r[3].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        tools.offer('tools/echo')\n\
        local msgs = messages.new()\n\
        msgs:user(item)\n\
        models.loop(msgs)\n\
        assert(#msgs == 4, 'user, the tool call, its result, and the terminal reply')\n\
        return msgs[#msgs].content\n\
        ```\n";
    let rig = Rig::run(loop_scenario(md).offer("echo", echo_tool()).chat(&gateway));

    assert_eq!(rig.trace().text(), "final|final|final");
    let bodies = gateway.requests();
    let message_counts: Vec<usize> = bodies.iter().map(|body| body.messages.len()).collect();
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
        let roles: Vec<&str> = body
            .messages
            .iter()
            .map(crate::model::Message::role)
            .collect();
        assert_eq!(
            roles,
            vec!["user", "assistant", "tool"],
            "round two replays the round-one exchange"
        );
    }
    let records = TaskRecorder::replayed(rig.trace()).records();
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([
            (task("0.0"), vec!["succeeded"]),
            (task("0.1"), vec!["succeeded"]),
            (task("0.2"), vec!["succeeded"]),
        ]),
        "three arms each succeed once: {:?}",
        task_lifecycle(&records)
    );
}
