//! A round's catalog tool calls run in parallel: the loop sends every
//! unanswered call as one batch, the scheduler issues each catalog call in
//! the same step and answers once every issued call has landed, and the
//! records append in call order. A local tool or a task built-in stops the
//! batch and runs alone between its neighbors, a call that cannot be
//! bound stops it and raises once the issued calls land, and a task
//! cancelled while parked on a batch orphans every call in it. Each test
//! steps a `Run` by hand, so it sees exactly what each step issued.

use promptforge_types::event::{Event, ReplyOrigin};

use super::models_loop::{echo_tools, loop_context, loop_prompt};
use super::serial_driver::{text_of, text_reply};
use super::*;
use crate::execute::run::{Effect, EffectAnswer, EffectId, Run, Step};
use crate::model::{Completion, CompletionResult, ToolCall};

/// The effects one step issued, in issue order, and the events it
/// reported. Panics when the run is over.
fn pending(step: Step) -> (Vec<(EffectId, Effect)>, Vec<Event>) {
    let Step::Pending { effects, events } = step else {
        panic!("the run is still going: {step:?}");
    };
    let effects = effects
        .into_iter()
        .map(|(id, _, effect)| (id, effect))
        .collect();
    (effects, events)
}

/// The one effect a step issued; panics on any other count.
fn only_effect(step: Step) -> (EffectId, Effect) {
    let (mut effects, _) = pending(step);
    assert_eq!(effects.len(), 1, "one effect is issued: {effects:?}");
    effects.remove(0)
}

/// A round requesting every `(id, name, arguments)` call, in order.
fn calls_reply(calls: &[(&str, &str, Value)]) -> EffectAnswer {
    let calls = calls
        .iter()
        .map(|(id, name, arguments)| {
            ToolCall::from_parts(*id, *name, arguments.clone()).expect("a scripted call is whole")
        })
        .collect();
    let completion = Completion::from_result(CompletionResult::ToolCalls(calls), "test-model")
        .expect("a scripted batch is accepted");
    EffectAnswer::Chat(Ok(Box::new(completion)))
}

/// A tool call's trusted answer `text`.
fn tool_text(text: &str) -> EffectAnswer {
    EffectAnswer::ToolCall(Ok(ToolOutput::trusted(text)))
}

/// The `value` argument of a `ToolCall` effect, which names the call in
/// these tests.
fn call_value(effect: &Effect) -> &str {
    let Effect::ToolCall { args, .. } = effect else {
        panic!("a tool call: {effect:?}");
    };
    args["value"].as_str().expect("the call names its value")
}

/// The tool messages of a `Chat` effect's request, in order.
fn tool_messages(effect: &Effect) -> Vec<String> {
    let Effect::Chat { messages, .. } = effect else {
        panic!("a model round: {effect:?}");
    };
    messages
        .iter()
        .filter(|message| message.role() == "tool")
        .map(|message| message.content().to_owned())
        .collect()
}

#[test]
fn a_rounds_catalog_calls_are_issued_in_one_step_and_append_in_call_order() {
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('echo twice')\n\
         models.loop(msgs)\n\
         return msgs[3].tool_call_id .. '=' .. msgs[3].content \
         .. '|' .. msgs[4].tool_call_id .. '=' .. msgs[4].content",
    );
    let prompt = parse(&md);
    let (state, _fixture) = loop_context(&prompt, echo_tools());
    let mut run = Run::from_state(state);
    let mut events = Vec::new();
    let (round, _) = only_effect(run.step());
    run.resume(
        round,
        calls_reply(&[
            ("c1", "echo", json!({ "value": "one" })),
            ("c2", "echo", json!({ "value": "two" })),
        ]),
    );
    let (calls, step_events) = pending(run.step());
    events.extend(step_events);
    let values: Vec<&str> = calls.iter().map(|(_, effect)| call_value(effect)).collect();
    assert_eq!(
        values,
        ["one", "two"],
        "both calls issue in one step, c1 first"
    );
    run.resume(calls[1].0, tool_text("second"));
    let (issued, step_events) = pending(run.step());
    events.extend(step_events);
    assert!(
        issued.is_empty(),
        "the loop waits for c1 before it goes on: {issued:?}"
    );
    run.resume(calls[0].0, tool_text("first"));
    let (next, step_events) = pending(run.step());
    events.extend(step_events);
    assert_eq!(next.len(), 1, "the next round is the one effect: {next:?}");
    assert_eq!(
        tool_messages(&next[0].1),
        ["first", "second"],
        "the records hold c1's result before c2's"
    );
    run.resume(next[0].0, text_reply("done"));
    let Step::Done { result, .. } = run.step() else {
        panic!("the reply ends the section");
    };
    assert_eq!(text_of(result), "c1=first|c2=second");
    let batch_turn = events
        .iter()
        .find_map(|event| match event {
            Event::AssistantToolCalls { turn, .. } => Some(*turn),
            _ => None,
        })
        .expect("the round reports its batch");
    let mut results: Vec<(&str, u32)> = events
        .iter()
        .filter_map(|event| match event {
            Event::ToolResult {
                turn, tool_call_id, ..
            } => Some((tool_call_id.as_str(), *turn)),
            _ => None,
        })
        .collect();
    results.sort_unstable();
    assert_eq!(results, [("c1", batch_turn), ("c2", batch_turn)]);
}

#[test]
fn a_local_call_or_a_task_builtin_runs_alone_between_its_neighbors() {
    let md = loop_prompt(
        "tools.allow_tasks()\n\
         tools.offer_local('grab', 'Local grab', { value = 'string' }, function(args)\n\
           return 'grabbed ' .. args.value\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('use them all')\n\
         models.loop(msgs)\n\
         local ids = {}\n\
         for i = 3, 6 do ids[#ids + 1] = msgs[i].tool_call_id end\n\
         return table.concat(ids, ',') .. '|' .. msgs[4].content",
    );
    let prompt = parse(&md);
    let (state, _fixture) = loop_context(&prompt, echo_tools());
    let mut run = Run::from_state(state);
    let (round, _) = only_effect(run.step());
    run.resume(
        round,
        calls_reply(&[
            ("c1", "echo", json!({ "value": "a" })),
            ("c2", "grab", json!({ "value": "b" })),
            ("c3", "task_status", json!({ "id": "0.0" })),
            ("c4", "echo", json!({ "value": "d" })),
        ]),
    );
    let (first, effect) = only_effect(run.step());
    assert_eq!(
        call_value(&effect),
        "a",
        "the batch stops before the local call"
    );
    run.resume(first, tool_text("echoed a"));
    let (last, effect) = only_effect(run.step());
    assert_eq!(
        call_value(&effect),
        "d",
        "grab and task_status answer on the driver thread, then c4 issues"
    );
    run.resume(last, tool_text("echoed d"));
    let (next, effect) = only_effect(run.step());
    assert_eq!(
        tool_messages(&effect).len(),
        4,
        "all four results reach the round"
    );
    run.resume(next, text_reply("done"));
    let Step::Done { result, .. } = run.step() else {
        panic!("the reply ends the section");
    };
    assert_eq!(text_of(result), "c1,c2,c3,c4|grabbed b");
}

#[test]
fn a_call_the_round_cannot_bind_stops_the_batch_and_raises_once_the_issued_calls_land() {
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('asked')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         return tostring(ok) .. ':' .. err.kind .. '|' .. #msgs",
    );
    let prompt = parse(&md);
    let tools = FixtureTools::new(
        vec![
            fixture_binding("echo", "echo capability", Arc::new(EchoTool)),
            fixture_binding("nope", "failing capability", Arc::new(FailingTool)),
        ],
        vec!["echo".to_owned(), "nope".to_owned()],
    );
    let (state, _fixture) = loop_context(&prompt, tools);
    let ctx = state.clone();
    let mut run = Run::from_state(state);
    let (round, _) = only_effect(run.step());
    // The round advertised `nope`; the offering then loses it, so the
    // round's answer passes the scope gate and the call binds to nothing.
    echo_tools().install(&ctx, RunFixture::new());
    run.resume(
        round,
        calls_reply(&[
            ("c1", "echo", json!({ "value": "one" })),
            ("c2", "nope", json!({ "value": "two" })),
            ("c3", "echo", json!({ "value": "three" })),
        ]),
    );
    let (issued, effect) = only_effect(run.step());
    assert_eq!(call_value(&effect), "one", "only c1 issues");
    run.resume(issued, tool_text("first"));
    let Step::Done { result, .. } = run.step() else {
        panic!("the caught raise ends the section, and c3 never issues");
    };
    assert_eq!(text_of(result), "false:unbound_tool|1");
}

#[test]
fn cancelling_a_task_parked_on_a_batch_orphans_every_call() {
    let md = "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n# Loop\n\n\
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
    let prompt = parse(md);
    let (state, _fixture) = loop_context(&prompt, echo_tools());
    let mut run = Run::from_state(state);
    let (rounds, _) = pending(run.step());
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
    run.resume(
        round,
        calls_reply(&[
            ("c1", "echo", json!({ "value": "one" })),
            ("c2", "echo", json!({ "value": "two" })),
        ]),
    );
    let (calls, _) = pending(run.step());
    assert_eq!(calls.len(), 2, "both calls are in flight: {calls:?}");
    run.resume(infer, text_reply("go"));
    // Main outlives the cancel, so a call the cancel left pending would
    // land on Child's cleared batch instead of in the run's teardown.
    let (linger, effect) = only_effect(run.step());
    assert!(
        matches!(&effect, Effect::Chat { round, .. } if round.origin == ReplyOrigin::Infer),
        "the cancel issues only Main's next round: {effect:?}"
    );
    for (id, _) in &calls {
        run.resume(*id, tool_text("late"));
    }
    let (issued, _) = pending(run.step());
    assert!(issued.is_empty(), "both orphans are discarded: {issued:?}");
    run.resume(linger, text_reply("done"));
    let Step::Done { result, .. } = run.step() else {
        panic!("Main's second round ends the run");
    };
    assert_eq!(text_of(result), "cancelled");
}
