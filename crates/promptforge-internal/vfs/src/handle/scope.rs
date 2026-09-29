//! Scopes and identities: the vector-clock state every claims check
//! reads, and the fork and join that order one identity's accesses
//! before another's.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[cfg(doc)]
use super::{Access, VfsRef};
use crate::error::VfsError;
use crate::path::VfsPath;
use crate::traits::ExecId;

/// A scope id, vended from a process-wide counter: unique across every
/// handle, because one scope's claims can land in several volumes'
/// ledgers through the mounted-handle forward.
pub(super) type ScopeId = u64;

/// A scope: the root identity from [`VfsRef::acquire`] together with
/// every identity forked from it. Every [`Access`] in the scope holds an
/// `Arc` of this, so the scope dies with its last access.
pub(crate) struct Scope {
    /// The scope's id, for the per-ledger registries.
    pub(super) id: ScopeId,
    /// The number of identities whose accesses still live; the scope ends
    /// at zero. An atomic so a claims check on one thread reads another
    /// scope's liveness without taking its lock (which the checker's own
    /// scope lock would deadlock against).
    pub(super) live: AtomicUsize,
    /// Set when the run that owns the scope ends: the scope has ended
    /// however many accesses a host still holds. Separate from `live`
    /// so a later release still balances the count.
    closed: AtomicBool,
    /// Per-identity state, behind the scope's own lock.
    inner: Mutex<ScopeInner>,
}

pub(super) struct ScopeInner {
    /// One record per identity ever in the scope, keyed by its
    /// [`ExecId`]. A record whose refs hit zero stays: its clock is the
    /// identity's final clock, which a late join still reads.
    pub(super) identities: HashMap<ExecId, Identity>,
}

/// One identity's happens-before state.
pub(super) struct Identity {
    /// The vector clock's shared snapshot: how much of every other
    /// identity's activity the identity has seen, without its own
    /// entry. Children fork this `Arc` read-only, so a fanout shares
    /// one map instead of copying it per arm.
    pub(super) seen: Arc<HashMap<ExecId, u64>>,
    /// The fork record: the parent identity and its own entry at fork
    /// time, frozen, chained through the parent's own record. A fresh
    /// acquire has none.
    pub(super) forked: Option<Arc<ForkEdge>>,
    /// The identity's own entry: its logical time, advanced once per
    /// admitted access. Outside the shared snapshot so a fork never
    /// copies the map.
    pub(super) own: u64,
    /// The accesses holding the identity, however many volumes they
    /// touch through a mounted-handle forward.
    pub(super) refs: usize,
}

/// One frozen fork snapshot: the identity forked from and the parent's
/// own entry at that moment. The `prev` link is the parent's own
/// record, so one identity's view of every ancestor is a walk, not a
/// copy.
pub(super) struct ForkEdge {
    /// The parent the identity forked from.
    parent: ExecId,
    /// The parent's own entry at fork time.
    clock: u64,
    /// The parent's fork record: the previous generation's snapshot.
    prev: Option<Arc<ForkEdge>>,
}

/// One identity's happens-before view at a claim check: its own entry,
/// the frozen fork chain, and the shared seen snapshot, taken together
/// so one check and its record read one epoch.
pub(super) struct View {
    pub(super) id: ExecId,
    pub(super) own: u64,
    pub(super) seen: Arc<HashMap<ExecId, u64>>,
    pub(super) forked: Option<Arc<ForkEdge>>,
}

impl View {
    /// How much of `other`'s progress the view has seen: the identity's
    /// own entry for itself, the frozen fork snapshot for an ancestor,
    /// or the shared seen map for everyone else - whichever is newest.
    pub(super) fn seen_clock(&self, other: ExecId) -> u64 {
        if other == self.id {
            return self.own;
        }
        let mut seen = 0;
        let mut edge = self.forked.as_deref();
        while let Some(record) = edge {
            if record.parent == other {
                seen = seen.max(record.clock);
            }
            edge = record.prev.as_deref();
        }
        seen.max(self.seen.get(&other).copied().unwrap_or(0))
    }
}

impl Scope {
    /// A fresh scope with no identities yet: its first attach makes it
    /// live.
    pub(crate) fn start() -> Arc<Scope> {
        Arc::new(Scope {
            id: next_scope_id(),
            live: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            inner: Mutex::new(ScopeInner {
                identities: HashMap::new(),
            }),
        })
    }

    /// Poison-safe lock on the scope's identities.
    pub(super) fn lock(&self) -> MutexGuard<'_, ScopeInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the scope has ended: its run closed it, or every
    /// identity's last access dropped.
    pub(super) fn ended(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.live.load(Ordering::Acquire) == 0
    }

    /// Ends the scope for good: its claims are ignored from here on, and
    /// every access still held refuses its next operation.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Refuses an operation on `path` once the scope is closed, before
    /// any claim or backend call.
    pub(super) fn refuse_if_closed(&self, path: &VfsPath) -> Result<(), VfsError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(VfsError::PermissionDenied {
                path: path.to_string(),
                reason: format!(
                    "the run that owned this access has ended, so {path} can no longer be \
                     touched through it; acquire a fresh access"
                ),
            });
        }
        Ok(())
    }

    /// Adds one access's reference to `id`: the identity registers fresh
    /// on its first access, and a mounted-handle forward of an existing
    /// identity joins its scope with one more reference.
    ///
    /// # Errors
    /// Returns [`VfsError::PermissionDenied`] when the scope is closed.
    pub(super) fn attach(&self, id: ExecId) -> Result<(), VfsError> {
        self.refuse_if_closed(&VfsPath::root())?;
        let mut inner = self.lock();
        match inner.identities.entry(id) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().refs += 1;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                // The own entry starts at 1, not 0: a sibling forked
                // before this identity's first access reads a blank
                // slot for it, which counts as 0, so the first epoch
                // must outrank that or it would be mistaken for an
                // unseen one.
                entry.insert(Identity {
                    seen: Arc::new(HashMap::new()),
                    forked: None,
                    own: 1,
                    refs: 1,
                });
                self.live.fetch_add(1, Ordering::AcqRel);
            }
        }
        Ok(())
    }

    /// Forks `child` from `parent`: the child shares the parent's seen
    /// snapshot read-only and records the parent's entry as one frozen
    /// fork step, and the parent's entry advances, so the parent's
    /// later accesses are not ordered before the child's.
    pub(super) fn fork(&self, parent: ExecId, child: ExecId) {
        let mut inner = self.lock();
        let parent_identity = inner
            .identities
            .get_mut(&parent)
            .unwrap_or_else(|| panic!("a live access's identity is registered"));
        // The child's own entry starts at 1, as an attach's does: its
        // first epoch must outrank every sibling's blank slot.
        let child_identity = Identity {
            seen: Arc::clone(&parent_identity.seen),
            forked: Some(Arc::new(ForkEdge {
                parent,
                clock: parent_identity.own,
                prev: parent_identity.forked.clone(),
            })),
            own: 1,
            refs: 1,
        };
        parent_identity.own += 1;
        inner.identities.insert(child, child_identity);
        self.live.fetch_add(1, Ordering::AcqRel);
    }

    /// Drops one access's reference to `id`; at zero the identity ends
    /// and, when it was the scope's last, the scope ends with it. The
    /// identity's record stays: its clock is its final clock.
    pub(super) fn release(&self, id: ExecId) {
        let mut inner = self.lock();
        if let Some(identity) = inner.identities.get_mut(&id) {
            identity.refs -= 1;
            if identity.refs == 0 {
                self.live.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }

    /// Merges `child`'s final clock into `owner`'s: everything the child
    /// did is ordered before the owner's next step. A missing identity is
    /// a join after the scope purged it, when no live access could still
    /// join, so it merges nothing.
    pub(super) fn join(&self, owner: ExecId, child: ExecId) {
        let mut inner = self.lock();
        let Some(child_identity) = inner.identities.get(&child) else {
            return;
        };
        let child_view = View {
            id: child,
            own: child_identity.own,
            seen: Arc::clone(&child_identity.seen),
            forked: child_identity.forked.clone(),
        };
        let Some(owner_identity) = inner.identities.get_mut(&owner) else {
            return;
        };
        let owner_seen = Arc::make_mut(&mut owner_identity.seen);
        // The child's own entry...
        let slot = owner_seen.entry(child).or_insert(0);
        *slot = (*slot).max(child_view.own);
        // ...its frozen fork chain - the owner's own entry in the chain
        // is never newer than the owner's, so it is skipped - ...
        let mut edge = child_view.forked.as_deref();
        while let Some(record) = edge {
            if record.parent != owner {
                let slot = owner_seen.entry(record.parent).or_insert(0);
                *slot = (*slot).max(record.clock);
            }
            edge = record.prev.as_deref();
        }
        // ...and its shared seen map.
        for (&other, &other_clock) in child_view.seen.iter() {
            if other == owner {
                continue;
            }
            let slot = owner_seen.entry(other).or_insert(0);
            if other_clock > *slot {
                *slot = other_clock;
            }
        }
    }

    /// The identity's happens-before view, taken together so one claim
    /// checks and records against one epoch.
    pub(super) fn view(&self, id: ExecId) -> View {
        let inner = self.lock();
        let identity = inner
            .identities
            .get(&id)
            .unwrap_or_else(|| panic!("a live access's identity is registered"));
        View {
            id,
            own: identity.own,
            seen: Arc::clone(&identity.seen),
            forked: identity.forked.clone(),
        }
    }

    /// Advances `id`'s own entry past its recorded epoch, so its next
    /// access records a fresh one.
    pub(super) fn advance(&self, id: ExecId, tick: u64) {
        let mut inner = self.lock();
        if let Some(identity) = inner.identities.get_mut(&id) {
            identity.own = identity.own.max(tick + 1);
        }
    }
}

/// The next scope id: process-wide, so scope ids stay unique across
/// handles and the mounted-handle forward never aliases two scopes.
fn next_scope_id() -> ScopeId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
