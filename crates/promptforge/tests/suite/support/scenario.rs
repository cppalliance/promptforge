//! Scenarios: a facade test's setup as data, the [`Driver`] that steps its
//! run, and the [`Trace`] a finished run leaves.
//!
//! The driver answers a store effect with `perform_vfs_op` and a timer as
//! fired at once, a chat effect with the next scripted reply served under
//! the bound model's name, and a tool call with the registered handler. An
//! effect the scenario holds waits, and gets `Dropped` once the run is
//! decided or withdraws it; no other effect is ever dropped. The driver
//! fails the test instead of hanging when the chat script runs out, when
//! scripted replies are left over at `Done`, or when an undecided run
//! issues nothing new while only held effects remain. It also fails a run
//! that reports `Done` while it still holds an effect, because `Done`
//! waits for every issued effect's answer.

use std::collections::VecDeque;
use std::sync::Arc;

use promptforge::effect::{AnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord};
use promptforge::event::Event;
use promptforge::ids::Provenance;
use promptforge::model::{Completion, ModelDescriptor};
use promptforge::timestamp::Timestamp;
use promptforge::tools::ToolCatalog;
use promptforge::vfs::perform_vfs_op;
use promptforge::{Prompt, Requirements, Run, RunContext, RunResult, Step};
use serde_json::json;

use super::{Reply, TestResult, Tool, canned, descriptor};

type Edit = Box<dyn Fn(RunContext) -> RunContext>;

/// One test's setup. Every scenario sets [`canned`] unless it names a
/// model, numbers run events on from the parse events, and fails to start
/// when prepare reports a gap.
#[derive(Default)]
pub(crate) struct Scenario {
    args: String,
    started_at: Timestamp,
    model: Option<ModelDescriptor>,
    tool: Option<Tool>,
    replies: Vec<Reply>,
    hold: Option<fn(&Effect) -> bool>,
    reverse: bool,
    context: Option<Edit>,
}

impl Scenario {
    /// A scenario that serves `replies` to the run's chat effects, in
    /// order, and registers no tool.
    pub(crate) fn replies(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            replies: replies.into_iter().collect(),
            ..Self::default()
        }
    }

    /// A scenario whose catalog lists `tool` alone, and that serves
    /// `replies` to the run's chat effects, in order.
    pub(crate) fn tool(tool: Tool, replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            tool: Some(tool),
            ..Self::replies(replies)
        }
    }

    /// Holds each effect `hold` picks, unanswered, until the run is decided
    /// or withdraws it.
    pub(crate) fn hold(mut self, hold: fn(&Effect) -> bool) -> Self {
        self.hold = Some(hold);
        self
    }

    /// Runs the prompt with `args` as its argument text.
    pub(crate) fn args(mut self, args: &str) -> Self {
        args.clone_into(&mut self.args);
        self
    }

    /// Starts the run at `started_at`, which the prompt reads as `sys.when`.
    pub(crate) fn started_at(mut self, started_at: Timestamp) -> Self {
        self.started_at = started_at;
        self
    }

    /// Sets `model` as the run's current model, in place of [`canned`].
    pub(crate) fn model(mut self, model: ModelDescriptor) -> Self {
        self.model = Some(model);
        self
    }

    /// Answers each batch of effects in reverse issue order.
    pub(crate) fn reverse(mut self) -> Self {
        self.reverse = true;
        self
    }

    /// Edits the run's context after the scenario sets its own, so an edit
    /// can also undo one of them.
    pub(crate) fn context(mut self, edit: impl Fn(RunContext) -> RunContext + 'static) -> Self {
        self.context = Some(Box::new(edit));
        self
    }

    /// Parses `source` as the run `greeter` and prepares its context,
    /// whatever prepare reports. [`start`](Self::start) is what fails on
    /// a gap.
    pub(crate) fn prepare(&self, source: &str) -> TestResult<Prepared> {
        let (parsed, events) = Prompt::parse(source, "greeter");
        let prompt = parsed?;
        let model = match &self.model {
            Some(model) => model.clone(),
            None => canned()?,
        };
        let mut ctx = RunContext::new("greeter", 7, self.started_at)
            .provenance_start(u32::try_from(events.len())?)
            .model(model);
        if let Some(tool) = &self.tool {
            let descriptor = descriptor(tool.id, json!({ "type": "object" }))?;
            ctx = ctx.tools(ToolCatalog::new(&[descriptor])?);
        }
        if let Some(edit) = &self.context {
            ctx = edit(ctx);
        }
        let (ctx, requirements) = ctx.prepare(&prompt);
        Ok(Prepared {
            prompt,
            ctx,
            requirements,
            events,
        })
    }

    /// Prepares `source` and creates its run, not yet stepped. Fails when
    /// prepare reports a gap.
    pub(crate) fn start(self, source: &str) -> TestResult<Driver> {
        let Prepared {
            prompt,
            ctx,
            requirements,
            events,
        } = self.prepare(source)?;
        if let Some(refusal) = requirements.refusal() {
            return Err(refusal.into());
        }
        Ok(Driver {
            run: Run::new(Arc::new(prompt), &self.args, ctx),
            replies: self.replies.into(),
            tool: self.tool,
            hold: self.hold,
            reverse: self.reverse,
            held: Vec::new(),
            events,
            effects: Vec::new(),
            result: None,
        })
    }

    pub(crate) fn run(self, source: &str) -> TestResult<Trace> {
        self.start(source)?.finish()
    }
}

pub(crate) struct Prepared {
    pub(crate) prompt: Prompt,
    pub(crate) ctx: RunContext,
    pub(crate) requirements: Requirements,
    /// The parse events.
    pub(crate) events: Vec<Event>,
}

/// A started run, stepped by hand for a test that acts mid-run.
pub(crate) struct Driver {
    pub(crate) run: Run,
    replies: VecDeque<Reply>,
    tool: Option<Tool>,
    hold: Option<fn(&Effect) -> bool>,
    reverse: bool,
    held: Vec<Issued>,
    events: Vec<Event>,
    effects: Vec<Logged>,
    result: Option<RunResult>,
}

struct Issued {
    id: EffectId,
    provenance: Provenance,
    effect: EffectRecord,
}

impl Driver {
    /// Steps the run once, answers or holds each effect the step issued,
    /// and drops the held effects it may. Returns how many it issued.
    pub(crate) fn pump(&mut self) -> TestResult<usize> {
        if self.result.is_some() {
            return Err("the run is done, so there is nothing left to step".into());
        }
        let (mut effects, withdrawn) = match self.run.step() {
            Step::Pending {
                effects,
                events,
                withdrawn,
            } => {
                self.events.extend(events);
                (effects, withdrawn)
            }
            Step::Done { result, events } => {
                self.events.extend(events);
                let held = self.held.len();
                if held > 0 {
                    return Err(format!("the run ended with {held} held effects unanswered").into());
                }
                let left = self.replies.len();
                if left > 0 {
                    return Err(format!("the run ended with {left} scripted replies left").into());
                }
                self.result = Some(result);
                return Ok(0);
            }
        };
        if self.reverse {
            effects.reverse();
        }
        let issued = effects.len();
        for (id, provenance, effect) in effects {
            let effect_record = effect.record();
            let held = Issued {
                id,
                provenance,
                effect: effect_record,
            };
            if self.hold.is_some_and(|hold| hold(&effect)) {
                self.held.push(held);
            } else {
                let answer = self.answer(effect)?;
                self.respond(held, answer);
            }
        }
        let decided = self.run.decided();
        let (dropped, held): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|issued| decided || withdrawn.contains(&issued.id));
        self.held = held;
        for issued in dropped {
            self.respond(issued, EffectAnswer::Dropped);
        }
        Ok(issued)
    }

    fn respond(&mut self, issued: Issued, answer: EffectAnswer) {
        self.effects.push(Logged {
            provenance: issued.provenance,
            effect: issued.effect,
            answer: answer.record(),
        });
        self.run.resume(issued.id, answer);
    }

    pub(crate) fn pump_until_held(&mut self) -> TestResult {
        while self.held.is_empty() {
            self.advance()?;
            if self.result.is_some() {
                return Err("the run finished without issuing an effect to hold".into());
            }
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> TestResult<Trace> {
        loop {
            if let Some(result) = self.result.take() {
                return Ok(Trace {
                    result,
                    events: self.events,
                    effects: self.effects,
                });
            }
            self.advance()?;
        }
    }

    /// Pumps once, and fails when another step could only wait forever.
    fn advance(&mut self) -> TestResult {
        let held = self.held.len();
        let issued = self.pump()?;
        let waiting = self.result.is_none() && !self.run.decided();
        if waiting && issued == 0 && self.held.len() == held {
            return Err(format!(
                "the undecided run issued nothing new while only {held} held effects remain"
            )
            .into());
        }
        Ok(())
    }

    fn answer(&mut self, effect: Effect) -> TestResult<EffectAnswer> {
        Ok(match effect {
            Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
            Effect::Timer { .. } => EffectAnswer::Timer,
            Effect::Chat { binding, .. } => {
                let reply = self
                    .replies
                    .pop_front()
                    .ok_or("the run asked for a model reply after the chat script ran out")?;
                let served = binding.id().name();
                let completion = match reply {
                    Reply::Completion(result) => Completion::from_result(result, served),
                    Reply::Failure(error) => Err(error),
                };
                EffectAnswer::Chat(completion.map(Box::new))
            }
            Effect::ToolCall { tool, args, .. } => match &self.tool {
                Some(registered) if tool.to_string() == registered.id => {
                    EffectAnswer::ToolCall((registered.handler)(&args))
                }
                _ => {
                    let error = format!(
                        "the run called {tool}, and the scenario registers no handler for it"
                    );
                    return Err(error.into());
                }
            },
        })
    }
}

pub(crate) struct Trace {
    pub(crate) result: RunResult,
    /// The parse events, then the run's events, in report order.
    pub(crate) events: Vec<Event>,
    /// Every effect the run issued, in the order the driver answered them.
    pub(crate) effects: Vec<Logged>,
}

impl Trace {
    /// The text of a `RunResult::Ok`. Fails on any other result.
    pub(crate) fn text(&self) -> TestResult<&str> {
        match &self.result {
            RunResult::Ok(text) => Ok(text),
            result => Err(format!("expected RunResult::Ok, got {result:?}").into()),
        }
    }
}

pub(crate) struct Logged {
    pub(crate) provenance: Provenance,
    pub(crate) effect: EffectRecord,
    pub(crate) answer: AnswerRecord,
}
