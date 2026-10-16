//! What kind of chain a record is: the main walk, the live H1 pass, a
//! `call` child, or a task's backing chain, each with the links and the
//! admission state only that kind has.

use super::super::{ChainIndex, SpawnRecord};

/// Where a task chain's admission stands.
#[derive(Debug)]
pub(in crate::execute::scheduler) enum Admission {
    /// Queued since its spawn and never run: the spawn record its start
    /// event reports once it is admitted.
    Queued(SpawnRecord),
    /// Admitted at least once, so its start event has fired and its
    /// terminal event may fire too.
    Admitted {
        /// Whether the chain holds its own admission slots now: from
        /// admission until it ends or parks on a task wait, which gives
        /// them back so its descendants can run. A task blocked on a
        /// `call` child gives them back while that child parks on a task
        /// wait.
        holding: bool,
    },
}

/// What kind of chain a record is.
#[derive(Debug)]
pub(in crate::execute::scheduler) enum ChainKind {
    /// The main walk.
    Root,
    /// The live H1 pass: the prompt's H1 blocks under its title - section
    /// 0. It runs the walk's rules with three deltas: the frame keeps id
    /// 0 (no section observations fire), a scalar return short-circuits
    /// the whole run, and the pass's end starts the root walk with the H1
    /// `var` hand-off. The `slice`/`index` walk position stays at the
    /// top-level slice, unused until a jump out of H1 starts the walk at
    /// the resolved target.
    H1,
    /// A `call` child, whose parent is blocked on it.
    Call {
        /// The call parent blocked on this chain.
        parent: ChainIndex,
        /// The nearest holding ancestor whose slots this chain gave back
        /// when it parked on a task wait: the chain's wake takes that
        /// ancestor's slots back before the chain continues. `None` while
        /// the chain is not parked that way.
        released_holder: Option<ChainIndex>,
    },
    /// A spawned task's backing chain, a fanout arm included.
    Task {
        /// The chain that spawned the task: the only chain allowed to
        /// wait on, inspect, or cancel it.
        owner: ChainIndex,
        /// Where the task's admission stands.
        admission: Admission,
    },
}

impl ChainKind {
    /// Whether the chain is the live H1 pass.
    pub(in crate::execute::scheduler) fn is_h1(&self) -> bool {
        matches!(self, ChainKind::H1)
    }

    /// The task's owner, when the chain backs a task.
    pub(in crate::execute::scheduler) fn owner(&self) -> Option<ChainIndex> {
        match self {
            ChainKind::Task { owner, .. } => Some(*owner),
            ChainKind::Root | ChainKind::H1 | ChainKind::Call { .. } => None,
        }
    }

    /// The call parent blocked on the chain, when it is a `call` child.
    pub(in crate::execute::scheduler) fn parent(&self) -> Option<ChainIndex> {
        match self {
            ChainKind::Call { parent, .. } => Some(*parent),
            ChainKind::Root | ChainKind::H1 | ChainKind::Task { .. } => None,
        }
    }

    /// The chain enclosing this one in the task tree: a task's owner, a
    /// `call` child's parent, and `None` for the root walk and the H1
    /// pass.
    pub(in crate::execute::scheduler) fn enclosing(&self) -> Option<ChainIndex> {
        self.owner().or(self.parent())
    }

    /// Whether the chain backs a task that has been admitted at least
    /// once.
    pub(in crate::execute::scheduler) fn admitted(&self) -> bool {
        matches!(
            self,
            ChainKind::Task {
                admission: Admission::Admitted { .. },
                ..
            }
        )
    }

    /// Whether the chain backs a task that holds its admission slots now.
    pub(in crate::execute::scheduler) fn holding(&self) -> bool {
        matches!(
            self,
            ChainKind::Task {
                admission: Admission::Admitted { holding: true },
                ..
            }
        )
    }

    /// Marks a holding task's slots given back.
    pub(in crate::execute::scheduler) fn stop_holding(&mut self) {
        match self {
            ChainKind::Task {
                admission: Admission::Admitted { holding },
                ..
            } => *holding = false,
            ChainKind::Root
            | ChainKind::H1
            | ChainKind::Call { .. }
            | ChainKind::Task {
                admission: Admission::Queued(_),
                ..
            } => {}
        }
    }
}
