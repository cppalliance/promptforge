//! The suites' recording observer vocabulary.
//!
//! [`Observer`] is the callback shape the Engine's tests were written
//! against, and [`Observation`] is the payload-free view of the
//! [`Event`](promptforge_types::event::Event) values a run returns.
//!
//! The Engine reports as values and never through a callback. The suites,
//! though, assert on sequences of `(execution, section, observation)`
//! records and on the `on_*` content hooks, so this module keeps that
//! vocabulary as a test fixture. [`forward()`] replays a returned batch onto
//! an observer, in order, so the suites hold without rewriting their
//! assertions. The remaining seams - the raw-body capture, the null
//! observer, the detail constants - are crate-internal plumbing.
//!
//! # Sensitivity
//! The `execution` and `section` coordinates are author-controlled, and
//! every `on_*` payload is model-, tool-, or user-authored; a recorder
//! that persists them owns treating them as untrusted.

use promptforge_types::event::ReplyOrigin;
use promptforge_types::ids::TaskId;
use promptforge_types::metrics::{CallMetrics, ToolCallEvent};
use promptforge_types::tools::OutputTrust;
use serde_json::Value;

mod forward;
mod observation;

pub use forward::forward;
pub(crate) use forward::forward_one;
pub use observation::Observation;
#[cfg(test)]
pub(crate) use observation::detail;

/// The recording seam a suite implements, one method per run report.
///
/// Each method has a default body that discards the report, so a recorder
/// pays only for the hooks it overrides.
pub trait Observer: Send + Sync {
    /// Records one typed [`Observation`] for `execution` and `section`.
    fn observe(&self, execution: &str, section: &str, event: Observation);

    /// Records one completed assistant reply.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_assistant_reply(&self, report: ReplyReport<'_>) {}

    /// Records one batch of tool calls the model requested, unexecuted.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_assistant_tool_calls(&self, report: ToolCallsReport<'_>) {}

    /// Records the result of one dispatched tool call.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_tool_result(&self, report: ToolResultReport<'_>) {}

    /// Records one completed block of model thinking.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_thinking(&self, report: ThinkingReport<'_>) {}

    /// Records one model-task notice as it was queued for the task's owner.
    #[expect(unused_variables, reason = "the default body discards the report")]
    fn on_task_notice(&self, report: TaskNoticeReport<'_>) {}
}

/// One completed assistant reply, as [`Observer::on_assistant_reply`]
/// receives it.
#[derive(Debug, Clone, Copy)]
pub struct ReplyReport<'a> {
    /// The caller-chosen run identifier.
    pub execution: &'a str,
    /// The event's reporting scope.
    pub section: &'a str,
    /// The model-turn counter the reply was produced under.
    pub turn: u32,
    /// The reply text.
    pub text: &'a str,
    /// The provider's stop label, when it sent one.
    pub finish_reason: Option<&'a str>,
    /// The model that produced the reply.
    pub model: &'a str,
    /// The call's measurements, when any were reported.
    pub metrics: Option<&'a CallMetrics>,
    /// The kind of round that produced the reply.
    pub origin: ReplyOrigin,
}

/// One batch of tool calls the model requested, as
/// [`Observer::on_assistant_tool_calls`] receives it.
#[derive(Debug, Clone, Copy)]
pub struct ToolCallsReport<'a> {
    /// The caller-chosen run identifier.
    pub execution: &'a str,
    /// The event's reporting scope.
    pub section: &'a str,
    /// The model-turn counter the batch was requested under.
    pub turn: u32,
    /// The model that requested the calls.
    pub model: &'a str,
    /// The calls, unexecuted.
    pub calls: &'a [ToolCallEvent],
}

/// One dispatched tool call's result, as [`Observer::on_tool_result`]
/// receives it.
#[derive(Debug, Clone, Copy)]
pub struct ToolResultReport<'a> {
    /// The caller-chosen run identifier.
    pub execution: &'a str,
    /// The event's reporting scope.
    pub section: &'a str,
    /// The model-turn counter the call was dispatched under.
    pub turn: u32,
    /// The provider-issued tool-call id the result answers.
    pub tool_call_id: &'a str,
    /// The alias the call named.
    pub alias: &'a str,
    /// The tool's output.
    pub content: &'a str,
    /// Whether the dispatch treated the tool's output as trusted.
    pub trust: OutputTrust,
}

/// One completed block of model thinking, as [`Observer::on_thinking`]
/// receives it.
#[derive(Debug, Clone, Copy)]
pub struct ThinkingReport<'a> {
    /// The caller-chosen run identifier.
    pub execution: &'a str,
    /// The event's reporting scope.
    pub section: &'a str,
    /// The model-turn counter the block was produced under.
    pub turn: u32,
    /// The model that produced it.
    pub model: &'a str,
    /// The thinking text.
    pub text: &'a str,
}

/// One model-task notice queued for the task's owner, as
/// [`Observer::on_task_notice`] receives it.
#[derive(Debug, Clone, Copy)]
pub struct TaskNoticeReport<'a> {
    /// The caller-chosen run identifier.
    pub execution: &'a str,
    /// The owner's reporting scope.
    pub section: &'a str,
    /// The owner's model-turn counter when the notice was queued.
    pub turn: u32,
    /// The task that ended.
    pub task: &'a TaskId,
    /// The sentence the model reads.
    pub text: &'a str,
}

/// An emitter over a sink nobody drains: the silent stand-in a suite
/// passes a VM seam when it has nothing to assert about the boundaries.
#[cfg(test)]
#[must_use]
pub(crate) fn null_emitter() -> promptforge_types::emitter::Emitter {
    promptforge_types::emitter::Emitter::root(
        promptforge_types::emitter::EventSink::default(),
        "test",
        promptforge_types::emitter::DebugMode::Off,
    )
}

/// An [`Observer`] that discards every report: what a suite installs when
/// it has nothing to assert about the boundaries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NullObserver;

impl Observer for NullObserver {
    fn observe(&self, _execution: &str, _section: &str, _event: Observation) {}
}

/// The raw model-turn capture a debug suite installs: the two bodies of
/// each completed turn, request before response, in turn order.
pub(crate) trait DebugCapture: Send + Sync {
    /// Receives one capture event for a model turn. `turn_index` is the
    /// 1-based model-turn number within the run.
    fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent);
}

/// One owned capture payload for a model turn: the verbatim wire body.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the captured fields are read only by the in-crate debug suites, which the non-test `test-support` build does not compile"
    )
)]
#[derive(Debug, Clone)]
pub(crate) enum DebugEvent {
    /// The JSON body sent to the chat-completions endpoint.
    Request {
        /// The serialized request body.
        body: Value,
    },
    /// The JSON body returned, with parsed metadata.
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
    fn request(body: Value) -> DebugEvent {
        DebugEvent::Request { body }
    }

    /// Builds a [`DebugEvent::Response`] from a response `body` and its
    /// parsed metadata.
    #[must_use]
    fn response(
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
