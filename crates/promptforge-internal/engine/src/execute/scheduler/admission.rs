//! Task admission: the slots a task chain takes at its owner and every
//! enclosing ancestor before it runs, and gives back while it waits.
//!
//! Admission bounds the run: every task chain takes a slot at its owner
//! and at every enclosing ancestor, held until the task ends - except
//! while it, or a `call` chain it is blocked on, is parked on a task
//! wait, which gives the slot back so its descendants can run - and a
//! chain admits at most its effective limit at once. The root's limit is the run's
//! [`RunLimits::max_concurrency`](crate::execute::RunLimits::max_concurrency)
//! ceiling; a spawned task and a call chain start with their parent's,
//! and `tasks.concurrency` lowers it, clamped to the parent's. Queue
//! order is spawn order, with resumptions first. The task's start event
//! fires at admission, when the task first runs.

use std::collections::VecDeque;
use std::sync::Arc;

use promptforge_types::event::Event;

use crate::execute::protocol::{Answer, Blocked};
use crate::{Error, Result};

use super::chain_record::{Admission, ChainKind};
use super::{ChainIndex, Scheduler};

impl Scheduler {
    /// The chain enclosing `id` in the task tree: its spawner for a task
    /// chain, its call parent for a call chain, `None` for the root.
    pub(super) fn enclosing(&self, id: ChainIndex) -> Option<ChainIndex> {
        self.chains[id.index()].kind.enclosing()
    }

    /// The chains a slot the task chain `id` holds counts against: its
    /// owner and every enclosing ancestor.
    fn slot_ancestors(&self, id: ChainIndex) -> impl Iterator<Item = ChainIndex> + '_ {
        std::iter::successors(self.chains[id.index()].kind.owner(), |at| {
            self.chains[at.index()].kind.enclosing()
        })
    }

    /// The admission slots in use at each chain, by arena index, counted
    /// from the tasks holding theirs: a chain's count is its own
    /// children's and, transitively, every descendant's, so no count is
    /// stored that could drift from the tasks' `holding` state.
    fn slots_in_use(&self) -> Vec<usize> {
        let mut used = vec![0; self.chains.len()];
        // The arena is u32-bounded at insertion (`start_chain`), so the
        // index range covers every chain.
        for (id, chain) in (0..).map(ChainIndex).zip(&self.chains) {
            if chain.kind.holding() {
                self.count_slot(&mut used, id);
            }
        }
        used
    }

    /// Counts one slot the task chain `id` holds into `used`, at its owner
    /// and every enclosing ancestor.
    fn count_slot(&self, used: &mut [usize], id: ChainIndex) {
        for ancestor in self.slot_ancestors(id) {
            used[ancestor.index()] += 1;
        }
    }

    /// Whether the queued task chain `id` can be admitted with the slots
    /// `used`: a free slot at its owner and at every enclosing ancestor,
    /// so the task counts against each chain's limit on the way up.
    fn can_admit(&self, used: &[usize], id: ChainIndex) -> bool {
        self.slot_ancestors(id).all(|ancestor| {
            used[ancestor.index()] < self.chains[ancestor.index()].concurrency.get()
        })
    }

    /// Gives back `id`'s admission slots when it holds any: the shared
    /// half of the chain-end paths and a task wait's park.
    pub(super) fn release_chain_slots(&mut self, id: ChainIndex) {
        self.chains[id.index()].kind.stop_holding();
    }

    /// Admits every queued task that can take a slot now: resumptions
    /// first (a task resumed from a join takes its slots back ahead of
    /// tasks that have not started, so a resume cannot starve behind a
    /// long queue), then fresh spawns in spawn order. The slots in use
    /// are counted once per pass, and each admission adds its own.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when a queued chain is not the kind of
    /// chain its queue holds, which only a scheduler bug produces.
    pub(super) fn admit(&mut self) -> Result<()> {
        if self.resuming.is_empty() && self.spawned.is_empty() {
            return Ok(());
        }
        let mut used = self.slots_in_use();
        let mut remaining = VecDeque::new();
        for id in std::mem::take(&mut self.resuming) {
            let holder = self.slot_holder(id);
            if self.can_admit(&used, holder) {
                self.resume_chain(id)?;
                self.count_slot(&mut used, holder);
            } else {
                remaining.push_back(id);
            }
        }
        self.resuming = remaining;
        let mut remaining = VecDeque::new();
        for id in std::mem::take(&mut self.spawned) {
            if self.can_admit(&used, id) {
                self.admit_chain(id)?;
                self.count_slot(&mut used, id);
            } else {
                remaining.push_back(id);
            }
        }
        self.spawned = remaining;
        Ok(())
    }

    /// The chain whose slots `id`'s resumption takes back: the holding
    /// ancestor a parked call chain released, or the resumed task itself.
    fn slot_holder(&self, id: ChainIndex) -> ChainIndex {
        match self.chains[id.index()].kind {
            ChainKind::Call {
                released_holder: Some(holder),
                ..
            } => holder,
            ChainKind::Root | ChainKind::H1 | ChainKind::Call { .. } | ChainKind::Task { .. } => id,
        }
    }

    /// Re-admits a resumed chain: marks its slot holder holding again and
    /// enqueues it. Its start event already fired at its first admission -
    /// this path only moves it from its wait back to running.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the slot holder is not an admitted
    /// task, which only a scheduler bug produces: only an admitted task
    /// holds slots to give back.
    fn resume_chain(&mut self, id: ChainIndex) -> Result<()> {
        let holder = self.slot_holder(id);
        let ChainKind::Task {
            admission: Admission::Admitted { holding },
            ..
        } = &mut self.chains[holder.index()].kind
        else {
            return Err(Error::internal(
                "a resumed chain's slots belong to an admitted task",
            ));
        };
        *holding = true;
        let chain = &mut self.chains[id.index()];
        if let ChainKind::Call {
            released_holder, ..
        } = &mut chain.kind
        {
            *released_holder = None;
        }
        chain.blocked = None;
        self.ready.push_back(id);
        Ok(())
    }

    /// Admits one queued task chain: marks it holding a slot at its owner
    /// and every enclosing ancestor, installs its spawn record, fires the
    /// start event on the spawner's sequence under the spawning section
    /// (the spawn-time capture, so a task admitted after its spawner moved
    /// on still reports where it was spawned), and enqueues it. The start
    /// event marks admission - the task first runs now - so a task still
    /// waiting for a slot reports no start event, and its Lua VM is only
    /// created at its first section entry, after admission.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the chain is not a queued task or
    /// its task has no slot, which only a scheduler bug produces: a spawn
    /// queues a task chain with its record and registers its slot first.
    fn admit_chain(&mut self, id: ChainIndex) -> Result<()> {
        let chain = &mut self.chains[id.index()];
        let ChainKind::Task {
            owner: spawner,
            admission,
        } = &mut chain.kind
        else {
            return Err(Error::internal("a queued task chain has a spawner"));
        };
        let spawner = *spawner;
        let Admission::Queued(record) =
            std::mem::replace(admission, Admission::Admitted { holding: true })
        else {
            return Err(Error::internal(
                "a queued task chain holds its spawn record",
            ));
        };
        chain.blocked = None;
        let task = chain.task.clone();
        let item = chain.seed.as_ref().and_then(|seed| seed.item.clone());
        let index = chain.seed.as_ref().and_then(|seed| seed.index);
        let var = chain.var.clone();
        let Some(slot) = self.tasks.get(&task) else {
            return Err(Error::internal("a task chain has a slot"));
        };
        let target = slot.target.clone();
        let origin = slot.origin;
        let emitter = Arc::clone(self.chains[spawner.index()].ctx.emitter());
        // The start event fires at admission, on the spawner's sequence:
        // the task first runs now, and the payload's seeds are enough to
        // start the same chain again under the same id.
        emitter.emit(&record.section, |execution, section, provenance| {
            Event::TaskStarted {
                execution,
                section,
                provenance,
                task,
                target,
                origin,
                input: record.input,
                item,
                index,
                var,
            }
        });
        self.ready.push_back(id);
        Ok(())
    }

    /// A chain parked on a task wait gives its admission slots back when
    /// the wait holds real tasks - a timer-only wait keeps them, since
    /// nothing it waits for needs a slot - so its descendants can run
    /// under its limit. A call chain holds no slots of its own, so it
    /// gives back those of its nearest holding ancestor through its call
    /// parent (the task it runs inside, blocked on the call) and records
    /// which chain gave them. A call chain in the main walk finds no
    /// holding ancestor, and the main walk holds no slots to give.
    pub(super) fn park_wait(&mut self, id: ChainIndex) {
        let chain = &self.chains[id.index()];
        let waits_on_tasks = chain
            .waiting_on
            .iter()
            .any(|task| self.tasks.get(task).is_some_and(|slot| !slot.is_internal()));
        if !waits_on_tasks {
            return;
        }
        if chain.kind.holding() {
            self.release_chain_slots(id);
            return;
        }
        let mut at = chain.kind.parent();
        while let Some(ancestor) = at {
            let chain = &self.chains[ancestor.index()];
            if chain.kind.holding() {
                self.release_chain_slots(ancestor);
                if let ChainKind::Call {
                    released_holder, ..
                } = &mut self.chains[id.index()].kind
                {
                    *released_holder = Some(ancestor);
                }
                return;
            }
            at = chain.kind.parent();
        }
    }

    /// Resumes a chain parked on a task wait with its delivered answer:
    /// a task chain that gave its slots back, and a call chain that gave
    /// back its ancestor's, take them again - admitted ahead of fresh
    /// starts at the drain's edge - while the main walk, and a chain that
    /// kept its slots (a timer-only wait), resume inline.
    pub(super) fn wake_from_wait(&mut self, id: ChainIndex, answer: Answer<Error>) {
        let chain = &mut self.chains[id.index()];
        let requeue = match &chain.kind {
            ChainKind::Call {
                released_holder, ..
            } => released_holder.is_some(),
            ChainKind::Task { .. } => !chain.kind.holding(),
            ChainKind::Root | ChainKind::H1 => false,
        };
        chain.incoming = Some(answer);
        if requeue {
            // Waiting for a slot again reads `queued`, exactly as a
            // fresh spawn's wait does.
            chain.blocked = Some(Blocked::Queued);
            self.resuming.push_back(id);
        } else {
            self.ready.push_back(id);
        }
    }
}
