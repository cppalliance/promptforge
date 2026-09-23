//! Answer application: the one path every performed effect's answer takes
//! back into the scheduler.
//!
//! The host hands back a raw [`EffectAnswer`] - a completion, a tool's own
//! output, a broker outcome, a store outcome, a timer's firing - and knows
//! nothing of what the parked chain asked for. `apply_answer` pairs the
//! answer with the effect's [`Continuation`] and turns it into the chain's
//! protocol [`Answer`] on the caller's thread, emitting the round's events
//! there: the model turn's boundaries and content, the tool call's
//! succeeded/failed event and `ToolResult` under the trust rule, the
//! operator's input, the store operation's outcome, a task history read's
//! events as the shim's sequence or the model's untrusted text. A timer's
//! firing completes its slot and wakes the waiter instead of resuming a
//! chain. A `Dropped` answer resumes the chain with the cancelled error,
//! whatever it was parked on.

use promptforge_api_types::tools::{ToolError, ToolOutput};

use crate::execute::protocol::{Answer, StoreOutcome, ToolCallOutcome};
use crate::execute::tools::accept_infer;
use crate::input::{INPUT_UNAVAILABLE_FALLBACK, InputError, InputOutcome};
use crate::lua::{ModelReport, UserInputOutcome, prepare_dispatch, prepare_model_dispatch};
use crate::model::{Completion, CompletionError};
use crate::store::StoreError;
use crate::{Error, Result};
use promptforge_api_types::event::lifecycle::Lifecycle;

use super::dispatch::classify_store_failure;
use super::tasks::{TaskBacking, TaskState};
use super::{
    ChainIndex, Continuation, EffectAnswer, EffectId, Pending, Scheduler, ToolCallContinuation,
};

/// The cancelled answer for a chain parked on `resume`'s kind of effect:
/// the protocol variant the chain's shim expects, holding the run's
/// cancellation error. A timer resumes no chain; its drop is applied to
/// its slot instead, before this is reached.
fn dropped_answer(resume: &Continuation) -> Answer<Error> {
    match resume {
        Continuation::Infer => Answer::Infer(Err(Error::Interrupted)),
        Continuation::Chat => Answer::Chat(Err(Error::Interrupted)),
        Continuation::ToolCall(_) => Answer::ToolCallResult(Err(Error::Interrupted)),
        Continuation::UserInput => Answer::UserInput(Err(Error::Interrupted)),
        // A timer's drop never reaches here; the cancelled store answer
        // is the harmless stand-in should it ever do so.
        Continuation::Store(_) | Continuation::Timer => Answer::Store(Err(Error::Interrupted)),
        Continuation::TaskEvents(reader) => reader.dropped(),
    }
}

impl Scheduler {
    /// Applies one performed effect's answer: removes the effect's pending
    /// entry, turns the raw answer into the parked chain's protocol answer
    /// under the effect's continuation (emitting the round's events), and
    /// re-queues the chain. A timer's firing completes its slot and wakes
    /// its waiter instead. A `Dropped` answer is the host giving the
    /// effect up: the chain resumes with the cancelled error.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when no pending entry explains the id
    /// (the caller has already ruled out an orphan, so the host answered
    /// an effect the run never issued or answered one twice - which fails
    /// loudly), or when the answer's kind does not match the effect's.
    /// Returns [`Error::Determinism`] when a store answer reports a
    /// claims-model conflict: the run ends on the spot rather than
    /// resuming the conflict into Lua, where an author `pcall` could catch
    /// it.
    pub(super) fn apply_answer(&mut self, id: EffectId, answer: EffectAnswer) -> Result<()> {
        let Some(Pending { chain, resume }) = self.pending.remove(&id) else {
            return Err(Error::internal(
                "an answer arrived for an effect the run did not issue or already answered",
            ));
        };
        let answer = match (resume, answer) {
            (Continuation::Timer, EffectAnswer::Dropped) => {
                self.drop_timer(id);
                return Ok(());
            }
            (resume, EffectAnswer::Dropped) => dropped_answer(&resume),
            (Continuation::Infer, EffectAnswer::Chat(result)) => {
                Answer::Infer(self.accept_infer(chain, result))
            }
            (Continuation::Chat, EffectAnswer::Chat(result)) => {
                self.accept_chat(chain, result.map_err(Error::from))?
            }
            (Continuation::ToolCall(call), EffectAnswer::ToolCall(result)) => {
                Answer::ToolCallResult(self.accept_tool_call(chain, &call, result))
            }
            (Continuation::UserInput, EffectAnswer::UserInput(result)) => {
                Answer::UserInput(self.accept_user_input(chain, result))
            }
            (Continuation::Store(observations), EffectAnswer::Store(result)) => {
                match self.accept_store(chain, observations, result) {
                    // A claims-model conflict is fatal: the suspended
                    // chains drop unarmed in the run's teardown, exactly
                    // as on the cancellation path.
                    Err(error @ Error::Determinism(_)) => return Err(error),
                    result => Answer::Store(result),
                }
            }
            (Continuation::Timer, EffectAnswer::Timer) => return self.fire_timer(id),
            (Continuation::TaskEvents(reader), EffectAnswer::TaskEvents(events)) => {
                self.accept_task_events(chain, &reader, events)
            }
            _ => {
                return Err(Error::internal(
                    "an effect's answer must be of the effect's own kind",
                ));
            }
        };
        self.answer_inline(chain, answer);
        Ok(())
    }

    /// Applies a nested infer round's completion: the single-prose-round
    /// reporting through the chain's own emitter, then the round's text.
    fn accept_infer(
        &self,
        chain: ChainIndex,
        result: std::result::Result<Box<Completion>, CompletionError>,
    ) -> Result<String> {
        let chain = &self.chains[chain.index()];
        accept_infer(
            result,
            chain.ctx.emitter(),
            chain.section_name(),
            chain.ctx.turns(),
        )
    }

    /// Applies a bound tool call's own answer through the shared dispatch
    /// body: the succeeded/failed event, the trust rule, and the
    /// `ToolResult` report - under the model's call id when the model
    /// issued the call (a tool's own failure then resumes as untrusted
    /// failure text), else as a script call classified by the binding's
    /// declared output kind. The counts were taken at dispatch, so the
    /// body is handed `None` for them.
    fn accept_tool_call(
        &self,
        chain: ChainIndex,
        call: &ToolCallContinuation,
        result: std::result::Result<ToolOutput, ToolError>,
    ) -> Result<ToolCallOutcome> {
        let chain = &self.chains[chain.index()];
        // The shared dispatch body reports through this chain's emitter, so
        // its reports land in the buffer under this chain's task.
        let emitter = chain.ctx.emitter();
        let section = chain.section_name();
        let nonce = chain.ctx.nonce();
        match &call.call_id {
            // Model-issued: the content always resumes, plain - it is the
            // tool record's text for the next round, never classified by
            // output kind.
            Some(call_id) => {
                let report = ModelReport {
                    script: call.report,
                    call_id: call_id.clone(),
                };
                prepare_model_dispatch(
                    &call.binding,
                    result,
                    None,
                    nonce,
                    emitter,
                    section,
                    &report,
                )
                .map(|outcome| ToolCallOutcome::Plain(outcome.into_content()))
                .map_err(Error::from)
            }
            None => match prepare_dispatch(
                &call.binding,
                result,
                None,
                nonce,
                emitter,
                section,
                Some(call.report),
            ) {
                Ok(outcome) => ToolCallOutcome::from_dispatch(
                    call.binding.output_kind,
                    call.binding.alias(),
                    outcome.into_content(),
                )
                .map_err(Error::from),
                Err(error) => Err(Error::from(error)),
            },
        }
    }

    /// Applies a broker's answer: delivered text is reported byte-exact
    /// and resumes with `available` true; an unavailable answer is the
    /// fixed fallback sentence with `available` false and records no
    /// input; a broker failure is the call's typed input error.
    fn accept_user_input(
        &self,
        chain: ChainIndex,
        result: std::result::Result<InputOutcome, InputError>,
    ) -> Result<UserInputOutcome> {
        let chain = &self.chains[chain.index()];
        match result {
            Ok(InputOutcome::Text(text)) => {
                chain.ctx.emitter().user_input(chain.section_name(), &text);
                Ok(UserInputOutcome {
                    text,
                    available: true,
                })
            }
            Ok(InputOutcome::Unavailable) => Ok(UserInputOutcome {
                text: INPUT_UNAVAILABLE_FALLBACK.to_owned(),
                available: false,
            }),
            Err(error) => Err(Error::from(error)),
        }
    }

    /// Applies a store operation's answer: the operation's succeeded or
    /// failed observation (pushed before the chain resumes, so the op's
    /// outcome precedes the chunk's closing boundary), then the outcome,
    /// with a failure classified for the answer channel.
    fn accept_store(
        &self,
        chain: ChainIndex,
        observations: Option<(Lifecycle, Lifecycle)>,
        result: std::result::Result<StoreOutcome, StoreError>,
    ) -> Result<StoreOutcome> {
        let chain = &self.chains[chain.index()];
        if let Some((succeeded, failed)) = observations {
            chain.ctx.emitter().report(
                chain.section_name(),
                if result.is_ok() { succeeded } else { failed },
            );
        }
        result.map_err(|error| classify_store_failure(&error))
    }

    /// Applies a dropped timer: the slot backed by the effect moves to
    /// `Cancelled` without waking its owner. A host drops a live timer
    /// only when it is cancelling the run, and that cancel tears the
    /// waiter down with every other chain.
    fn drop_timer(&mut self, effect: EffectId) {
        if let Some(slot) = self
            .tasks
            .values_mut()
            .find(|slot| slot.backing == TaskBacking::Effect(effect) && slot.state.is_live())
        {
            slot.state = TaskState::Cancelled;
            slot.ok = Some(false);
        }
    }
}
