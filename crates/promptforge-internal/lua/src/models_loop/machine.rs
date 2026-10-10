//! The rules of `models.loop` over typed inputs, with no Lua VM: the
//! round cap, the order a round's answer is judged in, the batch rule, a
//! local tool's report and re-raise, and the compactor's rules. The
//! machine pushes every record onto the list itself, and every Lua value
//! it only passes along is an opaque `V`, so the unit tests run over a
//! plain `MessageList` with any `V`.

use std::collections::BTreeMap;

use promptforge_types::metrics::ToolCallEvent;

use crate::compactors::OverflowReason;
use crate::error_value::{ErrorField, ErrorKind, Raised};
use crate::messages::MessageList;
use crate::protocol::{ChatResult, MessageContent, MessageRecord, MessageRole, ToolCallRecord};

/// The message the empty-reply rule raises when the answer names no
/// empty detail.
const EMPTY_MODEL_REPLY: &str = "empty model reply";

/// The message the round cap raises.
const NOT_CONVERGED: &str = "tool-call loop did not converge";

/// The message raised for a compactor that returns instead of raising.
const COMPACTOR_RETURNED: &str = "the selected compactor returned without raising: replacement \
                                  compactors are deferred; compactors.fail is the only shipped \
                                  policy";

/// The message raised for an input the machine's phase does not await.
const UNEXPECTED_INPUT: &str =
    "internal invariant violated: the models.loop step received an input its phase does not await";

/// One loop call: its rules' state and the values it passes along.
#[derive(Debug)]
pub(super) struct Machine<V> {
    /// The round cap: the run's `max_tool_iterations`.
    max_rounds: usize,
    /// The rounds started so far, the current one included.
    rounds: usize,
    /// The calls answered in this loop call, a tool's own failure
    /// included.
    answered: usize,
    /// The list every record the loop adds is pushed onto.
    list: MessageList,
    /// The same list as the value every `chat` request names.
    messages: V,
    /// The receiver of a handle's `loop`, set on every `chat` request;
    /// `None` for `models.loop`.
    handle: Option<V>,
    /// The compactor argument, or `compactors.fail` as read when the
    /// call began, which is not type-checked, as in the shim.
    compactor: V,
    /// The current batch's turn, sent back on every `tool_call` request.
    turn: u32,
    /// The current batch's calls, in the order the model listed them.
    calls: Vec<ToolCallEvent>,
    /// The current batch's results so far, one per answered call.
    results: Vec<String>,
    /// What the machine waits on.
    phase: Phase<V>,
}

/// What the machine waits on, which names the values the adapter reads
/// next.
#[derive(Debug)]
pub(super) enum Phase<V> {
    /// The `chat` answer: `(ok, round)`.
    Chatting,
    /// The compactor's raw `pcall`: `(ok, value)`.
    Compacting,
    /// The `tool_call` answer for the batch's next call:
    /// `(ok, result, handler, args)`.
    Calling,
    /// The handler's raw `pcall`: `(ok, value)`.
    Handling,
    /// The `local_tool_done` answer: `(ok, text)`, with the handler's own
    /// raised value when it raised.
    Reporting(Option<V>),
    /// The loop returned or raised.
    Done,
}

/// One typed input: the last request's answer, or the last call's
/// outcome.
#[derive(Debug)]
pub(super) enum Input<V> {
    /// The `chat` answer, or the value to raise.
    Answered(std::result::Result<Box<ChatResult>, V>),
    /// A bound tool's or a task built-in's text, or the value to raise.
    Dispatched(std::result::Result<String, V>),
    /// A local tool's handler and its arguments table.
    Local { handler: V, args: V },
    /// The compactor's or the handler's raw `pcall`: its first returned
    /// or raised value, and whether the run's cancel flag was set.
    Called {
        outcome: std::result::Result<V, V>,
        cancelled: bool,
    },
    /// The `local_tool_done` answer: the call's text, or the value to
    /// raise.
    Reported(std::result::Result<String, V>),
}

/// What runs next, after the records the step pushed.
#[derive(Debug)]
pub(super) enum Then<V> {
    /// Yield `{ op = "chat", messages = messages, handle = handle }`.
    Chat { messages: V, handle: Option<V> },
    /// Yield `{ op = "tool_call", alias = call.name, args = call.arguments,
    /// call_id = call.id, turn = turn }`.
    ToolCall { call: ToolCallEvent, turn: u32 },
    /// Call the compactor with the reason's tag, or nil when there is
    /// none.
    Compact {
        compactor: V,
        reason: Option<OverflowReason>,
    },
    /// Call the handler with its arguments table.
    Handle { handler: V, args: V },
    /// Yield `{ op = "local_tool_done", ok = true, value = value }` when
    /// the handler returned, or `{ op = "local_tool_done", ok = false }`
    /// when it raised.
    Report(Option<V>),
    /// Return nil.
    Return,
    /// Raise this value unchanged.
    Raise(V),
    /// Raise a compactor's failure through `normalize_failure`.
    RaiseNormalized(V),
    /// Raise a new error table.
    RaiseNew(Raised),
}

impl<V: Clone> Machine<V> {
    /// A loop over `list`, capped at `max_rounds` rounds, and its first
    /// step: the first round's `chat`, or `tool_loop_exhausted` when the
    /// cap is 0.
    pub(super) fn begin(
        max_rounds: usize,
        list: MessageList,
        messages: V,
        handle: Option<V>,
        compactor: V,
    ) -> (Self, Then<V>) {
        let mut machine = Machine {
            max_rounds,
            rounds: 0,
            answered: 0,
            list,
            messages,
            handle,
            compactor,
            turn: 0,
            calls: Vec::new(),
            results: Vec::new(),
            phase: Phase::Done,
        };
        let first = machine.next_round();
        (machine, first)
    }

    /// What the machine waits on.
    pub(super) fn phase(&self) -> &Phase<V> {
        &self.phase
    }

    /// Applies one input, pushes the records it settles, and returns what
    /// runs next. A reply is read as the renderer reads it, an empty
    /// string as no reply. An input other than the one
    /// [`Machine::phase`] names is a crate bug and raises an `internal`
    /// error.
    pub(super) fn step(&mut self, input: Input<V>) -> Then<V> {
        match (std::mem::replace(&mut self.phase, Phase::Done), input) {
            (Phase::Chatting, Input::Answered(Ok(round))) => self.judge(*round),
            (Phase::Compacting, Input::Called { outcome, cancelled }) => match outcome {
                Ok(_) => Then::RaiseNew(raised(ErrorKind::Lua, COMPACTOR_RETURNED)),
                Err(failure) if cancelled => Then::Raise(failure),
                Err(failure) => Then::RaiseNormalized(failure),
            },
            (Phase::Calling, Input::Dispatched(Ok(text)))
            | (Phase::Reporting(None), Input::Reported(Ok(text))) => {
                self.results.push(text);
                self.next_call()
            }
            (Phase::Calling, Input::Local { handler, args }) => {
                self.phase = Phase::Handling;
                Then::Handle { handler, args }
            }
            (Phase::Handling, Input::Called { outcome, cancelled }) => match outcome {
                Err(failure) if cancelled => Then::Raise(failure),
                Ok(value) => {
                    self.phase = Phase::Reporting(None);
                    Then::Report(Some(value))
                }
                Err(failure) => {
                    self.phase = Phase::Reporting(Some(failure));
                    Then::Report(None)
                }
            },
            (Phase::Reporting(Some(failure)), Input::Reported(_))
            | (Phase::Chatting, Input::Answered(Err(failure)))
            | (Phase::Calling, Input::Dispatched(Err(failure)))
            | (Phase::Reporting(None), Input::Reported(Err(failure))) => Then::Raise(failure),
            _ => Then::RaiseNew(raised(ErrorKind::Internal, UNEXPECTED_INPUT)),
        }
    }

    /// Starts the next round with its `chat`, or raises
    /// `tool_loop_exhausted` once the cap's rounds have all started.
    fn next_round(&mut self) -> Then<V> {
        if self.rounds >= self.max_rounds {
            return Then::RaiseNew(raised(ErrorKind::ToolLoopExhausted, NOT_CONVERGED));
        }
        self.rounds += 1;
        self.phase = Phase::Chatting;
        Then::Chat {
            messages: self.messages.clone(),
            handle: self.handle.clone(),
        }
    }

    /// Judges a round's answer: an overflow goes to the compactor, tool
    /// calls start a batch, a reply is appended and the loop returns, an
    /// empty reply that finished with `stop` after an answered call is the
    /// clean exit, and anything else raises `empty_model_reply`.
    fn judge(&mut self, round: ChatResult) -> Then<V> {
        if round.overflow {
            self.phase = Phase::Compacting;
            return Then::Compact {
                compactor: self.compactor.clone(),
                reason: round.overflow_reason,
            };
        }
        if let Some(calls) = round.tool_calls {
            self.turn = round.turn;
            self.calls = calls;
            self.results.clear();
            return self.next_call();
        }
        if let Some(reply) = round.reply.filter(|reply| !reply.is_empty()) {
            let record = text_record(MessageRole::Assistant, reply);
            return self.push(vec![record], |_| Then::Return);
        }
        if round.finish_reason.as_deref() == Some("stop") && self.answered > 0 {
            let record = text_record(MessageRole::Assistant, String::new());
            return self.push(vec![record], |_| Then::Return);
        }
        let mut empty = raised(ErrorKind::EmptyModelReply, EMPTY_MODEL_REPLY);
        if let Some(detail) = round.empty_detail {
            empty.message = detail;
        }
        if let Some(reason) = round.finish_reason {
            empty
                .fields
                .insert("finish_reason".to_owned(), ErrorField::String(reason));
        }
        Then::RaiseNew(empty)
    }

    /// The batch's next unanswered call, or the batch's records and the
    /// next round once every call has its result.
    fn next_call(&mut self) -> Then<V> {
        if let Some(call) = self.calls.get(self.results.len()) {
            self.phase = Phase::Calling;
            return Then::ToolCall {
                call: call.clone(),
                turn: self.turn,
            };
        }
        let calls = std::mem::take(&mut self.calls);
        let results = std::mem::take(&mut self.results);
        self.answered += calls.len();
        let mut assistant = text_record(MessageRole::Assistant, String::new());
        assistant.tool_calls = calls
            .iter()
            .map(|call| ToolCallRecord {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect();
        let mut records = vec![assistant];
        records.extend(calls.into_iter().zip(results).map(|(call, result)| {
            let mut record = text_record(MessageRole::Tool, result);
            record.tool_call_id = Some(call.id);
            record
        }));
        self.push(records, Self::next_round)
    }

    /// Pushes `records` onto the list in order, then runs `next`. The list
    /// refuses only a system record after a non-system one, which the loop
    /// never builds, so a refusal is a crate bug and raises `internal`.
    fn push(
        &mut self,
        records: Vec<MessageRecord>,
        next: impl FnOnce(&mut Self) -> Then<V>,
    ) -> Then<V> {
        for record in records {
            if let Err(message) = self.list.push(record) {
                self.phase = Phase::Done;
                return Then::RaiseNew(raised(ErrorKind::Internal, &message));
            }
        }
        next(self)
    }
}

/// A record of `role` holding the plain text `content`.
fn text_record(role: MessageRole, content: String) -> MessageRecord {
    MessageRecord {
        role,
        content: MessageContent::Text(content),
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

/// A new error of `kind` with `message` and no fields.
pub(super) fn raised(kind: ErrorKind, message: &str) -> Raised {
    Raised {
        kind,
        message: message.to_owned(),
        fields: BTreeMap::new(),
    }
}

#[cfg(test)]
#[path = "machine-tests.rs"]
mod tests;
