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

use crate::Error;
use crate::execute::protocol::Answer;

use super::{ChainIndex, Scheduler};

impl Scheduler {
    /// The chain enclosing `id` in the task tree: its spawner for a task
    /// chain, its call parent for a call chain, `None` for the root.
    pub(super) fn enclosing(&self, id: ChainIndex) -> Option<ChainIndex> {
        let chain = &self.chains[id.index()];
        chain.owner.or(chain.parent)
    }

    /// Whether the queued task chain `id` can be admitted now: a free
    /// slot at its owner and at every enclosing ancestor, so the task
    /// counts against each chain's limit on the way up.
    fn can_admit(&self, id: ChainIndex) -> bool {
        let mut at = self.chains[id.index()].owner;
        while let Some(ancestor) = at {
            let chain = &self.chains[ancestor.index()];
            if chain.slots_used >= chain.concurrency {
                return false;
            }
            at = chain.owner.or(chain.parent);
        }
        true
    }

    /// Takes one admission slot at the task's owner and every enclosing
    /// ancestor, as [`can_admit`](Self::can_admit) checked.
    fn take_slots(&mut self, id: ChainIndex) {
        let mut at = self.chains[id.index()].owner;
        while let Some(ancestor) = at {
            let chain = &mut self.chains[ancestor.index()];
            chain.slots_used += 1;
            at = chain.owner.or(chain.parent);
        }
    }

    /// Gives back the slots a running task held at its owner and every
    /// enclosing ancestor.
    fn release_slots(&mut self, id: ChainIndex) {
        let mut at = self.chains[id.index()].owner;
        while let Some(ancestor) = at {
            let chain = &mut self.chains[ancestor.index()];
            debug_assert!(
                chain.slots_used > 0,
                "a chain holding a slot cannot release below zero"
            );
            chain.slots_used = chain.slots_used.saturating_sub(1);
            at = chain.owner.or(chain.parent);
        }
    }

    /// Releases `id`'s held admission slots when it holds any: the shared
    /// half of the chain-end paths and a task wait's park.
    pub(super) fn release_chain_slots(&mut self, id: ChainIndex) {
        let holding = self.chains[id.index()].holding;
        if holding {
            self.release_slots(id);
            self.chains[id.index()].holding = false;
        }
    }

    /// Admits every queued task that can take a slot now: resumptions
    /// first (a task resumed from a join takes its slots back ahead of
    /// tasks that have not started, so a resume cannot starve behind a
    /// long queue), then fresh spawns in spawn order.
    pub(super) fn admit(&mut self) {
        let mut remaining = VecDeque::new();
        for id in std::mem::take(&mut self.resuming) {
            if self.can_admit(self.slot_holder(id)) {
                self.resume_chain(id);
            } else {
                remaining.push_back(id);
            }
        }
        self.resuming = remaining;
        let mut remaining = VecDeque::new();
        for id in std::mem::take(&mut self.spawned) {
            if self.can_admit(id) {
                self.admit_chain(id);
            } else {
                remaining.push_back(id);
            }
        }
        self.spawned = remaining;
    }

    /// The chain whose slots `id`'s resumption takes back: the holding
    /// ancestor a parked call chain released, or the resumed task itself.
    fn slot_holder(&self, id: ChainIndex) -> ChainIndex {
        self.chains[id.index()].released_holder.unwrap_or(id)
    }

    /// Re-admits a resumed chain: takes its slot holder's slots back and
    /// enqueues it. Its start event already fired at its first admission -
    /// this path only moves it from its wait back to running.
    fn resume_chain(&mut self, id: ChainIndex) {
        let holder = self.slot_holder(id);
        self.take_slots(holder);
        self.chains[holder.index()].holding = true;
        let chain = &mut self.chains[id.index()];
        chain.released_holder = None;
        chain.blocked = None;
        self.ready.push_back(id);
    }

    /// Admits one queued task chain: takes a slot at its owner and every
    /// enclosing ancestor, installs its spawn record, fires the start
    /// event on the spawner's sequence under the spawning section (the
    /// spawn-time capture, so a task admitted after its spawner moved on
    /// still reports where it was spawned), and enqueues it. The start
    /// event marks admission - the task first runs now - so a task still
    /// waiting for a slot reports no start event, and its Lua VM is only
    /// created at its first section entry, after admission.
    fn admit_chain(&mut self, id: ChainIndex) {
        self.take_slots(id);
        let (spawner, task, input, item, index, var, section) = {
            let chain = &mut self.chains[id.index()];
            chain.holding = true;
            chain.admitted = true;
            chain.blocked = None;
            let spawner = chain
                .owner
                .unwrap_or_else(|| unreachable!("a queued task chain has a spawner"));
            let record = chain
                .pending_spawn
                .take()
                .unwrap_or_else(|| unreachable!("a queued task chain holds its spawn record"));
            (
                spawner,
                chain.task.clone(),
                record.input,
                chain.seed.as_ref().and_then(|seed| seed.item.clone()),
                chain.seed.as_ref().and_then(|seed| seed.index),
                chain.var.clone(),
                record.section,
            )
        };
        let (target, origin, emitter) = {
            let slot = self
                .tasks
                .get(&task)
                .unwrap_or_else(|| unreachable!("a task chain has a slot"));
            let spawner_chain = &self.chains[spawner.index()];
            (
                slot.target.clone(),
                slot.origin,
                Arc::clone(spawner_chain.ctx.emitter()),
            )
        };
        // The start event fires at admission, on the spawner's sequence:
        // the task first runs now, and the payload's seeds are enough to
        // start the same chain again under the same id.
        emitter.emit(&section, |execution, section, provenance| {
            Event::TaskStarted {
                execution,
                section,
                provenance,
                task,
                target,
                origin,
                input,
                item,
                index,
                var,
            }
        });
        self.ready.push_back(id);
    }

    /// A chain parked on a task wait gives its admission slots back when
    /// the wait holds real tasks - a timer-only wait keeps them, since
    /// nothing it waits for needs a slot - so its descendants can run
    /// under its limit. A call chain holds no slots of its own, so it
    /// gives back those of its nearest holding ancestor through `parent`
    /// (the task it runs inside, blocked on the call) and records which
    /// chain gave them. A call chain in the main walk finds no holding
    /// ancestor, and the main walk holds no slots to give.
    pub(super) fn park_wait(&mut self, id: ChainIndex) {
        let chain = &self.chains[id.index()];
        let waits_on_tasks = chain
            .waiting_on
            .iter()
            .any(|task| self.tasks.get(task).is_some_and(|slot| !slot.is_internal()));
        if !waits_on_tasks {
            return;
        }
        if chain.holding {
            self.release_chain_slots(id);
            return;
        }
        let mut at = chain.parent;
        while let Some(ancestor) = at {
            let chain = &self.chains[ancestor.index()];
            if chain.holding {
                self.release_chain_slots(ancestor);
                self.chains[id.index()].released_holder = Some(ancestor);
                return;
            }
            at = chain.parent;
        }
    }

    /// Resumes a chain parked on a task wait with its delivered answer:
    /// a task chain that gave its slots back, and a call chain that gave
    /// back its ancestor's, take them again - admitted ahead of fresh
    /// starts at the drain's edge - while the main walk, and a chain that
    /// kept its slots (a timer-only wait), resume inline.
    pub(super) fn wake_from_wait(&mut self, id: ChainIndex, answer: Answer<Error>) {
        let chain = &self.chains[id.index()];
        let requeue = chain.released_holder.is_some() || (!chain.holding && chain.owner.is_some());
        let chain = &mut self.chains[id.index()];
        chain.incoming = Some(answer);
        if requeue {
            // Waiting for a slot again reads `queued`, exactly as a
            // fresh spawn's wait does.
            chain.blocked = Some("queued");
            self.resuming.push_back(id);
        } else {
            self.ready.push_back(id);
        }
    }
}
