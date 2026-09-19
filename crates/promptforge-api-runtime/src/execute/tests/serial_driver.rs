//! The serial sans-IO driver over `Run`, and the properties it makes
//! testable without a runtime or a gateway: the doc example, a three-arm
//! fanout whose answers arrive in reverse order, determinism (two runs
//! under the same context and answers produce identical effects, events,
//! provenances, and `sys.id`s), the batching-pairing property (answers
//! delivered one per step, all at once, and shuffled within a batch
//! produce identical per-task effects and events), and a model task whose
//! owner ends first reporting `abandoned` in both its event and its
//! notice. The helpers here - the canned completions and the local
//! performer - are shared with the `task_events` suite.

use std::collections::BTreeMap;

use promptforge_api_types::event::Event;
use promptforge_api_types::ids::{AbandonReason, Provenance, TaskId};

use super::model_tasks::{NeverBroker, model_task_context_with};
use super::scheduler::scheduler_context_from;
use super::*;
use crate::client::{Completion, CompletionResult, ToolCall};
use crate::execute::run::{Effect, EffectAnswer, EffectId, EffectRecord, Run, Step};
use crate::execute::task_history;
use crate::input::InputOutcome;
use crate::lua::run_store_op;
use crate::store::Store;
use crate::test_support::drive;

/// A canned text reply from the test model.
pub(super) fn text_reply(text: &str) -> EffectAnswer {
    EffectAnswer::Chat(Ok(Box::new(Completion::from_result(
        CompletionResult::Text(text.to_owned()),
        "test-model",
    ))))
}

/// A canned tool-call round from the test model: one call, `name` with
/// `arguments`, under `call_id`.
pub(super) fn tool_call_reply(call_id: &str, name: &str, arguments: Value) -> EffectAnswer {
    EffectAnswer::Chat(Ok(Box::new(Completion::from_result(
        CompletionResult::ToolCalls(vec![ToolCall::from_parts(call_id, name, arguments)]),
        "test-model",
    ))))
}

/// The first user message of a `Chat` effect, read off its record: the
/// prompt a `models.infer` round carries.
pub(super) fn infer_prompt(effect: &Effect) -> String {
    let EffectRecord::Chat { messages, .. } = effect.record() else {
        panic!("a chat effect records its messages: {effect:?}");
    };
    messages[0]["content"]
        .as_str()
        .expect("an infer round carries one user message")
        .to_owned()
}

/// Performs one effect locally, with no I/O: a store operation runs on the
/// effect's own access handle, a timer fires at once, an input wait is
/// unavailable, a bound tool is unbound, and a model round is answered by
/// `chat`, a function of the effect alone so the answer never depends on
/// arrival order.
pub(super) fn perform_locally(
    effect: &Effect,
    chat: &mut impl FnMut(&Effect) -> EffectAnswer,
) -> EffectAnswer {
    match effect {
        Effect::Chat { .. } => chat(effect),
        Effect::ToolCall { alias, .. } => EffectAnswer::ToolCall(Err(ToolError::message(format!(
            "no tool is bound as {alias} in this test"
        )))),
        Effect::UserInput { .. } => EffectAnswer::UserInput(Ok(InputOutcome::Unavailable)),
        Effect::Store { access, op } => {
            EffectAnswer::Store(run_store_op(&Store::new(access), op.clone()))
        }
        Effect::Timer { .. } => EffectAnswer::Timer,
        Effect::TaskEvents { .. } => panic!("the driver answers a history read itself"),
    }
}

/// A chat performer that echoes the infer prompt back as `r(<prompt>)`.
fn echo_chat(effect: &Effect) -> EffectAnswer {
    text_reply(&format!("r({})", infer_prompt(effect)))
}

/// A run over `md` with the `writer` model pre-bound, as the scheduler
/// suites build one.
fn model_run(md: &str) -> Run {
    let prompt = parse(md);
    Run::from_state(scheduler_context_from(
        &prompt,
        &TestStore::new(),
        &test_context(EXECUTION),
    ))
}

/// The three-arm fanout every property here runs: the parent names its
/// own `sys.id` and each arm's result, each arm names its own `sys.id`
/// and its one model round.
const FANOUT: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Fanout\n\n\
    ## Parent\n\n\
    ```lua\n\
    store.write('seed.txt', 'planted')\n\
    local r = fanout('### Worker', {'a', 'b', 'c'})\n\
    return sys.id .. '|' .. r[1].text .. '|' .. r[2].text .. '|' .. r[3].text\n\
    ```\n\n\
    ### Worker\n\n\
    ```lua\n\
    return sys.id .. '=' .. models.infer(item)\n\
    ```\n";

/// How a batched driver delivers a step's answers.
#[derive(Clone, Copy, Debug)]
enum Batching {
    /// One answer per step: the oldest outstanding effect, then step.
    OnePerStep,
    /// Every outstanding effect answered in issue order, then step.
    AllAtOnce,
    /// Every outstanding effect answered in reverse issue order, then step.
    Reversed,
}

/// Everything one driven run produced, in the forms the properties
/// compare.
struct Outcome {
    result: RunResult,
    events: Vec<Event>,
    effects: Vec<(Provenance, EffectRecord)>,
}

impl Outcome {
    /// The events grouped by task, each task's in sequence order.
    fn events_by_task(&self) -> BTreeMap<TaskId, Vec<Event>> {
        let mut grouped: BTreeMap<TaskId, Vec<Event>> = BTreeMap::new();
        for event in &self.events {
            grouped
                .entry(event.provenance().task.clone())
                .or_default()
                .push(event.clone());
        }
        grouped
    }

    /// The effects sorted by provenance, so two runs whose steps issued
    /// them in different interleavings compare equal.
    fn effects_by_provenance(&self) -> Vec<(Provenance, EffectRecord)> {
        let mut sorted = self.effects.clone();
        sorted.sort_by(|left, right| left.0.cmp(&right.0));
        sorted
    }

    fn text(&self) -> &str {
        match &self.result {
            RunResult::Ok(text) => text,
            other => panic!("the run succeeds: {other:?}"),
        }
    }
}

/// Drives `run` under `batching`, performing through [`perform_locally`]
/// with `echo_chat` as the model, and records what it produced.
fn drive_batched(mut run: Run, batching: Batching) -> Outcome {
    let mut events = Vec::new();
    let mut effects = Vec::new();
    let mut outstanding: Vec<(EffectId, Effect)> = Vec::new();
    loop {
        match run.step() {
            Step::Done {
                result,
                events: more,
            } => {
                events.extend(more);
                return Outcome {
                    result,
                    events,
                    effects,
                };
            }
            Step::Pending {
                effects: issued,
                events: more,
            } => {
                events.extend(more);
                for (id, provenance, effect) in issued {
                    effects.push((provenance, effect.record()));
                    outstanding.push((id, effect));
                }
                assert!(
                    !outstanding.is_empty(),
                    "a pending run has an effect to answer"
                );
                let answer = |effect: &Effect, events: &[Event]| match effect {
                    Effect::TaskEvents { task, last } => {
                        EffectAnswer::TaskEvents(task_history(events, task, *last))
                    }
                    other => perform_locally(other, &mut echo_chat),
                };
                match batching {
                    Batching::OnePerStep => {
                        let (id, effect) = outstanding.remove(0);
                        run.resume(id, answer(&effect, &events));
                    }
                    Batching::AllAtOnce => {
                        for (id, effect) in std::mem::take(&mut outstanding) {
                            run.resume(id, answer(&effect, &events));
                        }
                    }
                    Batching::Reversed => {
                        for (id, effect) in std::mem::take(&mut outstanding).into_iter().rev() {
                            run.resume(id, answer(&effect, &events));
                        }
                    }
                }
            }
        }
    }
}

/// The task ids of every `TaskSucceeded` in `events`, in order.
fn succeeded(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::TaskSucceeded { task, .. } => Some(task.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_driver_doc_example_runs_a_literal_prompt_with_no_effect() {
    // The `drive` doc example, pinned as a test: a literal return issues
    // nothing, so the performer never runs, and the events open and close
    // with the run's boundaries.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# Title\n\n## Only\n\n```lua\nreturn 'hello'\n```\n";
    let run = Run::new(Arc::new(parse(md)), "", test_context(EXECUTION));
    let (result, events) = drive(run, |_, effect| panic!("no effect is issued: {effect:?}"));
    let RunResult::Ok(text) = result else {
        panic!("the literal run succeeds: {result:?}");
    };
    assert_eq!(text, "hello");
    assert!(matches!(events.first(), Some(Event::RunStarted { .. })));
    assert!(matches!(events.last(), Some(Event::RunSucceeded { .. })));
}

#[test]
fn the_driver_performs_store_and_model_effects_and_keeps_the_events_in_order() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        store.write('notes.md', 'kept')\n\
        return store.read('notes.md') .. '/' .. models.infer('ask')\n\
        ```\n";
    let (result, events) = drive(model_run(md), |_, effect| {
        perform_locally(effect, &mut echo_chat)
    });
    let RunResult::Ok(text) = result else {
        panic!("the run succeeds: {result:?}");
    };
    assert_eq!(text, "kept/r(ask)");
    let order: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::StoreWriteSucceeded { .. } => Some("write"),
            Event::StoreReadSucceeded { .. } => Some("read"),
            Event::ModelTurnCompleted { .. } => Some("turn"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["write", "read", "turn"]);
}

#[test]
fn a_three_arm_fanout_fed_its_answers_in_reverse_order_packs_results_in_collection_order() {
    // One step issues all three arms' rounds; the answers land c, b, a.
    // The arms finish in that order (their terminals say so), yet the
    // parent's results follow the collection, and every id is the
    // hierarchical one the arms would have under any order.
    let mut run = model_run(FANOUT);
    let Step::Pending { effects, .. } = run.step() else {
        panic!("the fanout parks on its arms' rounds");
    };
    let mut effects = effects;
    let Effect::Store { .. } = &effects[0].2 else {
        panic!("the parent's store write is issued first: {effects:?}");
    };
    let (id, _, store) = effects.remove(0);
    run.resume(id, perform_locally(&store, &mut echo_chat));
    let Step::Pending { effects, .. } = run.step() else {
        panic!("the fanout parks on its arms' rounds");
    };
    let prompts: Vec<String> = effects
        .iter()
        .map(|(_, _, effect)| infer_prompt(effect))
        .collect();
    assert_eq!(prompts, ["a", "b", "c"], "all three arms issue in one step");
    for (id, _, effect) in effects.into_iter().rev() {
        run.resume(id, echo_chat(&effect));
    }
    let Step::Done { result, events } = run.step() else {
        panic!("every arm is answered, so the fanout completes");
    };
    let RunResult::Ok(text) = result else {
        panic!("the fanout succeeds: {result:?}");
    };
    assert_eq!(text, "0.1|0.0.0=r(a)|0.1.0=r(b)|0.2.0=r(c)");
    assert_eq!(
        succeeded(&events),
        ["0.2", "0.1", "0.0"],
        "the arms end in answer order, not collection order"
    );
}

#[test]
fn two_runs_under_the_same_context_and_answers_are_identical() {
    // The determinism property: the same prompt, context, and answers
    // produce the same text (the `sys.id`s in it), the same events in the
    // same order with the same provenances, and the same effects.
    let first = drive_batched(model_run(FANOUT), Batching::AllAtOnce);
    let second = drive_batched(model_run(FANOUT), Batching::AllAtOnce);
    assert_eq!(first.text(), "0.1|0.0.0=r(a)|0.1.0=r(b)|0.2.0=r(c)");
    assert_eq!(first.text(), second.text());
    assert_eq!(
        first.events, second.events,
        "identical events and provenances"
    );
    assert_eq!(first.effects, second.effects, "identical effects");
    assert!(
        first.effects.len() >= 4,
        "the store write and three rounds are effects: {:?}",
        first.effects
    );
}

#[test]
fn answers_one_per_step_all_at_once_and_reversed_produce_the_same_per_task_record() {
    // The batching-pairing property: however the host paces and orders
    // its answers, each task's effects and events - and the text with its
    // `sys.id`s - are the same. Only the interleaving across tasks may
    // differ, so the comparison is per task.
    let one = drive_batched(model_run(FANOUT), Batching::OnePerStep);
    let all = drive_batched(model_run(FANOUT), Batching::AllAtOnce);
    let reversed = drive_batched(model_run(FANOUT), Batching::Reversed);
    for other in [&all, &reversed] {
        assert_eq!(one.text(), other.text());
        assert_eq!(one.events_by_task(), other.events_by_task());
        assert_eq!(one.effects_by_provenance(), other.effects_by_provenance());
    }
    assert_ne!(
        succeeded(&all.events),
        succeeded(&reversed.events),
        "the strategies differ in arrival order, so the property is not vacuous"
    );
    assert_eq!(one.events_by_task().len(), 4, "the parent and three arms");
}

#[test]
fn a_model_task_whose_owner_ends_first_reports_abandoned_in_its_event_and_its_notice() {
    // Round 1 starts the task; round 2 ends the loop and the section
    // returns while the child is still live. The child's terminal is
    // `TaskAbandoned` (not cancelled: it lost its owner rather than being
    // stopped on purpose), and the notice queued for the model says so.
    let md = "---\nname: mt\ndescription: d\npromptforge: 0\n---\n\n\
        # ModelTasks\n\n\
        ## Only\n\n\
        ```lua\n\
        tools.allow_tasks({ '## Child' })\n\
        local msgs = messages.new()\n\
        msgs:user('go')\n\
        models.loop(msgs)\n\
        return 'owner done'\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\n\
        return models.infer('child work')\n\
        ```\n";
    let prompt = parse(md);
    let state = model_task_context_with(
        &prompt,
        Arc::new(NullObserver::default()),
        Arc::new(NeverBroker),
    );
    let mut rounds = 0;
    let (result, events) = drive(Run::from_state(state), |_, effect| {
        perform_locally(effect, &mut |effect| {
            if infer_prompt(effect) == "child work" {
                return text_reply("child result");
            }
            rounds += 1;
            match rounds {
                1 => tool_call_reply("call_1", "task", json!({ "target": "## Child" })),
                _ => text_reply("bye"),
            }
        })
    });
    let RunResult::Ok(text) = result else {
        panic!("the owner returns: {result:?}");
    };
    assert_eq!(text, "owner done");
    let child: TaskId = "0.0".parse().expect("a task id parses");
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::TaskAbandoned { task, reason: AbandonReason::OwnerReturned, .. } if *task == child
        )),
        "the child's terminal is abandoned with the owner's return as the reason: {events:?}"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            Event::TaskCancelled { task, .. } | Event::TaskSucceeded { task, .. } if *task == child
        )),
        "abandoned is the child's only terminal: {events:?}"
    );
    let notice = events
        .iter()
        .find_map(|event| match event {
            Event::TaskNotice { task, text, .. } if *task == child => Some(text.clone()),
            _ => None,
        })
        .expect("the abandonment queues a notice for the model");
    assert!(
        notice.starts_with("Task id=0.0 (## Child) was abandoned: "),
        "the notice names the task and says abandoned: {notice}"
    );
}
