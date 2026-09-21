//! The pending table's entry: what a parked chain asked for, in the terms
//! `apply_answer` needs to turn the host's raw [`EffectAnswer`] into the
//! chain's protocol [`Answer`] and emit the round's events. The effect
//! itself holds none of this: it describes the work, the continuation
//! describes what the work means to the chain.
//!
//! [`EffectAnswer`]: super::EffectAnswer
//! [`Answer`]: super::Answer

use crate::lua::{ScriptReport, ToolBinding};
use promptforge_api_types::event::lifecycle::Lifecycle;

use super::ChainIndex;
use super::task_events::TaskEventsReader;

/// The driver-side half of one issued leaf effect: how its answer resumes
/// the chain parked on it.
pub(super) enum Continuation {
    /// A nested `models.infer`: the completion becomes the round's text
    /// under the single-prose-round reporting rules.
    Infer,
    /// A `chat` round: the completion is classified against the scope the
    /// chain advertised and reported as one model turn.
    Chat,
    /// A bound tool call: the tool's own output goes through the shared
    /// dispatch body (counts already taken at dispatch, then the
    /// succeeded/failed event, the trust rule, and the `ToolResult`).
    ToolCall(ToolCallContinuation),
    /// A `user_input` wait: the broker's text is reported and resumes with
    /// its availability flag.
    UserInput,
    /// A store operation: the succeeded/failed observation pair its
    /// outcome reports, `None` for `exists`, which reports nothing.
    Store(Option<(Lifecycle, Lifecycle)>),
    /// The internal timer behind a timed wait: the firing completes the
    /// slot backed by the effect and wakes its waiting owner; no chain
    /// resumes.
    Timer,
    /// A task history read: the shim's event sequence, or the model's
    /// untrusted text.
    TaskEvents(TaskEventsReader),
}

/// What a bound `tool_call`'s answer is applied with: the binding the call
/// resolved to (its alias, output kind, and trust rules), the coordinates
/// the `ToolResult` reports under, and the model's call id when the model
/// issued the call.
pub(super) struct ToolCallContinuation {
    /// The binding the alias resolved to at dispatch.
    pub(super) binding: ToolBinding,
    /// The turn the call fired in.
    pub(super) report: ScriptReport,
    /// The model-issued call id, or `None` for a script call.
    pub(super) call_id: Option<String>,
}

/// One in-flight leaf effect's pending entry: the chain parked on it and
/// how its answer resumes that chain.
pub(super) struct Pending {
    /// The parked chain (for a timer, the owner whose wait the timer
    /// serves).
    pub(super) chain: ChainIndex,
    /// How the answer is applied.
    pub(super) resume: Continuation,
}
