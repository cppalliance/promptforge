//! The pending table's entry: what a parked chain asked for, in the terms
//! `apply_answer` needs to turn the caller's raw [`EffectAnswer`] into the
//! chain's protocol [`Answer`] and emit the round's events. The effect
//! itself holds none of this: it describes the work, the continuation
//! describes what the work means to the chain.
//!
//! [`EffectAnswer`]: super::EffectAnswer
//! [`Answer`]: super::Answer

use crate::execute::protocol::VfsOp;
use crate::lua::{ScriptReport, ToolBinding};
use promptforge_types::event::lifecycle::Lifecycle;
use promptforge_types::ids::RoundId;
use promptforge_vfs::ExecId;

use super::ChainIndex;

/// The driver-side half of one issued leaf effect: how its answer resumes
/// the chain parked on it.
pub(super) enum Continuation {
    /// A nested `models.infer`: the completion becomes the round's text
    /// under the single-prose-round reporting rules. The round's id waits
    /// here for the answer, whose events it stamps.
    Infer(RoundId),
    /// A `chat` round: the completion is classified against the scope the
    /// chain advertised and reported as one model turn. The round's id
    /// waits here for the answer, whose events it stamps.
    Chat(RoundId),
    /// A bound tool call: the tool's own output goes through the shared
    /// dispatch body (counts already taken at dispatch, then the
    /// succeeded/failed event, the trust rule, and the `ToolResult`).
    ToolCall(ToolCallContinuation),
    /// A [`Vfs`](super::Effect::Vfs) effect: the operation itself (for the
    /// answer's rendered message and conflict classification) and the
    /// succeeded/failed observation pair its outcome reports; `None` for
    /// an op this crate does not name.
    Vfs(VfsContinuation),
    /// The internal timer behind a timed wait: the firing completes the
    /// slot backed by the effect and wakes its waiting owner; no chain
    /// resumes.
    Timer,
}

/// What a store answer is applied with: the operation the chain yielded,
/// so its failure's model-facing message and determinism classification
/// render from the call surface the author used.
pub(super) struct VfsContinuation {
    /// The operation the effect performs.
    pub(super) op: VfsOp,
    /// The succeeded/failed observation pair the outcome reports; `None`
    /// for an op this crate does not name.
    pub(super) observations: Option<(Lifecycle, Lifecycle)>,
}

/// What a bound `tool_call`'s answer is applied with: the binding the call
/// resolved to (its alias, output kind, and trust rules), the coordinates
/// the `ToolResult` reports under, the call's site, and the call's
/// identity the answer joins.
pub(super) struct ToolCallContinuation {
    /// The binding the alias resolved to at dispatch.
    pub(super) binding: ToolBinding,
    /// The turn the call fired in.
    pub(super) report: ScriptReport,
    /// Who asked for the call, and where its answer goes.
    pub(super) site: CallSite,
    /// The call's own identity, forked from the chain's access at
    /// dispatch. The answer, or the call's abort, ends it and joins it
    /// back into the chain.
    pub(super) exec: ExecId,
}

/// Who asked for one tool call, and where its answer goes. Only a
/// model-issued call carries a call id, and only a batched one a slot.
pub(super) enum CallSite {
    /// A script's `tools.call`, under the turn its request named, if any.
    /// The answer resumes the chain directly, classified by the binding's
    /// declared output kind, and reports under no call id.
    Script { turn: Option<u32> },
    /// A model-issued call sent alone, under the turn of the round that
    /// requested it, if the request named one. The answer resumes the
    /// chain directly as content under the model's call id.
    Model { call_id: String, turn: Option<u32> },
    /// A model-issued call sent in a `tool_calls` batch, under the round's
    /// turn. The answer fills `slot` of the batch the chain is parked on.
    Batch {
        call_id: String,
        turn: u32,
        slot: usize,
    },
}

impl CallSite {
    /// The model's call id, or `None` for a script call.
    pub(super) fn call_id(&self) -> Option<&str> {
        match self {
            Self::Script { .. } => None,
            Self::Model { call_id, .. } | Self::Batch { call_id, .. } => Some(call_id.as_str()),
        }
    }

    /// The turn the request named, if any.
    pub(super) fn turn(&self) -> Option<u32> {
        match self {
            Self::Script { turn } | Self::Model { turn, .. } => *turn,
            Self::Batch { turn, .. } => Some(*turn),
        }
    }
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
