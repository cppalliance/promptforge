//! The step's `withdrawn` list: a step that cancels a task parked on a
//! batch lists the calls it orphaned, the calls the same step issued
//! included, in effect-id order however many tasks it cancels, and a
//! decided run's step lists none.

use super::*;

/// The effects one step issued, in issue order, and the tool calls it
/// withdrew. Panics when the run is over.
fn withdrawals(step: Step) -> (Vec<(EffectId, Effect)>, Vec<EffectId>) {
    let Step::Pending {
        effects, withdrawn, ..
    } = step
    else {
        panic!("the run is still going: {step:?}");
    };
    let effects = effects
        .into_iter()
        .map(|(id, _, effect)| (id, effect))
        .collect();
    (effects, withdrawn)
}

/// Main spawns Child and waits on one model round while Child parks on a
/// tool-call batch, then cancels Child and lingers on a second round.
const CANCELLED_CHILD: &str = "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n# Loop\n\n\
     ## Main\n\n```lua\n\
     local t = tasks.spawn('## Child')\n\
     models.infer('wait')\n\
     tasks.cancel(t)\n\
     models.infer('linger')\n\
     return 'cancelled'\n```\n\n\
     ## Child\n\n```lua\n\
     local msgs = messages.new()\n\
     msgs:user('go')\n\
     models.loop(msgs)\n\
     return 'child done'\n```\n";

/// A run of [`CANCELLED_CHILD`] past its first step, with the ids of
/// Main's `wait` round and Child's loop round. The fixture must outlive
/// the run.
fn cancelled_child() -> (Run, RunFixture, EffectId, EffectId) {
    let prompt = parse(CANCELLED_CHILD);
    let (state, fixture) = loop_context(&prompt, echo_tools());
    let mut run = Run::from_state(state);
    let (rounds, withdrawn) = withdrawals(run.step());
    assert!(
        withdrawn.is_empty(),
        "nothing is withdrawn yet: {withdrawn:?}"
    );
    let origin = |wanted: ReplyOrigin| {
        rounds
            .iter()
            .find_map(|(id, effect)| match effect {
                Effect::Chat { round, .. } if round.origin == wanted => Some(*id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("a {wanted:?} round: {rounds:?}"))
    };
    let (infer, round) = (origin(ReplyOrigin::Infer), origin(ReplyOrigin::Chat));
    (run, fixture, infer, round)
}

/// Child's round answer: two echo calls, `c1` then `c2`.
fn two_echoes() -> EffectAnswer {
    calls_reply(&[
        ("c1", "echo", json!({ "value": "one" })),
        ("c2", "echo", json!({ "value": "two" })),
    ])
}

/// The ids of the tool calls among `effects`, in issue order.
fn tool_call_ids(effects: &[(EffectId, Effect)]) -> Vec<EffectId> {
    effects
        .iter()
        .filter(|(_, effect)| matches!(effect, Effect::ToolCall { .. }))
        .map(|(id, _)| *id)
        .collect()
}

#[test]
fn cancelling_a_task_withdraws_its_unanswered_tool_calls() {
    let (mut run, _fixture, infer, round) = cancelled_child();
    run.resume(round, two_echoes());
    let (issued, withdrawn) = withdrawals(run.step());
    let calls = tool_call_ids(&issued);
    assert_eq!(calls.len(), 2, "both calls are in flight: {issued:?}");
    assert!(
        withdrawn.is_empty(),
        "issuing withdraws nothing: {withdrawn:?}"
    );
    run.resume(infer, text_reply("go"));
    let (linger, effect) = {
        let (mut issued, withdrawn) = withdrawals(run.step());
        assert_eq!(
            withdrawn, calls,
            "the cancel withdraws both calls, c1 first"
        );
        assert_eq!(issued.len(), 1, "only Main's next round: {issued:?}");
        issued.remove(0)
    };
    assert!(
        matches!(&effect, Effect::Chat { round, .. } if round.origin == ReplyOrigin::Infer),
        "the cancel issues only Main's next round: {effect:?}"
    );
    for id in calls {
        run.resume(id, EffectAnswer::Dropped);
    }
    run.resume(linger, text_reply("done"));
    let Step::Done { result, .. } = run.step() else {
        panic!("Main's second round ends the run");
    };
    assert_eq!(text_of(result), "cancelled");
}

#[test]
fn a_call_issued_and_withdrawn_in_one_step_is_listed_twice() {
    let (mut run, _fixture, infer, round) = cancelled_child();
    run.resume(round, two_echoes());
    run.resume(infer, text_reply("go"));
    let (issued, withdrawn) = withdrawals(run.step());
    let calls = tool_call_ids(&issued);
    assert_eq!(
        calls.len(),
        2,
        "Child issues both calls before Main cancels it: {issued:?}"
    );
    assert_eq!(withdrawn, calls, "the same step withdraws both calls");
    for (id, effect) in &issued {
        let answer = match effect {
            Effect::ToolCall { .. } => EffectAnswer::Dropped,
            _ => text_reply("done"),
        };
        run.resume(*id, answer);
    }
    let Step::Done { result, .. } = run.step() else {
        panic!("Main's second round ends the run");
    };
    assert_eq!(text_of(result), "cancelled");
}

#[test]
fn a_decided_run_lists_no_withdrawn_calls() {
    let (mut run, _fixture, infer, round) = cancelled_child();
    run.resume(round, two_echoes());
    let (issued, _) = withdrawals(run.step());
    let calls = tool_call_ids(&issued);
    assert_eq!(calls.len(), 2, "both calls are in flight: {issued:?}");
    run.cancel();
    let (issued, withdrawn) = withdrawals(run.step());
    assert!(run.decided(), "the cancel decides the run");
    assert!(
        issued.is_empty(),
        "a torn-down run issues nothing: {issued:?}"
    );
    assert!(
        withdrawn.is_empty(),
        "the run's end abandoned Child, yet a decided run lists none: {withdrawn:?}"
    );
    for id in calls.into_iter().chain([infer]) {
        run.resume(id, EffectAnswer::Dropped);
    }
    let Step::Done { result, .. } = run.step() else {
        panic!("every orphan is answered, so the run is done");
    };
    assert!(
        matches!(result, RunResult::Cancelled),
        "the run ends cancelled: {result:?}"
    );
}

/// Main spawns A and then B, each parked on a one-call batch, waits on a
/// model round, then cancels B before A and lingers on a second round.
const CANCELLED_PAIR: &str = "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n# Loop\n\n\
     ## Main\n\n```lua\n\
     local a = tasks.spawn('## A')\n\
     local b = tasks.spawn('## B')\n\
     models.infer('wait')\n\
     tasks.cancel(b)\n\
     tasks.cancel(a)\n\
     models.infer('linger')\n\
     return 'cancelled'\n```\n\n\
     ## A\n\n```lua\n\
     local msgs = messages.new()\n\
     msgs:user('a')\n\
     models.loop(msgs)\n\
     return 'a done'\n```\n\n\
     ## B\n\n```lua\n\
     local msgs = messages.new()\n\
     msgs:user('b')\n\
     models.loop(msgs)\n\
     return 'b done'\n```\n";

#[test]
fn one_step_lists_every_tasks_withdrawn_calls_in_effect_id_order() {
    let prompt = parse(CANCELLED_PAIR);
    let (state, _fixture) = loop_context(&prompt, echo_tools());
    let mut run = Run::from_state(state);
    let (rounds, _) = withdrawals(run.step());
    let asking = |user: &str| {
        rounds
            .iter()
            .find_map(|(id, effect)| match effect {
                Effect::Chat { messages, .. }
                    if messages.iter().any(|message| message.content() == user) =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("a round asking {user:?}: {rounds:?}"))
    };
    for task in ["a", "b"] {
        let call = (task, "echo", json!({ "value": task }));
        run.resume(asking(task), calls_reply(&[call]));
    }
    let (issued, _) = withdrawals(run.step());
    let values: Vec<&str> = issued
        .iter()
        .map(|(_, effect)| call_value(effect))
        .collect();
    assert_eq!(values, ["a", "b"], "A's call issues before B's");
    run.resume(asking("wait"), text_reply("go"));
    let (linger, withdrawn) = withdrawals(run.step());
    assert_eq!(
        withdrawn,
        tool_call_ids(&issued),
        "Main cancels B first, yet A's call is listed first"
    );
    for (id, _) in &issued {
        run.resume(*id, EffectAnswer::Dropped);
    }
    let [(linger, _)] = linger.as_slice() else {
        panic!("the cancels issue only Main's next round: {linger:?}");
    };
    run.resume(*linger, text_reply("done"));
    let Step::Done { result, .. } = run.step() else {
        panic!("Main's second round ends the run");
    };
    assert_eq!(text_of(result), "cancelled");
}
