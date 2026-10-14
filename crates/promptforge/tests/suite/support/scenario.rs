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

use promptforge::effect::{Effect, EffectAnswer, EffectId};
use promptforge::model::{Completion, CompletionResult};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use promptforge::vfs::perform_vfs_op;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

use super::{TestResult, Tool, canned};

/// One test's setup. Every scenario sets [`canned`], numbers run events on
/// from the parse events, and fails when prepare reports a gap.
#[derive(Default)]
pub(crate) struct Scenario {
    /// The one tool the catalog lists.
    tool: Option<Tool>,
    /// The chat script, served in order.
    replies: Vec<CompletionResult>,
    /// Picks the effects that wait unanswered.
    hold: Option<fn(&Effect) -> bool>,
}

impl Scenario {
    /// A scenario that serves `replies` to the run's chat effects, in
    /// order, and registers no tool.
    pub(crate) fn replies(replies: impl IntoIterator<Item = CompletionResult>) -> Self {
        Self {
            replies: replies.into_iter().collect(),
            ..Self::default()
        }
    }

    /// A scenario whose catalog lists `tool` alone, and that serves
    /// `replies` to the run's chat effects, in order.
    pub(crate) fn tool(tool: Tool, replies: impl IntoIterator<Item = CompletionResult>) -> Self {
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

    /// Parses `source` as the run `greeter` and prepares its context.
    pub(crate) fn prepare(&self, source: &str) -> TestResult<(Prompt, RunContext)> {
        let (parsed, events) = Prompt::parse(source, "greeter");
        let prompt = parsed?;
        let mut ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH)
            .provenance_start(u32::try_from(events.len())?)
            .model(canned()?);
        if let Some(tool) = &self.tool {
            let schema = serde_json::json!({ "type": "object" });
            let descriptor =
                ToolDescriptor::new(ToolId::parse(tool.id)?, "A scenario tool.", schema);
            ctx = ctx.tools(ToolCatalog::new(&[descriptor])?);
        }
        let (ctx, requirements) = ctx.prepare(&prompt);
        match requirements.refusal() {
            Some(refusal) => Err(refusal.into()),
            None => Ok((prompt, ctx)),
        }
    }

    /// Prepares `source` and creates its run, not yet stepped.
    pub(crate) fn start(self, source: &str) -> TestResult<Driver> {
        let (prompt, ctx) = self.prepare(source)?;
        Ok(Driver {
            run: Run::new(Arc::new(prompt), "", ctx),
            replies: self.replies.into(),
            tool: self.tool,
            hold: self.hold,
            held: Vec::new(),
            result: None,
        })
    }

    /// Runs `source` to its end.
    pub(crate) fn run(self, source: &str) -> TestResult<Trace> {
        self.start(source)?.finish()
    }
}

/// A started run, stepped by hand for a test that acts mid-run.
pub(crate) struct Driver {
    pub(crate) run: Run,
    replies: VecDeque<CompletionResult>,
    tool: Option<Tool>,
    hold: Option<fn(&Effect) -> bool>,
    held: Vec<EffectId>,
    result: Option<RunResult>,
}

impl Driver {
    /// Steps the run once, answers or holds each effect the step issued,
    /// and drops the held effects it may. Returns how many it issued.
    pub(crate) fn pump(&mut self) -> TestResult<usize> {
        if self.result.is_some() {
            return Err("the run is done, so there is nothing left to step".into());
        }
        let (effects, withdrawn) = match self.run.step() {
            Step::Pending {
                effects, withdrawn, ..
            } => (effects, withdrawn),
            Step::Done { result, .. } => {
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
        let issued = effects.len();
        for (id, _provenance, effect) in effects {
            if self.hold.is_some_and(|hold| hold(&effect)) {
                self.held.push(id);
            } else {
                let answer = self.answer(effect)?;
                self.run.resume(id, answer);
            }
        }
        let decided = self.run.decided();
        let (dropped, held): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|id| decided || withdrawn.contains(id));
        self.held = held;
        for id in dropped {
            self.run.resume(id, EffectAnswer::Dropped);
        }
        Ok(issued)
    }

    /// Pumps until the run holds at least one effect.
    pub(crate) fn pump_until_held(&mut self) -> TestResult {
        while self.held.is_empty() {
            self.advance()?;
            if self.result.is_some() {
                return Err("the run finished without issuing an effect to hold".into());
            }
        }
        Ok(())
    }

    /// Pumps until the run is done, and returns its trace.
    pub(crate) fn finish(mut self) -> TestResult<Trace> {
        loop {
            if let Some(result) = self.result.take() {
                return Ok(Trace { result });
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
                EffectAnswer::Chat(Completion::from_result(reply, served).map(Box::new))
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

/// What a finished run leaves.
pub(crate) struct Trace {
    /// The run's result.
    pub(crate) result: RunResult,
}
