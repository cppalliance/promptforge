//! The run-level event buffer and the task-scoped emitter over it.
//!
//! The engine reports as values: every boundary, content report, and
//! debug capture becomes one [`Event`] pushed into the run's
//! [`EventBuffer`], stamped with a [`Provenance`] - the nearest enclosing
//! task and that task's next sequence number - and drained by the driver
//! after every dispatch round. Nothing in here reaches a host; the
//! `events_to_observer` module forwards a drained batch to the legacy
//! [`Observer`] and `DebugCapture` seams.
//!
//! An [`Emitter`] is one chain's handle onto the buffer: it knows its task
//! (the main walk is task `0`; a `call` child shares its caller's emitter,
//! so it reports under the caller's task; a spawned chain gets its own
//! through [`Emitter::for_task`]) and stamps every event it pushes with
//! that task's next `seq`. The counters live in the buffer, under its one
//! lock, so a task's sequence is dense from zero however its chains and
//! the run's leaf tasks interleave.
//!
//! The emitter also implements [`Observer`], because the section VM and
//! the shared tool-dispatch body in `promptforge-lua` still take
//! `&dyn Observer` for the shared-library replay, the `log` checkpoint,
//! teardown, and the `ToolResult` report. Through the impl those reports
//! land in the same buffer as the scheduler's own, in order, until the
//! trait leaves the engine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use promptforge_api_types::event::Event;
use promptforge_api_types::events::{CallMetrics, ToolCallEvent};
use promptforge_api_types::ids::{Provenance, TaskId};
use serde_json::Value;

use crate::observe::{Observation, Observer};

#[path = "event_buffer-lifecycle.rs"]
mod lifecycle;

use lifecycle::lifecycle_event;
pub(crate) use lifecycle::{unit_lifecycle_variants, unit_observation};

/// The run's event buffer: the events not yet drained, plus one sequence
/// counter per task the run has reported under.
#[derive(Debug, Default)]
pub(crate) struct EventBuffer {
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

/// The shared handle onto one run's [`EventBuffer`]: every chain's emitter
/// and every spawned leaf task pushes through a clone, and the driver
/// drains through its own.
#[derive(Clone, Debug, Default)]
pub(crate) struct EventSink(Arc<Mutex<EventBuffer>>);

impl EventSink {
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
    /// stamp an issued effect carries, drawn from the same counter as the
    /// task's events so effects and events from one task share one dense
    /// sequence.
    fn allocate(&self, task: &TaskId) -> Provenance {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_provenance(task)
    }

    /// Takes every event pushed since the last drain, in push order.
    pub(crate) fn take(&self) -> Vec<Event> {
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
#[derive(Clone, Debug)]
pub(crate) struct Emitter {
    /// The run's buffer.
    sink: EventSink,
    /// The nearest enclosing task of the chain this emitter serves.
    task: TaskId,
    /// The caller-chosen run identifier every event carries.
    execution: Arc<str>,
    /// Whether the host asked for raw request/response capture: the model
    /// rounds emit `Request` and `Response` only when set, so a run that
    /// did not opt in never clones a body.
    debug: bool,
}

impl Emitter {
    /// Builds the emitter for `task` over `sink`.
    pub(crate) fn new(sink: EventSink, task: TaskId, execution: Arc<str>, debug: bool) -> Self {
        Self {
            sink,
            task,
            execution,
            debug,
        }
    }

    /// The emitter a spawned chain reports through: the same buffer under
    /// the chain's own task.
    #[must_use]
    pub(crate) fn for_task(&self, task: TaskId) -> Self {
        Self {
            sink: self.sink.clone(),
            task,
            execution: Arc::clone(&self.execution),
            debug: self.debug,
        }
    }

    /// The task this emitter stamps its events with.
    #[cfg(test)]
    pub(crate) fn task(&self) -> &TaskId {
        &self.task
    }

    /// Whether the run captures raw model-turn bodies.
    pub(crate) fn captures_debug(&self) -> bool {
        self.debug
    }

    /// Stamps one issued effect: this task's next provenance, drawn from
    /// the counter its events advance, so the effect orders among them.
    pub(crate) fn stamp_effect(&self) -> Provenance {
        self.sink.allocate(&self.task)
    }

    /// Pushes one event built from this task's next coordinates.
    fn push(&self, section: &str, build: impl FnOnce(String, String, Provenance) -> Event) {
        let execution = self.execution.to_string();
        let section = section.to_owned();
        self.sink.push(&self.task, |provenance| {
            build(execution, section, provenance)
        });
    }

    /// Reports one lifecycle boundary under `section`: the value form of
    /// today's [`Observation`] vocabulary, mapped variant for variant.
    pub(crate) fn report(&self, section: &str, observation: Observation) {
        self.push(section, |execution, section, provenance| {
            lifecycle_event(observation, execution, section, provenance)
        });
    }

    /// Reports one completed block of model thinking.
    pub(crate) fn thinking(&self, section: &str, turn: u32, model: &str, text: &str) {
        self.push(section, |execution, section, provenance| Event::Thinking {
            execution,
            section,
            provenance,
            turn,
            model: model.to_owned(),
            text: text.to_owned(),
        });
    }

    /// Reports one completed assistant reply.
    pub(crate) fn assistant_reply(
        &self,
        section: &str,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        metrics: Option<&CallMetrics>,
    ) {
        self.push(section, |execution, section, provenance| {
            Event::AssistantReply {
                execution,
                section,
                provenance,
                turn,
                text: text.to_owned(),
                finish_reason: finish_reason.map(str::to_owned),
                model: model.to_owned(),
                metrics: metrics.cloned(),
            }
        });
    }

    /// Reports one batch of tool calls the model requested, unexecuted.
    pub(crate) fn assistant_tool_calls(
        &self,
        section: &str,
        turn: u32,
        model: &str,
        calls: &[ToolCallEvent],
    ) {
        self.push(section, |execution, section, provenance| {
            Event::AssistantToolCalls {
                execution,
                section,
                provenance,
                turn,
                model: model.to_owned(),
                calls: calls.to_vec(),
            }
        });
    }

    /// Reports the result of one dispatched tool call.
    pub(crate) fn tool_result(
        &self,
        section: &str,
        turn: u32,
        tool_call_id: &str,
        alias: &str,
        content: &str,
        trusted: bool,
    ) {
        self.push(section, |execution, section, provenance| {
            Event::ToolResult {
                execution,
                section,
                provenance,
                turn,
                tool_call_id: tool_call_id.to_owned(),
                alias: alias.to_owned(),
                content: content.to_owned(),
                trusted,
            }
        });
    }

    /// Reports text the user supplied, byte-exact.
    pub(crate) fn user_input(&self, section: &str, text: &str) {
        self.push(section, |execution, section, provenance| Event::UserInput {
            execution,
            section,
            provenance,
            text: text.to_owned(),
        });
    }

    /// Reports one model-task notice as it is queued for the task's owner.
    pub(crate) fn task_notice(&self, section: &str, turn: u32, task: &TaskId, text: &str) {
        self.push(section, |execution, section, provenance| {
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
    pub(crate) fn request(&self, section: &str, turn: u32, body: Value) {
        self.push(section, |execution, section, provenance| Event::Request {
            execution,
            section,
            provenance,
            turn,
            body,
        });
    }

    /// Captures the response body of one completed model turn, with its
    /// parsed metadata. Gated as [`request`](Self::request) is.
    pub(crate) fn response(
        &self,
        section: &str,
        turn: u32,
        body: Value,
        finish_reason: Option<String>,
        reasoning_content: Option<String>,
    ) {
        self.push(section, |execution, section, provenance| Event::Response {
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

/// The seam for the Lua layer's `&dyn Observer` parameters: every report
/// lands in the buffer under this emitter's task. The `execution` each
/// call carries is the run's own, so the emitter's copy stands in for it;
/// `chain_id` and `depth` have no place in an [`Event`], whose provenance
/// names the task instead.
impl Observer for Emitter {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.report(section, event);
    }

    fn on_assistant_reply(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        metrics: Option<&CallMetrics>,
    ) {
        self.assistant_reply(section, turn, text, finish_reason, model, metrics);
    }

    fn on_assistant_tool_calls(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        model: &str,
        calls: &[ToolCallEvent],
    ) {
        self.assistant_tool_calls(section, turn, model, calls);
    }

    fn on_tool_result(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        tool_call_id: &str,
        alias: &str,
        content: &str,
        trusted: bool,
    ) {
        self.tool_result(section, turn, tool_call_id, alias, content, trusted);
    }

    fn on_thinking(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        model: &str,
        text: &str,
    ) {
        self.thinking(section, turn, model, text);
    }

    fn on_user_input(&self, _execution: &str, section: &str, text: &str) {
        self.user_input(section, text);
    }

    fn on_task_notice(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        task: &TaskId,
        text: &str,
    ) {
        self.task_notice(section, turn, task, text);
    }
}

#[cfg(test)]
#[path = "event_buffer-tests.rs"]
mod tests;
