//! The serial helpers for suites that drive a bare [`Run`]: the canned
//! chat answer, the one-effect step, the local performer, and the batched
//! driver.

use std::collections::BTreeMap;

use promptforge_types::event::Event;
use promptforge_types::ids::{Provenance, TaskId};

use super::super::scripted_chat::{ScriptedReply, text};
use crate::execute::{Effect, EffectAnswer, EffectId, EffectRecord, Run, RunResult, Step};
use crate::lua::run_store_op;
use crate::tools::ToolError;

/// The answer a `Chat` effect gets from the scripted model's `reply`,
/// built as [`ScriptedChat::complete_now`] builds it for a round within the
/// timeout.
///
/// [`ScriptedChat::complete_now`]: super::super::scripted_chat::ScriptedChat::complete_now
pub(crate) fn chat_answer(reply: &ScriptedReply) -> EffectAnswer {
    EffectAnswer::Chat(reply.answer().map(Box::new))
}

/// The one effect a pending step issued, with its id.
///
/// # Panics
/// When the step is done or issued any other number of effects.
#[track_caller]
pub(crate) fn only_effect(step: Step) -> (EffectId, Effect) {
    let Step::Pending { mut effects, .. } = step else {
        panic!("the step is pending, got {step:?}");
    };
    assert_eq!(
        effects.len(),
        1,
        "exactly one effect is issued: {effects:?}"
    );
    let (id, _, effect) = effects.remove(0);
    (id, effect)
}

/// The first user message of a `Chat` effect, read off its record: the
/// prompt a `models.infer` round sends.
#[track_caller]
pub(crate) fn infer_prompt(effect: &Effect) -> String {
    let EffectRecord::Chat { messages, .. } = effect.record() else {
        panic!("a chat effect records its messages: {effect:?}");
    };
    messages[0]["content"]
        .as_str()
        .expect("an infer round includes one user message")
        .to_owned()
}

/// Performs one effect locally, with no I/O: a store operation runs on the
/// effect's own access handle, a timer fires at once, a bound tool is
/// unbound, and a model round is answered by
/// `chat`, a function of the effect alone so the answer never depends on
/// arrival order.
pub(crate) fn perform_locally(
    effect: &Effect,
    chat: &mut impl FnMut(&Effect) -> EffectAnswer,
) -> EffectAnswer {
    match effect {
        Effect::Chat { .. } => chat(effect),
        Effect::ToolCall { alias, .. } => EffectAnswer::ToolCall(Err(ToolError::message(format!(
            "no tool is bound as {alias} in this test"
        )))),
        Effect::Vfs { access, op } => EffectAnswer::Vfs(run_store_op(access, op.clone())),
        Effect::Timer { .. } => EffectAnswer::Timer,
    }
}

/// The text of a run that succeeded; panics on any other outcome.
#[track_caller]
pub(crate) fn text_of(result: RunResult) -> String {
    match result {
        RunResult::Ok(text) => text,
        other => panic!("the run succeeds: {other:?}"),
    }
}

/// A chat performer that echoes the infer prompt back as `r(<prompt>)`.
pub(crate) fn echo_chat(effect: &Effect) -> EffectAnswer {
    chat_answer(&text(&format!("r({})", infer_prompt(effect))))
}

/// How a batched driver delivers a step's answers.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Batching {
    /// One answer per step: the oldest outstanding effect, then step.
    OnePerStep,
    /// Every outstanding effect answered in issue order, then step.
    AllAtOnce,
    /// Every outstanding effect answered in reverse issue order, then step.
    Reversed,
}

/// Everything one driven run produced, in the forms the properties
/// compare.
pub(crate) struct Outcome {
    pub(crate) result: RunResult,
    pub(crate) events: Vec<Event>,
    pub(crate) effects: Vec<(Provenance, EffectRecord)>,
}

impl Outcome {
    /// The events grouped by task, each task's in sequence order.
    pub(crate) fn events_by_task(&self) -> BTreeMap<TaskId, Vec<Event>> {
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
    pub(crate) fn effects_by_provenance(&self) -> Vec<(Provenance, EffectRecord)> {
        let mut sorted = self.effects.clone();
        sorted.sort_by(|left, right| left.0.cmp(&right.0));
        sorted
    }

    #[track_caller]
    pub(crate) fn text(&self) -> &str {
        match &self.result {
            RunResult::Ok(text) => text,
            other => panic!("the run succeeds: {other:?}"),
        }
    }
}

/// Drives `run` under `batching`, performing through [`perform_locally`]
/// with [`echo_chat`] as the model, and records what it produced.
pub(crate) fn drive_batched(mut run: Run, batching: Batching) -> Outcome {
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
                ..
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
                let answer = |effect: &Effect| perform_locally(effect, &mut echo_chat);
                match batching {
                    Batching::OnePerStep => {
                        let (id, effect) = outstanding.remove(0);
                        run.resume(id, answer(&effect));
                    }
                    Batching::AllAtOnce => {
                        for (id, effect) in std::mem::take(&mut outstanding) {
                            run.resume(id, answer(&effect));
                        }
                    }
                    Batching::Reversed => {
                        for (id, effect) in std::mem::take(&mut outstanding).into_iter().rev() {
                            run.resume(id, answer(&effect));
                        }
                    }
                }
            }
        }
    }
}
