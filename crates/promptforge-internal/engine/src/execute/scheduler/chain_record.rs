//! The chain record: one chain's identity, walk position, in-flight
//! coroutine, task bookkeeping, and walk-scoped slots. The scheduler's
//! arena holds one per chain it has started, indexed by [`ChainIndex`].

use std::collections::BTreeMap;
use std::sync::Arc;

use mlua::Thread;
use promptforge_types::ids::{ChainId, TaskId};
use promptforge_types::metrics::Usage;
use promptforge_vfs::Access;

use crate::execute::context::RunState;
use crate::execute::protocol::Answer;
use crate::execute::scope::DispatchTarget;
use crate::execute::section_context::{SectionContext, TaskSeed};
use crate::lua::UsageAnchor;
use crate::model::{Message, ModelId};
use crate::parser::{Block, Prompt, Section};
use crate::{Error, Result};

use super::await_tasks::AwaitTasks;
use super::{ChainIndex, Counters, SlicePath, SpawnRecord};

/// What the chain's `chat` rounds taught the context precheck about token
/// counts: the provider's numbers for the newest round that reported usage.
///
/// A chain has one parked `chat` round at a time. Dispatch holds that
/// round's projected messages in `in_flight`, beside the id of the model
/// they went to; the arrival of its answer settles them into `measured`
/// when the round reported usage, and drops them otherwise. A round with no
/// usage leaves the older measurement in place: it still describes a prefix
/// of the conversation, and the precheck uses it only while the request
/// extends that prefix on the same model.
///
/// The model is compared by [`ModelId`], the gateway server and model name
/// the binding resolved to, because that is what picks the tokenizer. The
/// alias is only a prompt-local label: two aliases of one model, with
/// other temperature or thinking settings, count tokens alike and share a
/// measurement.
#[derive(Default)]
pub(super) struct ChatAnchor {
    in_flight: Option<(ModelId, Vec<Message>)>,
    measured: Option<(ModelId, UsageAnchor)>,
}

impl ChatAnchor {
    /// The newest measurement, for the precheck of the next dispatch to
    /// `model`. A measurement from another model is not offered: its token
    /// counts come from another tokenizer.
    pub(super) fn measured(&self, model: &ModelId) -> Option<&UsageAnchor> {
        self.measured
            .as_ref()
            .filter(|(measured_by, _)| measured_by == model)
            .map(|(_, anchor)| anchor)
    }

    /// Records the projected messages of a round about to be sent to
    /// `model`.
    pub(super) fn sending(&mut self, model: ModelId, messages: Vec<Message>) {
        self.in_flight = Some((model, messages));
    }

    /// Settles the parked round: its model, sent messages, and `usage`
    /// replace the measurement when it reported usage; a failed round, or
    /// one with no usage, only releases the sent messages.
    pub(super) fn settle(&mut self, usage: Option<&Usage>) {
        let sent = self.in_flight.take();
        if let (Some((model, messages)), Some(usage)) = (sent, usage) {
            self.measured = Some((model, UsageAnchor::new(messages, usage)));
        }
    }
}

/// One chain: a contained line of section execution.
///
/// The chain owns its per-section frame and adds the chain position (the
/// sibling slice being walked plus the current index), the coroutine handle
/// for the in-flight Lua block, and the walk-scoped slots: the pending
/// Markdown buffer the next Lua fence consumes and the walk's `var` table.
/// One section entry is
/// one frame; the fall-through advance tears the old frame down and the
/// next entry constructs the next.
pub(super) struct Chain {
    /// The chain's hierarchical id: the parent chain's id extended by the
    /// parent's local child counter (the root chain, the main walk, is
    /// `0`; the H1 pass and the walk that follows it are the same chain).
    /// Every id the chain hands out - its children's, its section
    /// entries' - extends this path, so two runs of one prompt allocate
    /// identical ids however their chains interleave.
    pub(super) lineage: ChainId,
    /// The chain's local child and entry counters under `lineage`. The
    /// root chain's entry 0 is the H1 pass (section 0), consumed whether
    /// or not the prompt has H1 blocks, so the first walked section is
    /// always `0.1`.
    pub(super) counters: Counters,
    /// The nearest enclosing task: the chain's own id when the chain is a
    /// spawned task (a fanout arm included), its caller's task for a
    /// `call` child (a blocking child never interleaves with its caller,
    /// so the two share one task), and task `0` for the root chain. Every
    /// section the chain enters reads it as `sys.taskid`.
    pub(super) task: TaskId,
    /// The chain that spawned this chain, when the chain is a task's
    /// backing chain: the task's owner, the only chain allowed to wait on,
    /// inspect, or cancel it. `None` for the root and a `call` child.
    pub(super) owner: Option<ChainIndex>,
    /// A spawned chain's `item` and `sys.index` seeds, consumed by its
    /// first section entry; `None` afterward and on every other chain.
    pub(super) seed: Option<TaskSeed>,
    /// The tasks the chain is parked on in a `join_any` wait (or the
    /// model's `await_tasks`); empty while the chain is not waiting. A
    /// member's chain end delivers it and clears the set.
    pub(super) waiting_on: Vec<TaskId>,
    /// The model's `await_tasks` call the chain is parked in, when
    /// `waiting_on` is that call's set rather than an author `join_any`:
    /// the member's end answers the model's tool call with the drained
    /// notices instead of delivering the member to the shim. `None`
    /// otherwise.
    pub(super) awaiting: Option<AwaitTasks>,
    /// What the chain's suspended request is parked on, as `tasks.status`
    /// reports it (`chat`, `tool_call`, `store`, `timer`, `tasks`, `call`,
    /// or `queued` while the chain waits for a
    /// concurrency slot): set at dispatch or spawn, cleared when the
    /// answer resumes the chain. `None` while the chain runs or between
    /// blocks.
    pub(super) blocked: Option<&'static str>,
    /// Model-task notices not yet delivered into the chain's next model
    /// round, in arrival order: queued (with their task) when a model
    /// task the chain owns ends, drained - each drain joining its task -
    /// by the loop shim's per-round request or by the
    /// model's `await_tasks` answer. The H1 hand-off moves them to the
    /// walk with the pass's tasks.
    pub(super) task_notices: Vec<(TaskId, String)>,
    /// The latest progress note published through `tasks.note` for the
    /// task this chain backs, reported by `tasks.status`.
    pub(super) note: Option<String>,
    /// The chain's fork of the run context: the run's own for the root
    /// chain, `with_args` for a call chain's input override.
    pub(super) ctx: RunState,
    /// The chain's VFS access capability, installed into each section VM
    /// the chain enters: the walk and the live H1 pass share one root
    /// identity, a call chain borrows its parent's (a blocking child is
    /// the same serial thread - no new identity, no false conflicts), and
    /// a task chain forks its own from its spawner's, joined back into
    /// the spawner at every delivery and at chain end. `None` only after
    /// the chain ends: the arena is append-only, so `finish` and
    /// `abort_subtree` take the slot to drop the identity's last
    /// reference at chain end rather than at scheduler drop.
    pub(super) access: Option<Arc<Access>>,
    /// The per-section frame (VM, `sys`, conversation, counts): `Some`
    /// while a section is entered, `None` before the first entry and
    /// between sections.
    pub(super) frame: Option<SectionContext>,
    /// The sibling slice the chain walks, named by its path in the prompt
    /// tree the run shares. A jump to a child swaps this to the jumper's
    /// child slice until the child level exhausts.
    pub(super) slice: SlicePath,
    /// The section of `slice` the chain is running, or the next entry
    /// candidate while the chain is between sections.
    pub(super) index: usize,
    /// The name of the section the chain most recently entered: the name
    /// its reports carry on the walk, including between sections and after
    /// the walk runs off its slice, where `index` names no running section.
    /// Before the first entry, the name of the section at the chain's
    /// start index, or the prompt's title when the index is past the slice.
    pub(super) entered: String,
    /// The suspended parent positions of the chain's jump-started child
    /// walks: the parent slice plus the jumper's index in it. A jump to a
    /// child pushes the current position and descends; when the child
    /// level exhausts, the pop resumes the parent after the jumper.
    pub(super) positions: Vec<(SlicePath, usize)>,
    /// The section's in-flight or next Lua/prose block: while `coroutine`
    /// is `Some` this is the suspended block's index, otherwise the next
    /// block to start.
    pub(super) block: usize,
    /// The coroutine handle for the in-flight Lua block: exists only while
    /// a block is running or suspended; a block that returns disposes of it.
    pub(super) coroutine: Option<Thread>,
    /// The answer delivered for a suspended coroutine, consumed at resume.
    pub(super) incoming: Option<Answer<Error>>,
    /// The pending Markdown buffer: the prose block the next Lua fence
    /// consumes, installed as that block's lazy `prose` template when the
    /// coroutine starts. Cleared at every section entry; an unconsumed
    /// buffer drops with the section, never evaluated.
    pub(super) pending_prose: Option<String>,
    /// The walk's `var` table: seeds each section's VM at entry; the
    /// section's final `var` is read back before teardown and replaces the
    /// slot. A call chain's slot seeds from the caller's snapshot and
    /// is discarded with the chain, so the caller never sees the chain's
    /// writes.
    pub(super) var: serde_json::Value,
    /// The chain's call nesting depth: each call child and each spawned
    /// task runs one level deeper. The recursion cap checks this field,
    /// which carries the depth across a spawn boundary as well as a call.
    pub(super) call_depth: usize,
    /// The chain's effective admission limit: the most tasks this chain
    /// may have admitted at once. The root's is the run's ceiling
    /// ([`RunLimits::max_concurrency`](crate::execute::RunLimits::max_concurrency));
    /// a spawned task and a call chain start with their parent's, and
    /// `tasks.concurrency` lowers it, clamped to the parent's.
    pub(super) concurrency: usize,
    /// The number of admitted tasks currently holding a slot against
    /// this chain: the chain's own children and, transitively, every
    /// descendant's. A task's admission takes a slot here and at every
    /// enclosing ancestor, so the count never exceeds `concurrency`.
    pub(super) slots_used: usize,
    /// Whether this chain currently holds its own admission slots: `true`
    /// from admission until it ends or parks on a task wait, which gives
    /// the slots back so its descendants can run. A task blocked on a
    /// `call` child gives them back while that child parks on a task
    /// wait.
    pub(super) holding: bool,
    /// The nearest holding ancestor whose slots this call chain gave back
    /// when it parked on a task wait: the chain's wake takes that
    /// ancestor's slots back before the chain continues. `None` on every
    /// other chain and once the slots are retaken.
    pub(super) released_holder: Option<ChainIndex>,
    /// Whether the chain was admitted at least once: the start event has
    /// fired, so its terminal event may fire too. A cancelled or
    /// abandoned chain that never ran reports no terminal.
    pub(super) admitted: bool,
    /// A queued task's spawn record, held from spawn until its
    /// admission and consumed by the start event; `None` on every other
    /// chain and after admission.
    pub(super) pending_spawn: Option<SpawnRecord>,
    /// The call parent blocked on this chain, if any.
    pub(super) parent: Option<ChainIndex>,
    /// The tool scope the chain's last `chat` round advertised, keyed by
    /// alias: the round's answer is checked against it, so a tool name the
    /// model invents or reaches for outside the scope fails as out of
    /// scope. `None` before the chain's first round.
    pub(super) advertised: Option<BTreeMap<String, DispatchTarget>>,
    /// The provider's token counts for the chain's `chat` rounds, which the
    /// next round's context precheck counts from.
    pub(super) anchor: ChatAnchor,
    /// The H1 marker: the chain runs the prompt's H1 blocks under its
    /// title - section 0. Such a chain runs the walk's rules with three
    /// deltas: the frame keeps id 0 (no section observations fire), a
    /// scalar return short-circuits the whole run, and the pass's end
    /// starts the root walk with the H1 `var` hand-off. The `slice`/`index`
    /// walk position stays at the top-level slice, unused until a jump out
    /// of H1 starts the walk at the resolved target.
    pub(super) h1: bool,
}

impl Chain {
    /// The chain's access capability for section-VM installation. A live
    /// chain always holds one; `finish` and `abort_subtree` take it at
    /// chain end.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the chain's capability is gone,
    /// which only the chain-end paths do - a live chain always holds it.
    pub(super) fn access(&self) -> Result<&Arc<Access>> {
        self.access
            .as_ref()
            .ok_or(Error::internal("a live chain holds its access capability"))
    }

    /// The chain's current section within `prompt`: the section at its
    /// walk position. Resolved against the caller's handle on the tree so
    /// the result outlives a mutable borrow of the chain.
    fn section<'p>(&self, prompt: &'p Prompt) -> &'p Section {
        &self.slice.resolve(prompt)[self.index]
    }

    /// The chain's current block sequence: the H1 pass's blocks, or
    /// the current section's blocks on the walk.
    pub(super) fn blocks<'p>(&self, prompt: &'p Prompt) -> &'p [Block] {
        if self.h1 {
            promptforge_parser::detail::h1_blocks(prompt)
        } else {
            self.section(prompt).blocks()
        }
    }

    /// The chain's current section name for observations and errors: the
    /// prompt's title for the live H1 pass, else the name of the section
    /// the chain most recently entered - so a chain between sections, or
    /// past its slice's last section, reports the section it just left.
    pub(super) fn section_name(&self) -> &str {
        if self.h1 {
            self.ctx.prompt().title()
        } else {
            &self.entered
        }
    }
}

#[cfg(test)]
#[path = "chain_record-tests.rs"]
mod tests;
