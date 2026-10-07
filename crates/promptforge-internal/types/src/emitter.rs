//! The run-level event buffer and the task-scoped emitter over it.
//!
//! The Engine reports as values: every boundary, content report, and
//! debug capture becomes one [`Event`] pushed into the run's buffer,
//! stamped with a [`Provenance`] - the nearest enclosing task and that
//! task's next sequence number - and drained by the run's `step` through
//! the [`EventSink`]. The caller reads each drained batch, the one path
//! by which an event leaves this module.
//!
//! An [`Emitter`] is one chain's handle onto the buffer: it knows its task
//! (the main walk is task `0`; a `call` child shares its caller's emitter,
//! so it reports under the caller's task; a spawned chain gets its own
//! through [`Emitter::for_task`]) and stamps every event it pushes with
//! that task's next `seq`. The counters sit in the buffer, under its one
//! lock, so a task's sequence is dense from zero however its chains and
//! the run's leaf tasks interleave.
//!
//! The emitter is the one reporting seam every Engine crate takes: the
//! parser reports parse-time compilation through it, the section VM its
//! chunk boundaries, the tool-dispatch body its results, the scheduler
//! everything else. It sits in this crate so those crates can name it
//! without depending on the runtime.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::event::Event;
use crate::event::ReplyOrigin;
use crate::event::lifecycle::Lifecycle;
use crate::ids::{ChainId, Provenance, RoundId, TaskId};
use crate::metrics::{CallMetrics, ToolCallEvent};
use crate::tools::{OutputTrust, ToolId};

#[cfg(test)]
#[path = "emitter-tests.rs"]
mod tests;

/// Whether a run captures each model round's raw request and response
/// bodies as `Request` and `Response` events.
///
/// Off by default. The `Chat` effect and its answer already carry the
/// bodies, so a caller that logs effects has them. A caller that also
/// wants them in the event stream selects `On`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DebugMode {
    /// The model rounds skip the `Request` and `Response` events and
    /// the body clones those events need.
    #[default]
    Off,
    /// Every model round emits its raw request and response bodies.
    On,
}

/// The run's event buffer: the events not yet drained, plus one sequence
/// counter per task the run has reported under.
#[derive(Debug, Default)]
struct EventBuffer {
    /// The events pushed since the last drain, in push order.
    events: Vec<Event>,
    /// Each task's next sequence number.
    seqs: HashMap<TaskId, u32>,
}

impl EventBuffer {
    /// Allocates `task`'s next provenance: its current counter, advanced
    /// by one. Saturating at `u32::MAX`, which no reachable run approaches.
    fn next_provenance(&mut self, task: &TaskId) -> Provenance {
        let seq = self.seqs.entry(task.clone()).or_insert(0);
        let provenance = Provenance {
            task: task.clone(),
            seq: *seq,
        };
        *seq = seq.saturating_add(1);
        provenance
    }
}

/// The shared handle onto one run's event buffer: every chain's emitter
/// pushes through a clone, and the run drains through its own.
#[derive(Clone, Debug, Default)]
pub struct EventSink(Arc<Mutex<EventBuffer>>);

impl EventSink {
    /// A buffer whose root task (`0`) counts from `start` instead of zero.
    ///
    /// A prompt's parse reports under task `0` through its own sink before
    /// any run exists, so a caller that keeps the parse events and the run's
    /// events in one sequence seeds the run's buffer with the parse's event count:
    /// the run's first root-task stamp continues the parse's sequence, and
    /// `(task, seq)` stays unique across the two. Every other task still
    /// counts from zero.
    #[must_use]
    pub fn seeded(start: u32) -> Self {
        let mut buffer = EventBuffer::default();
        buffer.seqs.insert(TaskId::from(ChainId::root()), start);
        Self(Arc::new(Mutex::new(buffer)))
    }

    /// Pushes one event built from `task`'s next provenance. The lock is
    /// held only for the allocation and the push; `build` runs under it,
    /// so it must not touch the sink.
    fn push(&self, task: &TaskId, build: impl FnOnce(Provenance) -> Event) {
        // A poisoned lock means an emitter panicked mid-push; the buffer's
        // contents are still consistent, so recover it rather than turn a
        // report into a second panic.
        let mut buffer = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let provenance = buffer.next_provenance(task);
        buffer.events.push(build(provenance));
    }

    /// Allocates `task`'s next provenance without pushing an event: the
    /// stamp on an issued effect, drawn from the same counter as the
    /// task's events so effects and events from one task share one dense
    /// sequence.
    fn allocate(&self, task: &TaskId) -> Provenance {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_provenance(task)
    }

    /// Takes every event pushed since the last drain, in push order.
    #[must_use]
    pub fn take(&self) -> Vec<Event> {
        let mut buffer = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut buffer.events)
    }
}

/// One task's handle onto the run's event buffer: every event it pushes is
/// stamped with the task's next sequence number under the run's execution
/// id. Cheap to clone; a clone shares the task and the buffer.
///
/// Every report is write-only: the Engine never reads an event back
/// through this path, so recording every event or dropping them all
/// leaves a run's outputs, errors, and ordering unchanged.
#[derive(Clone, Debug)]
pub struct Emitter {
    /// The run's buffer.
    sink: EventSink,
    /// The nearest enclosing task of the chain this emitter serves.
    task: TaskId,
    /// The caller-chosen run identifier stamped on every event.
    execution: Arc<str>,
    /// Whether the caller asked for raw request/response capture: the model
    /// rounds emit `Request` and `Response` only when [`DebugMode::On`],
    /// so a run that did not opt in never clones a body.
    debug: DebugMode,
}

impl Emitter {
    /// Builds the emitter for `task` over `sink`.
    #[must_use]
    pub fn new(sink: EventSink, task: TaskId, execution: Arc<str>, debug: DebugMode) -> Self {
        Self {
            sink,
            task,
            execution,
            debug,
        }
    }

    /// The root task's emitter over `sink`: the main walk is task `0`,
    /// and so is a prompt's parse, which happens before any run exists.
    #[must_use]
    pub fn root(sink: EventSink, execution: &str, debug: DebugMode) -> Self {
        Self::new(
            sink,
            TaskId::from(ChainId::root()),
            Arc::from(execution),
            debug,
        )
    }

    /// The emitter a spawned chain reports through: the same buffer under
    /// the chain's own task.
    #[must_use]
    pub fn for_task(&self, task: TaskId) -> Self {
        Self {
            sink: self.sink.clone(),
            task,
            execution: Arc::clone(&self.execution),
            debug: self.debug,
        }
    }

    /// The task this emitter stamps its events with.
    #[must_use]
    pub fn task(&self) -> &TaskId {
        &self.task
    }

    /// The caller-chosen run identifier stamped on every event.
    #[must_use]
    pub fn execution(&self) -> &str {
        &self.execution
    }

    /// Whether the run captures raw model-turn bodies.
    #[must_use]
    pub fn captures_debug(&self) -> bool {
        self.debug == DebugMode::On
    }

    /// Stamps one issued effect: this task's next provenance, drawn from
    /// the counter its events advance, so the effect orders among them.
    #[must_use]
    pub fn stamp_effect(&self) -> Provenance {
        self.sink.allocate(&self.task)
    }

    /// Pushes one event built from this task's next coordinates: the
    /// general form every named report below is a case of, for the
    /// variants with a payload and no dedicated method.
    pub fn emit(&self, section: &str, build: impl FnOnce(String, String, Provenance) -> Event) {
        let execution = self.execution.to_string();
        let section = section.to_owned();
        self.sink.push(&self.task, |provenance| {
            build(execution, section, provenance)
        });
    }

    /// Reports one payload-free lifecycle boundary under `section`.
    pub fn report(&self, section: &str, boundary: Lifecycle) {
        self.emit(section, boundary);
    }

    /// Reports the author's `log(message)` checkpoint.
    pub fn lua(&self, section: &str, message: &str) {
        self.emit(section, |execution, section, provenance| Event::Lua {
            execution,
            section,
            provenance,
            message: message.to_owned(),
        });
    }

    /// Reports one completed block of model thinking from `round`.
    pub fn thinking(&self, section: &str, turn: u32, round: RoundId, model: &str, text: &str) {
        self.emit(section, |execution, section, provenance| Event::Thinking {
            execution,
            section,
            provenance,
            turn,
            round,
            model: model.to_owned(),
            text: text.to_owned(),
        });
    }

    /// Reports one completed assistant reply: `round`'s text reply with
    /// its [`ReplyOrigin`] provenance.
    #[expect(
        clippy::too_many_arguments,
        reason = "the reply report names its full run coordinates, including the round and origin, in one call"
    )]
    pub fn assistant_reply(
        &self,
        section: &str,
        turn: u32,
        round: RoundId,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        metrics: Option<&CallMetrics>,
        origin: ReplyOrigin,
    ) {
        self.emit(section, |execution, section, provenance| {
            Event::AssistantReply {
                execution,
                section,
                provenance,
                turn,
                round,
                text: text.to_owned(),
                finish_reason: finish_reason.map(str::to_owned),
                model: model.to_owned(),
                metrics: metrics.cloned(),
                origin,
            }
        });
    }

    /// Reports one batch of tool calls the model requested in `round`,
    /// unexecuted.
    pub fn assistant_tool_calls(
        &self,
        section: &str,
        turn: u32,
        round: RoundId,
        model: &str,
        calls: &[ToolCallEvent],
    ) {
        self.emit(section, |execution, section, provenance| {
            Event::AssistantToolCalls {
                execution,
                section,
                provenance,
                turn,
                round,
                model: model.to_owned(),
                calls: calls.to_vec(),
            }
        });
    }

    /// Reports the result of one dispatched tool call. `tool` is the bound
    /// tool `alias` resolved to, or `None` for a Lua-local tool or a task
    /// built-in. The event records `trust` as its `trusted` flag: `true`
    /// only for [`OutputTrust::Trusted`].
    #[expect(
        clippy::too_many_arguments,
        reason = "the result report names its full run coordinates, including the alias and the tool it resolved to, in one call"
    )]
    pub fn tool_result(
        &self,
        section: &str,
        turn: u32,
        tool_call_id: &str,
        alias: &str,
        tool: Option<&ToolId>,
        content: &str,
        trust: OutputTrust,
    ) {
        let trusted = trust == OutputTrust::Trusted;
        self.emit(section, |execution, section, provenance| {
            Event::ToolResult {
                execution,
                section,
                provenance,
                turn,
                tool_call_id: tool_call_id.to_owned(),
                alias: alias.to_owned(),
                tool: tool.cloned(),
                content: content.to_owned(),
                trusted,
            }
        });
    }

    /// Reports one model-task notice as it is queued for the task's owner.
    pub fn task_notice(&self, section: &str, turn: u32, task: &TaskId, text: &str) {
        self.emit(section, |execution, section, provenance| {
            Event::TaskNotice {
                execution,
                section,
                provenance,
                turn,
                task: task.clone(),
                text: text.to_owned(),
            }
        });
    }

    /// Captures the request body of one completed model turn. The caller
    /// gates on [`captures_debug`](Self::captures_debug) so a run that did
    /// not opt in never clones a body.
    pub fn request(&self, section: &str, turn: u32, body: Value) {
        self.emit(section, |execution, section, provenance| Event::Request {
            execution,
            section,
            provenance,
            turn,
            body,
        });
    }

    /// Captures the response body of one completed model turn, with its
    /// parsed metadata. Gated as [`request`](Self::request) is.
    pub fn response(
        &self,
        section: &str,
        turn: u32,
        body: Value,
        finish_reason: Option<String>,
        reasoning_content: Option<String>,
    ) {
        self.emit(section, |execution, section, provenance| Event::Response {
            execution,
            section,
            provenance,
            turn,
            body,
            finish_reason,
            reasoning_content,
        });
    }
}
