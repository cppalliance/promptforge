//! The suites' recording observer: the callback shape the engine's tests
//! were written against, fed from the [`Event`](promptforge_api_types::event::Event)
//! values a run returns.
//!
//! The engine reports as values and never through a callback. The suites,
//! though, assert on sequences of `(execution, section, observation)`
//! records and on the `on_*` content hooks, so this module keeps that
//! vocabulary as a test fixture: [`Observation`] is the payload-free view
//! of a lifecycle event, [`Observer`] the recording trait a suite
//! implements, [`RecordingObserver`] the one most suites install, and
//! [`DebugCapture`] the raw-body sink the debug suites use. [`forward`]
//! replays a returned batch onto them, in order, so the suites hold
//! without rewriting their assertions. None of this is engine API: a
//! production host reads the events themselves.
//!
//! # Sensitivity
//! The `execution` and `section` coordinates are author-controlled, and
//! every `on_*` payload is model-, tool-, or user-authored; a recorder
//! that persists them owns treating them as untrusted, exactly as a host
//! does with the events they came from.

use std::sync::{Mutex, PoisonError};

use promptforge_api_types::event::ReplyOrigin;
use promptforge_api_types::ids::TaskId;
use promptforge_api_types::metrics::{CallMetrics, ToolCallEvent};
use serde_json::Value;

#[path = "recording-forward.rs"]
mod forward;
#[path = "recording-observation.rs"]
mod observation;

pub use forward::{forward, forward_one};
pub use observation::{Observation, detail};

/// The recording seam a suite implements: one method per report the
/// engine used to make through a callback, each with a default body that
/// discards it, so a recorder pays only for the hooks it overrides.
#[expect(
    clippy::too_many_arguments,
    reason = "each content report names its full run coordinates in one call, as the suites' recorders expect"
)]
pub trait Observer: Send + Sync {
    /// Records one typed [`Observation`] for `execution` and `section`.
    fn observe(&self, execution: &str, section: &str, event: Observation);

    /// Records one completed assistant reply. `origin` is the reply's
    /// provenance: [`ReplyOrigin::Chat`] for a user-facing turn,
    /// [`ReplyOrigin::Infer`] for a programmatic inference round.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_assistant_reply(
        &self,
        execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        metrics: Option<&CallMetrics>,
        origin: ReplyOrigin,
    ) {
    }

    /// Records one batch of tool calls the model requested, unexecuted.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_assistant_tool_calls(
        &self,
        execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        model: &str,
        calls: &[ToolCallEvent],
    ) {
    }

    /// Records the result of one dispatched tool call.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_tool_result(
        &self,
        execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        tool_call_id: &str,
        alias: &str,
        content: &str,
        trusted: bool,
    ) {
    }

    /// Records one completed block of model thinking.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_thinking(
        &self,
        execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        model: &str,
        text: &str,
    ) {
    }

    /// Records text the user supplied, byte-exact.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_user_input(&self, execution: &str, section: &str, text: &str) {}

    /// Records one model-task notice as it was queued for the task's owner.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_task_notice(
        &self,
        execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        task: &TaskId,
        text: &str,
    ) {
    }
}

/// An emitter over a sink nobody drains: the silent stand-in a suite
/// passes a VM seam when it has nothing to assert about the boundaries.
#[must_use]
pub fn null_emitter() -> promptforge_api_types::emitter::Emitter {
    promptforge_api_types::emitter::Emitter::root(
        promptforge_api_types::emitter::EventSink::default(),
        "test",
        promptforge_api_types::emitter::DebugMode::Off,
    )
}

/// An [`Observer`] that discards every report: what a suite installs when
/// it has nothing to assert about the boundaries. Construct it through
/// `Default`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NullObserver;

impl Observer for NullObserver {
    fn observe(&self, _execution: &str, _section: &str, _event: Observation) {}
}

/// The recorder most suites install: every observation it is handed, in
/// order, as a correlated `(execution, section, trace line)` record, so a
/// test asserts on the whole sequence rather than on a count.
#[derive(Debug, Default)]
pub struct RecordingObserver(Mutex<Vec<(String, String, String)>>);

impl RecordingObserver {
    /// The full correlated records recorded so far, in order. A recorder
    /// poisoned by a panicking test still yields what it saw.
    #[must_use]
    pub fn records(&self) -> Vec<(String, String, String)> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The `(section, trace line)` pairs recorded so far, in order.
    #[must_use]
    pub fn events(&self) -> Vec<(String, String)> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(_, section, detail)| (section.clone(), detail.clone()))
            .collect()
    }
}

impl Observer for RecordingObserver {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push((
            execution.to_owned(),
            section.to_owned(),
            event.to_string(),
        ));
    }
}

/// The raw model-turn capture a debug suite installs: the two bodies of
/// each completed turn, request before response, in turn order.
pub trait DebugCapture: Send + Sync {
    /// Receives one capture event for a model turn. `turn_index` is the
    /// 1-based model-turn number within the run.
    fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent);
}

/// One owned capture payload for a model turn: the verbatim wire body.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DebugEvent {
    /// The JSON body sent to the chat-completions endpoint.
    #[non_exhaustive]
    Request {
        /// The serialized request body.
        body: Value,
    },
    /// The JSON body returned, with parsed metadata.
    #[non_exhaustive]
    Response {
        /// The raw response body.
        body: Value,
        /// The choice's `finish_reason`, when the backend supplied one.
        finish_reason: Option<String>,
        /// The message's `reasoning_content`, when the backend supplied one.
        reasoning_content: Option<String>,
    },
}

impl DebugEvent {
    /// Builds a [`DebugEvent::Request`] from a serialized request `body`.
    #[must_use]
    pub fn request(body: Value) -> DebugEvent {
        DebugEvent::Request { body }
    }

    /// Builds a [`DebugEvent::Response`] from a response `body` and its
    /// parsed metadata.
    #[must_use]
    pub fn response(
        body: Value,
        finish_reason: Option<String>,
        reasoning_content: Option<String>,
    ) -> DebugEvent {
        DebugEvent::Response {
            body,
            finish_reason,
            reasoning_content,
        }
    }
}
