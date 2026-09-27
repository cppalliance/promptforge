//! The cloneable handle, the RAII capability, and the happens-before
//! claims ledger.
//!
//! [`VfsRef`] is the public handle: an `Arc`-shared volume pairing one
//! backend with the claims ledger, behind poison-safe locks. [`Access`]
//! is the RAII capability vended by [`VfsRef::acquire`]: it canonicalizes
//! paths at receipt, consults the handle's policy before the claims check
//! so a denied operation never registers a claim, records its claims, and
//! locks the backend's access object per call. A handle with an installed
//! op sink fires it on every admitted operation - after policy and claims
//! pass, before the backend executes; see [`crate::observe`].
//!
//! # Scopes, fork and join
//!
//! [`VfsRef::acquire`] starts a *scope*: the acquired [`Access`] is the
//! root identity, and every identity [`Access::spawn`] forks joins the
//! scope. Each identity holds a vector clock, and every admitted access
//! records an *epoch* - its identity paired with its own clock entry at
//! that moment - on each region it touches. One epoch is ordered before
//! another identity's next step exactly when the other's clock has seen
//! the epoch's entry, so conflicts follow happens-before (FastTrack-style,
//! Flanagan and Freund, PLDI 2009) instead of liveness: a spawn forks the
//! parent's clock into the child, and a join - [`crate::detail::access_join`] -
//! merges the child's final clock back into the owner's. An identity's own
//! entry lives outside the shared snapshot, so a spawn reuses the parent's
//! map read-only and records the parent's entry as one frozen fork step:
//! a fanout shares one map instead of copying it per arm. Claims are never
//! released during a scope's life. An identity ends when its last
//! [`Access`] drops, its final clock stays in the scope for late joins,
//! and a scope ends with its last identity; its claims are then ignored
//! and purged lazily. Two live scopes never order each other, so their
//! claims always conflict. See the reference docs on the facade for the
//! region model: a read claims the path, directory children, or pattern
//! it observes; a write claims its path, the ancestors it may create, or
//! the whole subtree it removes.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use crate::error::{PathReason, VfsError};
use crate::glob::{compile_glob, matches_tokens, validate_glob_grammar, validate_glob_pattern};
use crate::grep::{GrepQuery, GrepResults};
use crate::observe::{OpEvent, OpSink, Origin};
use crate::path::{VfsPath, VfsPathBuf, canonicalize, canonicalize_absolute};
use crate::router::{Mounts, Router, StoreDecl, VfsRefBuilder};
use crate::stat::{Entry, Stat};
use crate::traits::{AcquireContext, AllowAll, ExecId, Op, Policy, Verdict, Vfs, VfsAccess};

/// Whether an operation claims read or write intent on its region.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClaimKind {
    Read,
    Write,
}

impl fmt::Display for ClaimKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => f.write_str("read"),
            Self::Write => f.write_str("write"),
        }
    }
}

/// One recorded access: the identity paired with its own clock entry at
/// that moment.
#[derive(Clone, Copy)]
struct Epoch {
    id: ExecId,
    clock: u64,
}

/// A scope id, vended from a process-wide counter: unique across every
/// handle, because one scope's claims can land in several volumes'
/// ledgers through the mounted-handle forward.
type ScopeId = u64;

/// A scope: the root identity from [`VfsRef::acquire`] together with
/// every identity forked from it. Every [`Access`] in the scope holds an
/// `Arc` of this, so the scope dies with its last access.
pub(crate) struct Scope {
    /// The scope's id, for the per-ledger registries.
    id: ScopeId,
    /// The number of identities whose accesses still live; the scope ends
    /// at zero. An atomic so a claims check on one thread reads another
    /// scope's liveness without taking its lock (which the checker's own
    /// scope lock would deadlock against).
    live: AtomicUsize,
    /// Per-identity state, behind the scope's own lock.
    inner: Mutex<ScopeInner>,
}

struct ScopeInner {
    /// One record per identity ever in the scope, keyed by its
    /// [`ExecId`]. A record whose refs hit zero stays: its clock is the
    /// identity's final clock, which a late join still reads.
    identities: HashMap<ExecId, Identity>,
}

/// One identity's happens-before state.
struct Identity {
    /// The vector clock's shared snapshot: how much of every other
    /// identity's activity the identity has seen, without its own
    /// entry. Children fork this `Arc` read-only, so a fanout shares
    /// one map instead of copying it per arm.
    seen: Arc<HashMap<ExecId, u64>>,
    /// The fork record: the parent identity and its own entry at fork
    /// time, frozen, chained through the parent's own record. A fresh
    /// acquire has none.
    forked: Option<Arc<ForkEdge>>,
    /// The identity's own entry: its logical time, advanced once per
    /// admitted access. Outside the shared snapshot so a fork never
    /// copies the map.
    own: u64,
    /// The accesses holding the identity, however many volumes they
    /// touch through a mounted-handle forward.
    refs: usize,
}

/// One frozen fork snapshot: the identity forked from and the parent's
/// own entry at that moment. The `prev` link is the parent's own
/// record, so one identity's view of every ancestor is a walk, not a
/// copy.
struct ForkEdge {
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
struct View {
    id: ExecId,
    own: u64,
    seen: Arc<HashMap<ExecId, u64>>,
    forked: Option<Arc<ForkEdge>>,
}

impl View {
    /// How much of `other`'s progress the view has seen: the identity's
    /// own entry for itself, the frozen fork snapshot for an ancestor,
    /// or the shared seen map for everyone else - whichever is newest.
    fn seen_clock(&self, other: ExecId) -> u64 {
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
            inner: Mutex::new(ScopeInner {
                identities: HashMap::new(),
            }),
        })
    }

    /// Poison-safe lock on the scope's identities.
    fn lock(&self) -> MutexGuard<'_, ScopeInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the scope has ended: every identity's last access dropped.
    fn ended(&self) -> bool {
        self.live.load(Ordering::Acquire) == 0
    }

    /// Adds one access's reference to `id`: the identity registers fresh
    /// on its first access, and a mounted-handle forward of an existing
    /// identity joins its scope with one more reference.
    fn attach(&self, id: ExecId) {
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
    }

    /// Forks `child` from `parent`: the child shares the parent's seen
    /// snapshot read-only and records the parent's entry as one frozen
    /// fork step, and the parent's entry advances, so the parent's
    /// later accesses are not ordered before the child's.
    fn fork(&self, parent: ExecId, child: ExecId) {
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
    fn release(&self, id: ExecId) {
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
    fn join(&self, owner: ExecId, child: ExecId) {
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
    fn view(&self, id: ExecId) -> View {
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
    fn advance(&self, id: ExecId, tick: u64) {
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

/// The bookkeeping of who touched what, shared by every scope of one
/// storage view. Claims are never released during a scope's life: they
/// are ignored once their scope ends and purged lazily.
struct Claims {
    inner: Mutex<ClaimsTables>,
}

/// How large the tables must grow before a prune runs.
const PRUNE_AT: usize = 4096;

/// One region's claims: the last write as a single epoch, reads as the
/// latest epoch per identity, and may-create writes the same way - the
/// FastTrack representation, which never stores a full vector clock per
/// claim. Every claim is tagged with its scope.
#[derive(Default)]
struct RegionClaims {
    /// The last write epoch and its scope.
    write: Option<(ScopeId, Epoch)>,
    /// Read epochs: the latest per identity, tagged by scope.
    reads: HashMap<ExecId, (ScopeId, u64)>,
    /// May-create write epochs: the latest per identity, tagged by scope.
    created: HashMap<ExecId, (ScopeId, u64)>,
}

struct ClaimsTables {
    /// Path-keyed regions, one map per kind, so the conservative scans
    /// (patterns matching paths, subtrees covering paths) walk only the
    /// regions that can overlap them.
    paths: HashMap<VfsPath, RegionClaims>,
    children: HashMap<VfsPath, RegionClaims>,
    patterns: HashMap<VfsPath, RegionClaims>,
    subtrees: HashMap<VfsPath, RegionClaims>,
    ancestors: HashMap<VfsPath, RegionClaims>,
    /// The scopes with claims in these tables, for the liveness check.
    scopes: HashMap<ScopeId, Weak<Scope>>,
    /// The approximate claim count, for the prune trigger.
    entries: usize,
}

impl ClaimsTables {
    fn new() -> Self {
        Self {
            paths: HashMap::new(),
            children: HashMap::new(),
            patterns: HashMap::new(),
            subtrees: HashMap::new(),
            ancestors: HashMap::new(),
            scopes: HashMap::new(),
            entries: 0,
        }
    }

    /// Whether `other`'s claim conflicts with an access by `view` of
    /// `scope`. Within one scope the claim is ordered before the access
    /// exactly when the access's view has seen it; claims from another
    /// live scope always conflict, and an ended scope's are ignored.
    fn conflicts(
        &self,
        scope: &Scope,
        view: &View,
        other_scope: ScopeId,
        other: ExecId,
        other_clock: u64,
    ) -> bool {
        if other_scope == scope.id {
            return other_clock > view.seen_clock(other);
        }
        self.scopes
            .get(&other_scope)
            .and_then(Weak::upgrade)
            .is_some_and(|other| !other.ended())
    }
}

impl Claims {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ClaimsTables::new()),
        }
    }

    /// Poison-safe lock: each guard scope is one complete table mutation,
    /// so a panicking claimant cannot leave the tables half-updated.
    fn tables(&self) -> MutexGuard<'_, ClaimsTables> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers `scope` in these tables' registry: claims tagged with
    /// the scope's id stay checkable for liveness.
    fn register_scope(&self, scope: &Arc<Scope>) {
        self.tables()
            .scopes
            .entry(scope.id)
            .or_insert_with(|| Arc::downgrade(scope));
    }

    /// Checks and records a write of `path` by `id` of `scope`: the path
    /// claim, the may-create claims on its ancestors, and the checks
    /// against the regions it overlaps - the path's own reads and writes,
    /// its parent's children, every pattern that matches it, and every
    /// subtree that covers it.
    fn claim_write(&self, scope: &Arc<Scope>, id: ExecId, path: &VfsPath) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id);
        if let Some(region) = tables.paths.get(path) {
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, &view, other_scope, epoch.id, epoch.clock)
            {
                return Err(conflict(
                    path,
                    id,
                    ClaimKind::Write,
                    epoch.id,
                    ClaimKind::Write,
                ));
            }
            for (&other, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, &view, other_scope, other, other_clock) {
                    return Err(conflict(path, id, ClaimKind::Write, other, ClaimKind::Read));
                }
            }
        }
        if let Some(parent) = parent_of(path)
            && let Some(region) = tables.children.get(&parent)
        {
            for (&other, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, &view, other_scope, other, other_clock) {
                    return Err(conflict(path, id, ClaimKind::Write, other, ClaimKind::Read));
                }
            }
        }
        for (pattern, region) in &tables.patterns {
            if !pattern_matches_path(pattern, path) {
                continue;
            }
            for (&other, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, &view, other_scope, other, other_clock) {
                    return Err(conflict(path, id, ClaimKind::Write, other, ClaimKind::Read));
                }
            }
        }
        for (subtree, region) in &tables.subtrees {
            if !subtree_covers(subtree, path) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, &view, other_scope, epoch.id, epoch.clock)
            {
                return Err(conflict(
                    path,
                    id,
                    ClaimKind::Write,
                    epoch.id,
                    ClaimKind::Write,
                ));
            }
        }
        // The record: the last write replaces the standing one, and reads
        // ordered before it clear (an access racing with one of them
        // races with the write instead).
        let region = tables.paths.entry(path.clone()).or_default();
        region.write = Some((
            scope.id,
            Epoch {
                id,
                clock: view.own,
            },
        ));
        region
            .reads
            .retain(|&other, &mut (other_scope, other_clock)| {
                other_scope != scope.id || other_clock > view.seen_clock(other)
            });
        for ancestor in may_create(path) {
            // The ancestors it may create are checked against reads only:
            // a read of the ancestor observes the entry this write may
            // create under it, so it conflicts, while another write may
            // create alongside.
            if let Some(region) = tables.paths.get(&ancestor) {
                for (&other, &(other_scope, other_clock)) in &region.reads {
                    if tables.conflicts(scope, &view, other_scope, other, other_clock) {
                        return Err(conflict(path, id, ClaimKind::Write, other, ClaimKind::Read));
                    }
                }
            }
            let region = tables.ancestors.entry(ancestor).or_default();
            region.created.insert(id, (scope.id, view.own));
        }
        Self::finish_claim(scope, id, &mut tables, view.own);
        Ok(())
    }

    /// Checks and records a read of `path` by `id` of `scope`: the path's
    /// last write, the may-create writes on the path itself, and every
    /// subtree that covers it. Reads never conflict with reads.
    fn claim_read(&self, scope: &Arc<Scope>, id: ExecId, path: &VfsPath) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id);
        if let Some(region) = tables.paths.get(path)
            && let Some((other_scope, epoch)) = region.write
            && tables.conflicts(scope, &view, other_scope, epoch.id, epoch.clock)
        {
            return Err(conflict(
                path,
                id,
                ClaimKind::Read,
                epoch.id,
                ClaimKind::Write,
            ));
        }
        if let Some(region) = tables.ancestors.get(path) {
            for (&other, &(other_scope, other_clock)) in &region.created {
                if tables.conflicts(scope, &view, other_scope, other, other_clock) {
                    return Err(conflict(path, id, ClaimKind::Read, other, ClaimKind::Write));
                }
            }
        }
        for (subtree, region) in &tables.subtrees {
            if !subtree_covers(subtree, path) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, &view, other_scope, epoch.id, epoch.clock)
            {
                return Err(conflict(
                    path,
                    id,
                    ClaimKind::Read,
                    epoch.id,
                    ClaimKind::Write,
                ));
            }
        }
        let region = tables.paths.entry(path.clone()).or_default();
        region.reads.insert(id, (scope.id, view.own));
        Self::finish_claim(scope, id, &mut tables, view.own);
        Ok(())
    }

    /// Checks and records a list of `dir`'s children by `id` of `scope`:
    /// the may-create writes that would change the listing, and every
    /// subtree that covers it.
    fn claim_list(&self, scope: &Arc<Scope>, id: ExecId, dir: &VfsPath) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id);
        if let Some(region) = tables.ancestors.get(dir) {
            for (&other, &(other_scope, other_clock)) in &region.created {
                if tables.conflicts(scope, &view, other_scope, other, other_clock) {
                    return Err(conflict(dir, id, ClaimKind::Read, other, ClaimKind::Write));
                }
            }
        }
        for (subtree, region) in &tables.subtrees {
            if !subtree_covers(subtree, dir) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, &view, other_scope, epoch.id, epoch.clock)
            {
                return Err(conflict(
                    dir,
                    id,
                    ClaimKind::Read,
                    epoch.id,
                    ClaimKind::Write,
                ));
            }
        }
        let region = tables.children.entry(dir.clone()).or_default();
        region.reads.insert(id, (scope.id, view.own));
        Self::finish_claim(scope, id, &mut tables, view.own);
        Ok(())
    }

    /// Checks and records a glob of `pattern` by `id` of `scope`: every
    /// path the pattern matches, the may-create writes under its literal
    /// base, and every subtree it overlaps. The pattern itself is the
    /// claim, not each match.
    fn claim_glob(
        &self,
        scope: &Arc<Scope>,
        id: ExecId,
        pattern: &VfsPath,
    ) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id);
        for (path, region) in &tables.paths {
            if !pattern_matches_path(pattern, path) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, &view, other_scope, epoch.id, epoch.clock)
            {
                return Err(conflict(
                    pattern,
                    id,
                    ClaimKind::Read,
                    epoch.id,
                    ClaimKind::Write,
                ));
            }
        }
        for ancestor in pattern_base(pattern) {
            if let Some(region) = tables.ancestors.get(&ancestor) {
                for (&other, &(other_scope, other_clock)) in &region.created {
                    if tables.conflicts(scope, &view, other_scope, other, other_clock) {
                        return Err(conflict(
                            pattern,
                            id,
                            ClaimKind::Read,
                            other,
                            ClaimKind::Write,
                        ));
                    }
                }
            }
        }
        for (subtree, region) in &tables.subtrees {
            if !pattern_overlaps_subtree(pattern, subtree) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, &view, other_scope, epoch.id, epoch.clock)
            {
                return Err(conflict(
                    pattern,
                    id,
                    ClaimKind::Read,
                    epoch.id,
                    ClaimKind::Write,
                ));
            }
        }
        let region = tables.patterns.entry(pattern.clone()).or_default();
        region.reads.insert(id, (scope.id, view.own));
        Self::finish_claim(scope, id, &mut tables, view.own);
        Ok(())
    }

    /// Checks and records a whole-subtree claim on `path` by `id` of
    /// `scope` (a recursive remove or a directory rename): the subtree's
    /// own standing write, every path and listing under it, and every
    /// pattern that overlaps it.
    fn claim_subtree(
        &self,
        scope: &Arc<Scope>,
        id: ExecId,
        path: &VfsPath,
    ) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id);
        if let Some(region) = tables.subtrees.get(path)
            && let Some((other_scope, epoch)) = region.write
            && tables.conflicts(scope, &view, other_scope, epoch.id, epoch.clock)
        {
            return Err(conflict(
                path,
                id,
                ClaimKind::Write,
                epoch.id,
                ClaimKind::Write,
            ));
        }
        for (other, region) in &tables.paths {
            if !subtree_covers(path, other) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, &view, other_scope, epoch.id, epoch.clock)
            {
                return Err(conflict(
                    path,
                    id,
                    ClaimKind::Write,
                    epoch.id,
                    ClaimKind::Write,
                ));
            }
            for (&reader, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, &view, other_scope, reader, other_clock) {
                    return Err(conflict(
                        path,
                        id,
                        ClaimKind::Write,
                        reader,
                        ClaimKind::Read,
                    ));
                }
            }
        }
        for (dir, region) in &tables.children {
            if !subtree_covers(path, dir) {
                continue;
            }
            for (&reader, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, &view, other_scope, reader, other_clock) {
                    return Err(conflict(
                        path,
                        id,
                        ClaimKind::Write,
                        reader,
                        ClaimKind::Read,
                    ));
                }
            }
        }
        for (pattern, region) in &tables.patterns {
            if !pattern_overlaps_subtree(pattern, path) {
                continue;
            }
            for (&reader, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, &view, other_scope, reader, other_clock) {
                    return Err(conflict(
                        path,
                        id,
                        ClaimKind::Write,
                        reader,
                        ClaimKind::Read,
                    ));
                }
            }
        }
        let region = tables.subtrees.entry(path.clone()).or_default();
        region.write = Some((
            scope.id,
            Epoch {
                id,
                clock: view.own,
            },
        ));
        Self::finish_claim(scope, id, &mut tables, view.own);
        Ok(())
    }

    /// The shared tail of a successful claim: the identity's clock
    /// advances past its recorded epoch, and a table that has grown
    /// prunes.
    fn finish_claim(scope: &Arc<Scope>, id: ExecId, tables: &mut ClaimsTables, tick: u64) {
        scope.advance(id, tick);
        tables.entries += 1;
        if tables.entries > PRUNE_AT {
            prune(tables);
        }
    }
}

/// The conflict error names the path, both identities, and both claim
/// kinds: the executor maps it to a fatal run error, and the message is
/// the whole diagnosis.
fn conflict(
    path: &VfsPath,
    id: ExecId,
    kind: ClaimKind,
    other: ExecId,
    other_kind: ClaimKind,
) -> VfsError {
    VfsError::Conflict {
        path: path.to_string(),
        detail: format!(
            "{kind} on {path} by {id:?} conflicts with a {other_kind} claim by {other:?}"
        ),
    }
}

/// The parent directory of `path`, or `None` at the namespace root.
fn parent_of(path: &VfsPath) -> Option<VfsPath> {
    let (parent, _) = path.as_str().rsplit_once('/')?;
    if parent.is_empty() {
        None
    } else {
        Some(
            canonicalize_absolute(parent)
                .unwrap_or_else(|err| panic!("a canonical path's parent canonicalizes: {err}")),
        )
    }
}

/// The ancestors a write to `path` may create, nearest first, excluding
/// the namespace root: creating the file may create its parent
/// directories.
fn may_create(path: &VfsPath) -> Vec<VfsPath> {
    let mut ancestors = Vec::new();
    let mut rest = path.as_str();
    while let Some((parent, _)) = rest.rsplit_once('/') {
        if parent.is_empty() {
            break;
        }
        rest = parent;
        ancestors.push(
            canonicalize_absolute(parent)
                .unwrap_or_else(|err| panic!("a canonical path's parent canonicalizes: {err}")),
        );
    }
    ancestors
}

/// Whether `subtree` covers `path`: the subtree is the path itself or
/// everything under it.
fn subtree_covers(subtree: &VfsPath, path: &VfsPath) -> bool {
    path.as_str() == subtree.as_str()
        || path
            .as_str()
            .strip_prefix(subtree.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The literal base of a canonical glob `pattern`: the text before the
/// first wildcard, with any trailing slash dropped. The base and its own
/// ancestors are the directories a matching write may create.
fn pattern_base(pattern: &VfsPath) -> Vec<VfsPath> {
    let Some((prefix, _)) = pattern.as_str().split_once('*') else {
        // No wildcard: the pattern names one path, and a write creating
        // it claims that path's ancestors.
        return may_create(pattern);
    };
    let base = prefix.trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }
    let base = canonicalize_absolute(base)
        .unwrap_or_else(|err| panic!("a canonical pattern's literal prefix canonicalizes: {err}"));
    let mut bases = vec![base.clone()];
    let mut rest = base;
    while let Some((parent, _)) = rest.as_str().rsplit_once('/') {
        if parent.is_empty() {
            break;
        }
        rest = canonicalize_absolute(parent).unwrap_or_else(|err| {
            panic!("a canonical pattern's literal prefix canonicalizes: {err}")
        });
        bases.push(rest.clone());
    }
    bases
}

/// Whether `pattern` matches `path`, as the glob grammar reads it. A
/// pattern whose grammar did not pass validation (a grep filter never
/// validated at this layer) falls back to its literal prefix, which is
/// conservative.
fn pattern_matches_path(pattern: &VfsPath, path: &VfsPath) -> bool {
    if validate_glob_grammar(pattern.as_str()).is_err() {
        return pattern_base(pattern)
            .first()
            .is_some_and(|base| path.as_str().starts_with(base.as_str()));
    }
    let tokens = compile_glob(pattern.as_str().as_bytes());
    matches_tokens(&tokens, path.as_str().as_bytes())
}

/// Whether `pattern` overlaps `subtree`, conservatively by literal
/// prefix: one's literal prefix is a prefix of the other's.
fn pattern_overlaps_subtree(pattern: &VfsPath, subtree: &VfsPath) -> bool {
    let Some((prefix, _)) = pattern.as_str().split_once('*') else {
        return pattern_matches_path(pattern, subtree) || subtree_covers(subtree, pattern);
    };
    let prefix = prefix.trim_end_matches('/');
    let subtree = subtree.as_str();
    prefix == subtree
        || prefix
            .strip_prefix(subtree)
            .is_some_and(|rest| rest.starts_with('/'))
        || subtree
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Prunes claims that can never conflict again: every claim of an ended
/// or dead scope, and - outside the pattern regions, which accumulate for
/// the life of a run - every epoch that happens before every live
/// identity of its scope. Runs under the tables lock.
fn prune(tables: &mut ClaimsTables) {
    prune_dead_scopes(tables);
    prune_ordered_epochs(tables);
    tables.entries = tables.paths.len()
        + tables.children.len()
        + tables.patterns.len()
        + tables.subtrees.len()
        + tables.ancestors.len();
}

/// Drops every claim of an ended or dead scope and the registry entries
/// for those scopes: an ended scope's claims are ignored anyway, so they
/// can go at once.
fn prune_dead_scopes(tables: &mut ClaimsTables) {
    let mut dead: HashSet<ScopeId> = HashSet::new();
    for (&scope_id, weak) in &tables.scopes {
        if weak.upgrade().is_none_or(|scope| scope.ended()) {
            dead.insert(scope_id);
        }
    }
    if dead.is_empty() {
        return;
    }
    let retain = |region: &mut RegionClaims| {
        region.write = region
            .write
            .filter(|(scope_id, _)| !dead.contains(scope_id));
        region
            .reads
            .retain(|_, (scope_id, _)| !dead.contains(scope_id));
        region
            .created
            .retain(|_, (scope_id, _)| !dead.contains(scope_id));
    };
    for region in tables.paths.values_mut() {
        retain(region);
    }
    for region in tables.children.values_mut() {
        retain(region);
    }
    for region in tables.patterns.values_mut() {
        retain(region);
    }
    for region in tables.subtrees.values_mut() {
        retain(region);
    }
    for region in tables.ancestors.values_mut() {
        retain(region);
    }
    tables.scopes.retain(|scope_id, _| !dead.contains(scope_id));
}

/// Drops every epoch that happens before every live identity of its
/// scope: no future access can race with it. Pattern regions are exempt:
/// they accumulate for the life of a run.
fn prune_ordered_epochs(tables: &mut ClaimsTables) {
    // One view per live scope of its live identities' clocks, then
    // each epoch checked against it.
    let mut live_views: HashMap<ScopeId, Vec<View>> = HashMap::new();
    for (&scope_id, weak) in &tables.scopes {
        let Some(scope) = weak.upgrade() else {
            continue;
        };
        if scope.ended() {
            continue;
        }
        let inner = scope.lock();
        let views: Vec<View> = inner
            .identities
            .iter()
            .filter(|(_, identity)| identity.refs > 0)
            .map(|(&id, identity)| View {
                id,
                own: identity.own,
                seen: Arc::clone(&identity.seen),
                forked: identity.forked.clone(),
            })
            .collect();
        if !views.is_empty() {
            live_views.insert(scope_id, views);
        }
    }
    let ordered_before_everyone = |scope_id: ScopeId, epoch: Epoch| {
        live_views.get(&scope_id).is_some_and(|views| {
            views
                .iter()
                .all(|view| epoch.clock <= view.seen_clock(epoch.id))
        })
    };
    let prune_region = |region: &mut RegionClaims| {
        region.write = region
            .write
            .filter(|(scope_id, epoch)| !ordered_before_everyone(*scope_id, *epoch));
        region.reads.retain(|&other, &mut (scope_id, other_clock)| {
            !ordered_before_everyone(
                scope_id,
                Epoch {
                    id: other,
                    clock: other_clock,
                },
            )
        });
        region
            .created
            .retain(|&other, &mut (scope_id, other_clock)| {
                !ordered_before_everyone(
                    scope_id,
                    Epoch {
                        id: other,
                        clock: other_clock,
                    },
                )
            });
    };
    for region in tables.paths.values_mut() {
        prune_region(region);
    }
    for region in tables.children.values_mut() {
        prune_region(region);
    }
    for region in tables.subtrees.values_mut() {
        prune_region(region);
    }
    for region in tables.ancestors.values_mut() {
        prune_region(region);
    }
}

/// Re-spells a namespace-absolute result relative to the access's root,
/// or `None` when the result does not sit under it (never, for results
/// of a pattern that joined onto the root).
fn strip_root(root: &str, matched: &str) -> Option<String> {
    if root == "/" {
        return matched.strip_prefix('/').map(str::to_owned);
    }
    let prefix = format!("{root}/");
    matched.strip_prefix(&prefix).map(str::to_owned)
}

/// The largest logical store path, in bytes, accepted by the store
/// view: the ceiling the retired store facade enforced
/// (`MAX_STORE_PATH_BYTES`), kept so a store path stays a bounded
/// denial-of-service lever.
const MAX_STORE_PATH_BYTES: usize = 1024;

/// The store's strict logical-path rules, applied by the store view
/// before a path reaches canonicalization, in the store contract's
/// check order: the first rule broken is the one reported, with the
/// path exactly as supplied.
fn validate_store_path(raw: &str) -> Result<(), VfsError> {
    let reject = |reason| {
        Err(VfsError::InvalidPath {
            path: raw.to_owned(),
            reason,
        })
    };
    if raw.is_empty() {
        return reject(PathReason::Empty);
    }
    if raw.len() > MAX_STORE_PATH_BYTES {
        return reject(PathReason::TooLong);
    }
    if raw.starts_with('/') {
        return reject(PathReason::Absolute);
    }
    if raw.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return reject(PathReason::Control);
    }
    // A backslash is a separator on some backends and a literal on
    // others; refuse it so a canonical `/`-separated path cannot be
    // reinterpreted.
    if raw.contains('\\') {
        return reject(PathReason::Backslash);
    }
    for segment in raw.split('/') {
        if segment.is_empty() {
            return reject(PathReason::EmptySegment);
        }
        if segment == "." || segment == ".." {
            return reject(PathReason::Traversal);
        }
        // Trailing `.`/space are stripped by some backends, so the
        // stored name would not round-trip.
        if segment.ends_with('.') || segment.ends_with(' ') {
            return reject(PathReason::UnsafeSuffix);
        }
        if is_reserved_device_name(segment) {
            return reject(PathReason::ReservedName);
        }
    }
    Ok(())
}

/// Whether `segment` is a platform-reserved device name. Windows
/// treats names like `CON`, `NUL`, `COM1`, and `LPT1` as devices even
/// with an extension (`con.txt`), so the base name before the first
/// `.` is checked case-insensitively.
fn is_reserved_device_name(segment: &str) -> bool {
    let base = segment.split('.').next().unwrap_or(segment);
    let upper = base.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || is_numbered_device(&upper, "COM")
        || is_numbered_device(&upper, "LPT")
}

/// Whether `name` is `<prefix>N` for a single digit `1..=9`.
fn is_numbered_device(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.parse::<u8>().ok().filter(|_| rest.len() == 1))
        .is_some_and(|n| (1..=9).contains(&n))
}

/// Re-spells an error's paths for a store view's caller: a canonical
/// path under the store root and a mount-relative backend path both
/// become the logical form the caller supplied, and prose fields that
/// embed the path (a conflict's diagnosis, a denial's reason) follow.
fn relativize_error(err: VfsError, root: &str) -> VfsError {
    let logical = |path: &str| {
        strip_root(root, path).unwrap_or_else(|| path.trim_start_matches('/').to_owned())
    };
    match err {
        VfsError::NotFound { path } => VfsError::NotFound {
            path: logical(&path),
        },
        VfsError::AlreadyExists { path } => VfsError::AlreadyExists {
            path: logical(&path),
        },
        VfsError::NotADirectory { path } => VfsError::NotADirectory {
            path: logical(&path),
        },
        VfsError::IsADirectory { path } => VfsError::IsADirectory {
            path: logical(&path),
        },
        VfsError::DirectoryNotEmpty { path } => VfsError::DirectoryNotEmpty {
            path: logical(&path),
        },
        VfsError::NotUtf8 { path } => VfsError::NotUtf8 {
            path: logical(&path),
        },
        VfsError::InvalidPath { path, reason } => VfsError::InvalidPath {
            path: logical(&path),
            reason,
        },
        VfsError::InvalidRange { path, reason } => VfsError::InvalidRange {
            path: logical(&path),
            reason,
        },
        VfsError::Anchor {
            path,
            anchor,
            count,
        } => VfsError::Anchor {
            path: logical(&path),
            anchor,
            count,
        },
        VfsError::PermissionDenied { path, reason } => VfsError::PermissionDenied {
            reason: reason.replace(&path, &logical(&path)),
            path: logical(&path),
        },
        VfsError::Unsupported { path, detail } => VfsError::Unsupported {
            detail: detail.replace(&path, &logical(&path)),
            path: logical(&path),
        },
        VfsError::Conflict { path, detail } => VfsError::Conflict {
            detail: detail.replace(&path, &logical(&path)),
            path: logical(&path),
        },
        VfsError::Backend { message } => VfsError::Backend { message },
    }
}

/// One mounted filesystem instance: its backend and the ledger of who is
/// touching what. The two are separately `Arc`-shareable so `overlay()`
/// (a later step) can share the claims table while swapping the backend.
struct Volume {
    backend: Arc<Mutex<Box<dyn Vfs>>>,
    claims: Arc<Claims>,
    sink: Option<OpSink>,
    /// The declared store, when the builder that built this handle
    /// declared one. The store view derives from it; a mounted
    /// handle's declaration stays invisible behind its backend.
    store: Option<StoreDecl>,
}

/// The cloneable handle over one backend and its claims ledger.
///
/// Clones share the backend, the claims tables, and the policy: claims
/// registered through one clone conflict with operations through another.
#[derive(Clone)]
pub struct VfsRef {
    volume: Arc<Volume>,
    policy: Arc<dyn Policy + Sync>,
}

impl fmt::Debug for VfsRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VfsRef").finish_non_exhaustive()
    }
}

impl VfsRef {
    /// Returns a handle over `backend` with the [`AllowAll`] policy.
    #[must_use]
    pub fn new(backend: impl Vfs + 'static) -> VfsRef {
        Self::with_policy(backend, AllowAll)
    }

    /// Returns a handle over `backend` consulting `policy` on every
    /// operation. The policy is dynamic through shared state: the host
    /// holds the same `Arc` and changes behavior mid-run, and the next
    /// operation sees it.
    #[must_use]
    pub fn with_policy(
        backend: impl Vfs + 'static,
        policy: impl Policy + Sync + 'static,
    ) -> VfsRef {
        VfsRef {
            volume: Arc::new(Volume {
                backend: Arc::new(Mutex::new(Box::new(backend))),
                claims: Arc::new(Claims::new()),
                sink: None,
                store: None,
            }),
            policy: Arc::new(policy),
        }
    }

    /// Returns a builder for installing mounts. Mounts are fixed at
    /// [`VfsRefBuilder::build`], so the table is immutable and cheap to
    /// `Arc`-share thereafter.
    #[must_use]
    pub fn builder() -> VfsRefBuilder {
        VfsRefBuilder::new()
    }

    /// Returns a handle with `backend` mounted at `prefix` over this
    /// handle's namespace. The claims table is shared: conflicts are
    /// detected across both views of the same storage. The overlay
    /// inherits this handle's store declaration, when it has one.
    ///
    /// # Panics
    /// Panics when `prefix` is not an absolute virtual path or is the
    /// root: an overlay at `/` would replace the base entirely, so use
    /// [`VfsRef::new`] instead.
    #[must_use]
    pub fn overlay(&self, prefix: &str, backend: impl Vfs + 'static) -> VfsRef {
        let canonical = canonicalize_absolute(prefix)
            .unwrap_or_else(|err| panic!("invalid overlay prefix {prefix:?}: {err}"));
        assert!(
            canonical.as_str() != "/",
            "an overlay at / would replace the base entirely; use VfsRef::new instead"
        );
        // The base handle mounts at the root of the overlay's router:
        // operations outside the overlay prefix route through the base's
        // own policy and claims under the caller's identity.
        let mut mounts = Mounts::new();
        let root = canonicalize_absolute("/")
            .unwrap_or_else(|err| panic!("the namespace root is always valid: {err}"))
            .to_buf();
        mounts.insert(root, Arc::new(Mutex::new(Box::new(self.clone()))));
        mounts.insert(canonical.to_buf(), Arc::new(Mutex::new(Box::new(backend))));
        VfsRef {
            volume: Arc::new(Volume {
                backend: Arc::new(Mutex::new(Box::new(Router::new(mounts)))),
                claims: Arc::clone(&self.volume.claims),
                // One sink observes both views of the same storage, fired
                // by the outer capability with the caller's origin.
                sink: self.volume.sink.clone(),
                store: self.volume.store.clone(),
            }),
            policy: Arc::clone(&self.policy),
        }
    }

    /// Acquires the capability for a new serial thread of execution.
    /// This is the only way in: every acquire vends a fresh [`ExecId`]
    /// and starts a new *scope* - the root identity together with every
    /// identity later forked from it. Two acquires are two scopes, and
    /// nothing orders two scopes, so their claims always conflict while
    /// both live; a scope's claims are ignored once its last identity
    /// ends. `origin` is pure observability: it labels every operation
    /// event this capability fires and never gates anything.
    ///
    /// # Errors
    /// Returns an error when the backend refuses to acquire the identity.
    pub fn acquire(&self, origin: Origin) -> Result<Access, VfsError> {
        self.acquire_with(
            &AcquireContext::new(ExecId::vend(), Scope::start()),
            Some(origin),
        )
    }

    /// Acquires the capability for the identity and scope in `cx`: a
    /// fresh acquire passes a new scope, and a mounted handle receives
    /// the caller's context, so the forwarded capability joins the
    /// caller's scope and its claims conflict across both views of the
    /// same storage. A `None` origin is the mount forward: the outer
    /// handle already fired the caller's origin, so the forward fires
    /// nothing rather than double the event with a fabricated, less
    /// precise one.
    ///
    /// # Errors
    /// Returns an error when the backend refuses to acquire the identity;
    /// see [`VfsRef::acquire`].
    pub(crate) fn acquire_with(
        &self,
        cx: &AcquireContext,
        origin: Option<Origin>,
    ) -> Result<Access, VfsError> {
        let inner = self.backend().acquire(cx)?;
        Ok(Access {
            id: cx.id(),
            origin,
            root: VfsPath::root(),
            store_root: None,
            volume: self.volume.clone(),
            policy: self.policy.clone(),
            scope: self.join_scope(cx),
            inner: Mutex::new(inner),
        })
    }

    /// Joins the scope in `cx`: registers it in this handle's claims
    /// tables and attaches the identity, fresh or forwarded.
    fn join_scope(&self, cx: &AcquireContext) -> Arc<Scope> {
        let scope = Arc::clone(cx.scope());
        self.volume.claims.register_scope(&scope);
        scope.attach(cx.id());
        scope
    }

    /// Builds a handle over a router with a fresh claims table and the
    /// installed policy, op sink, and store declaration: the builder's
    /// exit.
    pub(crate) fn from_router(
        router: Router,
        policy: Arc<dyn Policy + Sync>,
        sink: Option<OpSink>,
        store: Option<StoreDecl>,
    ) -> VfsRef {
        VfsRef {
            volume: Arc::new(Volume {
                backend: Arc::new(Mutex::new(Box::new(router))),
                claims: Arc::new(Claims::new()),
                sink,
                store,
            }),
            policy,
        }
    }

    /// Poison-safe lock on the backend.
    fn backend(&self) -> MutexGuard<'_, Box<dyn Vfs>> {
        self.volume
            .backend
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The public capability. Holds an [`ExecId`] and the backend's access
/// object; every operation canonicalizes the path, checks the policy,
/// checks the claims tables, fires the op sink, then locks the backend
/// per call. Dropping the capability drops one reference to its identity;
/// the identity - and with it, the scope - ends when its last access
/// drops.
#[must_use = "an acquire dropped immediately is a bug: the capability holds its identity's claims"]
pub struct Access {
    id: ExecId,
    /// The caller-supplied observability origin; `None` only on the
    /// crate-private mount forward, which never fires.
    origin: Option<Origin>,
    /// The root that relative paths and patterns join onto, fixed for
    /// the access's life. A plain acquire roots at `/`.
    root: VfsPath,
    /// The declared store root, when this access is a store view:
    /// relative paths join onto it, the strict store-path rules gate
    /// every path, and errors come back in the caller's logical form.
    /// Plain accesses have `None`.
    store_root: Option<VfsPath>,
    volume: Arc<Volume>,
    policy: Arc<dyn Policy + Sync>,
    /// The scope this identity belongs to: its vector clock and its
    /// reference count, shared with every access of the scope.
    scope: Arc<Scope>,
    inner: Mutex<Box<dyn VfsAccess>>,
}

impl Access {
    /// Returns the capability for a new concurrent thread of execution.
    /// The spawn is the fork: the child gets a fresh [`ExecId`] and a
    /// shared snapshot of this capability's clock plus its own entry,
    /// while this capability's entry advances, so its later accesses are
    /// not ordered before the child's. [`crate::detail::access_join`] is
    /// the matching join. `origin` labels the child's operation events,
    /// as in [`VfsRef::acquire`].
    ///
    /// # Errors
    /// Returns an error when the backend refuses to acquire the child's
    /// identity; see [`VfsRef::acquire`]. A failed spawn still advances
    /// this capability's own clock entry, which orders nothing new: its
    /// later accesses are merely no longer ordered before a child that
    /// never ran.
    ///
    /// Crate-internal: backs [`crate::detail::access_spawn`].
    pub(crate) fn spawn(&self, origin: Origin) -> Result<Access, VfsError> {
        let id = ExecId::vend();
        // The fork comes first so a wrapped handle acquiring the child
        // finds it already registered in the scope.
        self.scope.fork(self.id, id);
        let inner = match self
            .backend()
            .acquire(&AcquireContext::new(id, Arc::clone(&self.scope)))
        {
            Ok(inner) => inner,
            Err(err) => {
                self.scope.release(id);
                return Err(err);
            }
        };
        // A store view's arms keep the view's confinement to the store
        // mount and its logical error paths.
        let inner: Box<dyn VfsAccess> = match &self.store_root {
            Some(root) => Box::new(StoreScoped {
                root: root.as_str().to_owned(),
                inner,
            }),
            None => inner,
        };
        Ok(Access {
            id,
            origin: Some(origin),
            // The child shares the parent's root: a store view's arms
            // see the same store.
            root: self.root.clone(),
            store_root: self.store_root.clone(),
            volume: self.volume.clone(),
            policy: self.policy.clone(),
            scope: Arc::clone(&self.scope),
            inner: Mutex::new(inner),
        })
    }

    /// The identity every operation through this capability is
    /// attributed to. Crate-internal: the engine reads it through
    /// [`crate::detail::access_id`].
    pub(crate) const fn exec_id(&self) -> ExecId {
        self.id
    }

    /// Crate-internal: backs [`crate::detail::access_join`].
    pub(crate) fn join(&self, child: ExecId) {
        self.scope.join(self.id, child);
    }

    /// The claims-table half of one operation: runs `claim` against
    /// this access's identity and scope, and re-spells a store view's
    /// error paths into the caller's logical form.
    fn admit(
        &self,
        claim: impl FnOnce(&Claims, &Arc<Scope>, ExecId, &VfsPath) -> Result<(), VfsError>,
        path: &VfsPath,
    ) -> Result<(), VfsError> {
        claim(&self.volume.claims, &self.scope, self.id, path).map_err(|err| self.relativize(err))
    }

    /// Re-spells an error's paths for the store view's caller; a plain
    /// access's errors pass through untouched.
    fn relativize(&self, err: VfsError) -> VfsError {
        match &self.store_root {
            Some(root) => relativize_error(err, root.as_str()),
            None => err,
        }
    }

    /// The store view: an access rooted at the declared store root
    /// whose operations reach the store's own mount alone, sharing
    /// this access's identity, scope, claims table, policy, and op
    /// sink. The view applies the store's strict logical-path rules
    /// and reports error paths in the caller's logical form.
    /// Crate-internal: backs [`crate::detail::store_view`].
    ///
    /// # Errors
    /// Returns an error when the handle declares no store.
    pub(crate) fn store_view(&self) -> Result<Access, VfsError> {
        let Some(store) = &self.volume.store else {
            return Err(VfsError::Unsupported {
                path: self.root.to_string(),
                detail: "the handle declares no store".to_owned(),
            });
        };
        // One mount - the store's own - behind a fresh router:
        // operations are confined to the store mount, so a store at
        // `/` never reaches a host directory mounted beneath it.
        let mut mounts = Mounts::new();
        mounts.insert(store.root.to_buf(), Arc::clone(&store.mount));
        let mut router = Router::new(mounts);
        let inner = router.acquire(&AcquireContext::new(self.id, Arc::clone(&self.scope)))?;
        // The view holds one more reference to the identity, like a
        // mount forward: the identity - and its scope - ends with its
        // last access, the view included.
        self.scope.attach(self.id);
        Ok(Access {
            id: self.id,
            // The view keeps the chain's origin: store operations fire
            // the op sink under the chain's label.
            origin: self.origin.clone(),
            root: store.root.clone(),
            store_root: Some(store.root.clone()),
            volume: Arc::new(Volume {
                backend: Arc::new(Mutex::new(Box::new(router))),
                claims: Arc::clone(&self.volume.claims),
                sink: self.volume.sink.clone(),
                store: None,
            }),
            policy: Arc::clone(&self.policy),
            scope: Arc::clone(&self.scope),
            inner: Mutex::new(Box::new(StoreScoped {
                root: store.root.as_str().to_owned(),
                inner,
            })),
        })
    }

    /// Reads the file at `path` as stored.
    ///
    /// # Errors
    /// Returns an error when the policy denies the read, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let path = self.gate(Op::Read, path)?;
        self.admit(Claims::claim_read, &path)?;
        self.fire(Op::Read, &path);
        self.inner().read(&path)
    }

    /// Reads the file at `path` as UTF-8 text.
    ///
    /// # Errors
    /// Returns an error when the file's contents are not UTF-8.
    pub fn read_string(&self, path: &str) -> Result<String, VfsError> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes).map_err(|_| VfsError::NotUtf8 {
            path: path.to_owned(),
        })
    }

    /// Reads lines `start..=end` of the file at `path`, 1-based and
    /// inclusive, joined by `"\n"` with no trailing newline. An omitted
    /// `end` means the last line; a given `end` clamps down to it; a
    /// `start` past the last line reads as the empty string.
    ///
    /// # Errors
    /// Returns an error when `start` is below 1 or `end` is before
    /// `start`, when the file is missing, or when it is not UTF-8.
    pub fn read_range(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, VfsError> {
        self.with_line_range(path, start, end, |lines, _| lines.join("\n"))
    }

    /// Reads lines `start..=end` as numbered lines, numbered absolutely
    /// from `start`, each right-aligned to the width of the largest
    /// emitted number and followed by `"| "`. Bounds behave as in
    /// [`Access::read_range`].
    ///
    /// # Errors
    /// Returns an error under the same conditions as
    /// [`Access::read_range`].
    pub fn read_range_numbered(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, VfsError> {
        self.with_line_range(path, start, end, |lines, first| {
            let width = (first + lines.len() - 1).to_string().len();
            lines
                .iter()
                .enumerate()
                .map(|(index, line)| format!("{:>width$}| {}", first + index, line))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// Creates or overwrites the file at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the write, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn write(&self, path: &str, contents: &[u8]) -> Result<(), VfsError> {
        let path = self.gate(Op::Write, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Write, &path);
        self.inner().write(&path, contents)
    }

    /// Appends to the file at `path`, creating it if absent.
    ///
    /// # Errors
    /// Returns an error when the policy denies the append, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn append(&self, path: &str, contents: &[u8]) -> Result<(), VfsError> {
        let path = self.gate(Op::Append, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Append, &path);
        self.inner().append(&path, contents)
    }

    /// Replaces the unique occurrence of `old` with `new` in the file at
    /// `path`. An empty `old` is refused. Zero matches and multiple
    /// matches are both errors.
    ///
    /// # Errors
    /// Returns an error when `old` is empty, when the policy denies the
    /// write, when an access unordered with this one holds a conflicting
    /// claim on `path`, when the match count is not exactly one, or when
    /// the backend fails.
    pub fn str_replace(&self, path: &str, old: &str, new: &str) -> Result<(), VfsError> {
        // The store view validates the path before the anchor, in the
        // store contract's order.
        if self.store_root.is_some() {
            validate_store_path(path)?;
        }
        if old.is_empty() {
            return Err(VfsError::Anchor {
                path: path.to_owned(),
                anchor: String::new(),
                count: 0,
            });
        }
        let path = self.gate(Op::Write, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Write, &path);
        self.inner().str_replace(&path, old, new)
    }

    /// Removes the file, link, or directory at `path`.
    ///
    /// A confirmed removal is `Ok(true)`, and a missing path is
    /// `Ok(false)`: deleting is idempotent.
    ///
    /// # Errors
    /// Returns an error when the policy denies the delete, when an access
    /// unordered with this one holds a conflicting claim - a recursive
    /// removal claims the whole subtree - or when the backend fails.
    pub fn remove(&self, path: &str, recursive: bool) -> Result<bool, VfsError> {
        let path = self.gate(Op::Delete, path)?;
        if recursive {
            self.admit(Claims::claim_subtree, &path)?;
        } else {
            self.admit(Claims::claim_write, &path)?;
        }
        self.fire(Op::Delete, &path);
        match self.inner().remove(&path, recursive) {
            Ok(()) => Ok(true),
            // The backend trait reports an absent path as NotFound; the
            // public capability confirms the absence instead.
            Err(VfsError::NotFound { .. }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// A confirmed absence is `Ok(false)`; a backend failure is `Err`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the check, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn exists(&self, path: &str) -> Result<bool, VfsError> {
        let path = self.gate(Op::Exists, path)?;
        self.admit(Claims::claim_read, &path)?;
        self.fire(Op::Exists, &path);
        self.inner().exists(&path)
    }

    /// Returns the paths matching `pattern` that are files, or only
    /// directories when the pattern ends in `/`, sorted.
    ///
    /// The raw pattern is validated before canonicalization, so a
    /// backslash or a control character is refused rather than treated
    /// as a pattern byte, and a backslash is never turned into a
    /// separator. A pattern without a leading `/` joins onto the
    /// access's root, and its results come back relative to that root.
    ///
    /// # Errors
    /// Returns an error when the pattern is empty, over-long,
    /// control-bearing, backslash-bearing, or grammar-invalid, when a
    /// store view's strict path rules refuse it, when the policy denies
    /// the glob, when an access unordered with this one holds a
    /// conflicting claim (the pattern is the claim, not each match), or
    /// when the backend fails. Each malformed
    /// pattern reports the rule it broke as a [`PathReason`] in the
    /// [`VfsError::InvalidPath`].
    pub fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        if pattern.is_empty() {
            return Err(VfsError::InvalidPath {
                path: pattern.to_owned(),
                reason: PathReason::Empty,
            });
        }
        if let Err(reason) = validate_glob_pattern(pattern) {
            return Err(VfsError::InvalidPath {
                path: pattern.to_owned(),
                reason,
            });
        }
        // A trailing `/` asks for directories only; matching and the
        // claim key use the pattern without it.
        let dirs_only = pattern.ends_with('/');
        let stripped = pattern.trim_end_matches('/');
        let pattern = if stripped.is_empty() { "/" } else { stripped };
        self.glob_pattern(pattern, dirs_only)
    }

    /// The canonical-pattern half of [`Access::glob`]: gates, claims,
    /// fires, and matches one pattern whose trailing `/` was already
    /// split off into `dirs_only`. Crate-private so the mounted-handle
    /// forward can preserve the flag it received.
    fn glob_pattern(&self, pattern: &str, dirs_only: bool) -> Result<Vec<String>, VfsError> {
        let relative = !pattern.starts_with('/');
        // The claim key is the canonicalized pattern; the backend
        // receives it canonical too.
        let claimed = self.gate(Op::Glob, pattern)?;
        self.admit(Claims::claim_glob, &claimed)?;
        self.fire(Op::Glob, &claimed);
        let mut matches = self.inner().glob_kind(claimed.as_str(), dirs_only)?;
        if relative {
            for matched in &mut matches {
                if let Some(rest) = strip_root(self.root.as_str(), matched) {
                    *matched = rest;
                }
            }
        }
        Ok(matches)
    }

    /// Lists the directory at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the list, when an access
    /// unordered with this one holds a conflicting claim on the
    /// directory's children, or when the backend fails.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, VfsError> {
        let path = self.gate(Op::List, path)?;
        self.admit(Claims::claim_list, &path)?;
        self.fire(Op::List, &path);
        self.inner().list(&path)
    }

    /// Returns metadata for `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the stat, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn stat(&self, path: &str) -> Result<Stat, VfsError> {
        let path = self.gate(Op::Stat, path)?;
        self.admit(Claims::claim_read, &path)?;
        self.fire(Op::Stat, &path);
        self.inner().stat(&path)
    }

    /// Creates the directory at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the mkdir, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn mkdir(&self, path: &str, recursive: bool) -> Result<(), VfsError> {
        let path = self.gate(Op::Mkdir, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Mkdir, &path);
        self.inner().mkdir(&path, recursive)
    }

    /// Renames or moves, atomically where the backend allows. The source
    /// is claimed as the whole subtree it moves, the destination as a
    /// write.
    ///
    /// # Errors
    /// Returns an error when the policy denies the rename, when an access
    /// unordered with this one holds a conflicting claim on either path,
    /// or when the backend fails.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let from = self.gate(Op::Rename, from)?;
        let to = self.gate(Op::Rename, to)?;
        // Both paths gated, so the operation is admitted: the source
        // subtree and the destination are claimed, then one event per
        // canonical path.
        self.admit(Claims::claim_subtree, &from)?;
        self.admit(Claims::claim_write, &to)?;
        self.fire(Op::Rename, &from);
        self.fire(Op::Rename, &to);
        self.inner().rename(&from, &to)
    }

    /// Copies the file at `from` to `to`. The source is claimed as a
    /// read, the destination as a write.
    ///
    /// # Errors
    /// Returns an error when the policy denies the copy, when an access
    /// unordered with this one holds a conflicting claim on either path,
    /// or when the backend fails.
    pub fn copy(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let from = self.gate(Op::Copy, from)?;
        let to = self.gate(Op::Copy, to)?;
        self.admit(Claims::claim_read, &from)?;
        self.admit(Claims::claim_write, &to)?;
        self.fire(Op::Copy, &from);
        self.fire(Op::Copy, &to);
        self.inner().copy(&from, &to)
    }

    /// Searches files under the query's root.
    ///
    /// # Errors
    /// Returns an error when a store view's strict path rules refuse
    /// the root, when the policy denies the search, when an access
    /// unordered with this one holds a conflicting claim on what the
    /// search observes (the root and the filter, as a pattern), or when
    /// the backend fails.
    pub fn grep(&self, query: &GrepQuery) -> Result<GrepResults, VfsError> {
        // The store view runs the strict rules on the root before
        // canonicalization: an absolute root is the one shape that
        // would canonicalize namespace-absolute and carry the grep's
        // claims outside the store mount.
        if self.store_root.is_some() {
            validate_store_path(query.root.as_str())?;
        }
        let root = canonicalize(&self.root, query.root.as_str())?;
        self.check_policy(Op::Grep, &root)
            .map_err(|err| self.relativize(err))?;
        // The search observes every file its glob pattern covers: the
        // root and the filter, as the default body composes them.
        let base = match root.as_str() {
            "/" => "",
            root => root,
        };
        let pattern = match &query.glob_filter {
            Some(filter) => format!("{base}/**/{filter}"),
            None => format!("{base}/**/*"),
        };
        match canonicalize(&self.root, &pattern) {
            Ok(pattern) => self.admit(Claims::claim_glob, &pattern)?,
            // A filter the canonicalizer cannot hold (a traversal, for
            // example) falls back to the root alone, conservatively.
            Err(_) => self.admit(Claims::claim_read, &root)?,
        }
        self.fire(Op::Grep, &root);
        self.inner().grep(query)
    }

    /// Canonicalizes at receipt and consults the policy - in that order,
    /// so a denied operation never registers a claim and every claim key
    /// is the canonical path. A path without a leading `/` joins onto the
    /// access's root. The store view applies the store's strict
    /// logical-path rules before canonicalization, and re-spells its
    /// policy denials into the caller's logical form.
    fn gate(&self, op: Op, path: &str) -> Result<VfsPath, VfsError> {
        // The strict rules run on every caller-supplied path, glob
        // patterns included: a leading `/` is the one pattern shape
        // that would canonicalize namespace-absolute and carry the
        // claim outside the store mount. A pattern's wildcard grammar
        // is validated separately, in `Access::glob`, before the gate.
        if self.store_root.is_some() {
            validate_store_path(path)?;
        }
        let path = canonicalize(&self.root, path)?;
        self.check_policy(op, &path)
            .map_err(|err| self.relativize(err))?;
        Ok(path)
    }

    /// Consults the handle's policy. v1 maps `Ask` to `PermissionDenied`:
    /// the approval dialog is a host concern above this layer, and the
    /// reason string still names what was asked and which rule fired.
    fn check_policy(&self, op: Op, path: &VfsPath) -> Result<(), VfsError> {
        match self.policy.check(op, path) {
            Verdict::Allow => Ok(()),
            Verdict::Deny(reason) | Verdict::Ask(reason) => Err(VfsError::PermissionDenied {
                path: path.to_string(),
                reason,
            }),
        }
    }

    /// Fires the installed sink for one admitted path of `op` -
    /// fire-and-forget, after policy and claims pass, before the backend
    /// executes. A handle without a sink, or a capability vended to a
    /// mounted-handle forward (the outer handle already fired with the
    /// caller's origin), fires nothing.
    fn fire(&self, op: Op, path: &VfsPath) {
        let (Some(sink), Some(origin)) = (&self.volume.sink, &self.origin) else {
            return;
        };
        sink(OpEvent { op, path, origin });
    }

    /// Reads the file and resolves one line range while its contents
    /// remain live.
    fn with_line_range(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
        render: impl FnOnce(&[&str], usize) -> String,
    ) -> Result<String, VfsError> {
        // The store view validates the path before the bounds, in the
        // store contract's order.
        if self.store_root.is_some() {
            validate_store_path(path)?;
        }
        if start < 1 {
            return Err(VfsError::InvalidRange {
                path: path.to_owned(),
                reason: "start is below 1",
            });
        }
        let contents = self.read_string(path)?;
        let lines: Vec<&str> = contents.lines().collect();
        if start > lines.len() {
            return Ok(String::new());
        }
        let end = end.unwrap_or(lines.len()).min(lines.len());
        if end < start {
            return Err(VfsError::InvalidRange {
                path: path.to_owned(),
                reason: "end is before start",
            });
        }
        Ok(render(&lines[start - 1..end], start))
    }

    /// Poison-safe lock on the backend's access object, held per call.
    fn inner(&self) -> MutexGuard<'_, Box<dyn VfsAccess>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Poison-safe lock on the backend.
    fn backend(&self) -> MutexGuard<'_, Box<dyn Vfs>> {
        self.volume
            .backend
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Access")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for Access {
    fn drop(&mut self) {
        // Drops one reference to the identity; the identity ends - and,
        // when it was the last, the scope ends with it - at zero. Its
        // claims are never released: an ended scope's claims are ignored
        // and purged lazily.
        self.scope.release(self.id);
        // Releasing the identity at the backend is best-effort: the
        // happens-before state is already handled, so a backend failure
        // here cannot leave a conflict behind.
        let _ = self.backend().release(self.id);
    }
}

/// A handle is itself a backend: mounting a base handle under a child
/// router - which is how [`VfsRef::overlay`] shares one claims table
/// across two views of the same storage - routes operations through the
/// base's policy and claims under the caller's identity.
impl Vfs for VfsRef {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        // The mount forward leaves the origin unset: the outer handle already
        // fired the caller's, and a fabricated one here would double the
        // event with a less precise label.
        Ok(Box::new(HandleAccess(self.acquire_with(cx, None)?)))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        // The vended session's Drop drops the identity's reference;
        // nothing is registered at this level.
        let _ = id;
        Ok(())
    }

    fn read_only(&self) -> bool {
        self.backend().read_only()
    }
}

/// The session vended by a mounted handle: forwards every operation
/// through the base handle's capability, so its policy and claims apply
/// under the caller's identity. Paths arrive canonical, so the
/// capability's canonicalization at receipt is an idempotent re-check.
///
/// Byte-range reads and the POSIX extras keep their trait defaults: the
/// public capability exposes neither, so there is nothing to forward to.
struct HandleAccess(Access);

impl VfsAccess for HandleAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.0.read(path.as_str())
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.0.write(path.as_str(), contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.0.append(path.as_str(), contents)
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        // The backend trait's contract reports an absent path as
        // NotFound; the capability's Ok(false) maps back onto it.
        if self.0.remove(path.as_str(), recursive)? {
            Ok(())
        } else {
            Err(VfsError::NotFound {
                path: path.to_string(),
            })
        }
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.0.exists(path.as_str())
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.0.glob(pattern)
    }

    fn glob_kind(&self, pattern: &str, dirs_only: bool) -> Result<Vec<String>, VfsError> {
        // The forward re-enters the full public pipeline with the flag
        // intact, so the dirs-only split survives the handle boundary.
        self.0.glob_pattern(pattern, dirs_only)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.0.list(path.as_str())
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.0.stat(path.as_str())
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.0.mkdir(path.as_str(), recursive)
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.rename(from.as_str(), to.as_str())
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.copy(from.as_str(), to.as_str())
    }

    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str) -> Result<(), VfsError> {
        self.0.str_replace(path.as_str(), old, new)
    }

    fn grep(&self, query: &GrepQuery) -> Result<GrepResults, VfsError> {
        self.0.grep(query)
    }
}

/// The store view's backend session: the store's own mount behind a
/// one-mount router, whose error paths come back mount-relative and
/// are re-spelled into the caller's logical form.
struct StoreScoped {
    /// The declared store root, for re-spelling error paths.
    root: String,
    inner: Box<dyn VfsAccess>,
}

impl StoreScoped {
    /// Re-spells one error into the caller's logical form.
    fn logical(&self, err: VfsError) -> VfsError {
        relativize_error(err, &self.root)
    }
}

impl VfsAccess for StoreScoped {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.inner.read(path).map_err(|err| self.logical(err))
    }

    fn read_range(&self, path: &VfsPath, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        self.inner
            .read_range(path, offset, len)
            .map_err(|err| self.logical(err))
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.inner
            .write(path, contents)
            .map_err(|err| self.logical(err))
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.inner
            .append(path, contents)
            .map_err(|err| self.logical(err))
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.inner
            .remove(path, recursive)
            .map_err(|err| self.logical(err))
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.inner.exists(path).map_err(|err| self.logical(err))
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.inner.glob(pattern).map_err(|err| self.logical(err))
    }

    fn glob_kind(&self, pattern: &str, dirs_only: bool) -> Result<Vec<String>, VfsError> {
        self.inner
            .glob_kind(pattern, dirs_only)
            .map_err(|err| self.logical(err))
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.inner.list(path).map_err(|err| self.logical(err))
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.inner.stat(path).map_err(|err| self.logical(err))
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.inner
            .mkdir(path, recursive)
            .map_err(|err| self.logical(err))
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.rename(from, to).map_err(|err| self.logical(err))
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.copy(from, to).map_err(|err| self.logical(err))
    }

    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str) -> Result<(), VfsError> {
        self.inner
            .str_replace(path, old, new)
            .map_err(|err| self.logical(err))
    }

    fn grep(&self, query: &GrepQuery) -> Result<GrepResults, VfsError> {
        self.inner.grep(query).map_err(|err| self.logical(err))
    }

    fn symlink(&mut self, target: &VfsPath, link: &VfsPath) -> Result<(), VfsError> {
        self.inner
            .symlink(target, link)
            .map_err(|err| self.logical(err))
    }

    fn read_link(&self, path: &VfsPath) -> Result<VfsPathBuf, VfsError> {
        self.inner.read_link(path).map_err(|err| self.logical(err))
    }

    fn chmod(&mut self, path: &VfsPath, mode: u32) -> Result<(), VfsError> {
        self.inner
            .chmod(path, mode)
            .map_err(|err| self.logical(err))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use super::{Access, VfsRef};
    use crate::error::VfsError;
    use crate::observe::{OpEvent, Origin};
    use crate::path::VfsPath;
    use crate::stat::{Entry, FileType, Stat};
    use crate::traits::{AcquireContext, ExecId, Op, Policy, Verdict, Vfs, VfsAccess};

    /// Minimal in-memory backend shared between the `Vfs` and the access
    /// objects it vends. Releases are recorded so tests can observe the
    /// identity lifecycle.
    #[derive(Clone, Default)]
    struct StubFs {
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        released: Arc<Mutex<Vec<ExecId>>>,
    }

    impl StubFs {
        fn seeded(files: &[(&str, &str)]) -> StubFs {
            let stub = StubFs::default();
            for (name, text) in files {
                stub.files()
                    .insert((*name).to_owned(), text.as_bytes().to_vec());
            }
            stub
        }

        fn files(&self) -> MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
            self.files.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn released(&self) -> Vec<ExecId> {
            self.released
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Vfs for StubFs {
        fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
            let _ = cx;
            Ok(Box::new(StubAccess {
                files: Arc::clone(&self.files),
            }))
        }

        fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
            self.released
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(id);
            Ok(())
        }
    }

    struct StubAccess {
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    }

    impl StubAccess {
        fn files(&self) -> MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
            self.files.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl VfsAccess for StubAccess {
        fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
            self.files()
                .get(path.as_str())
                .cloned()
                .ok_or_else(|| VfsError::NotFound {
                    path: path.to_string(),
                })
        }

        fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
            self.files().insert(path.to_string(), contents.to_vec());
            Ok(())
        }

        fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
            self.files()
                .entry(path.to_string())
                .or_default()
                .extend_from_slice(contents);
            Ok(())
        }

        fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
            let _ = recursive;
            self.files()
                .remove(path.as_str())
                .map(|_| ())
                .ok_or_else(|| VfsError::NotFound {
                    path: path.to_string(),
                })
        }

        fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
            Ok(self.files().contains_key(path.as_str()))
        }

        fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
            let prefix = pattern.split('*').next().unwrap_or(pattern);
            Ok(self
                .files()
                .keys()
                .filter(|name| name.starts_with(prefix))
                .cloned()
                .collect())
        }

        fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
            Err(VfsError::Unsupported {
                path: path.to_string(),
                detail: "the stub does not list".into(),
            })
        }

        fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
            // Every stored key is a file; the stub holds no directories.
            let bytes =
                self.files()
                    .get(path.as_str())
                    .cloned()
                    .ok_or_else(|| VfsError::NotFound {
                        path: path.to_string(),
                    })?;
            Ok(Stat {
                file_type: FileType::File,
                size: bytes.len() as u64,
                mode: None,
                modified: None,
                created: None,
            })
        }

        fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
            let _ = (path, recursive);
            Ok(())
        }

        fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
            let bytes = self
                .files()
                .remove(from.as_str())
                .ok_or_else(|| VfsError::NotFound {
                    path: from.to_string(),
                })?;
            self.files().insert(to.to_string(), bytes);
            Ok(())
        }

        fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
            let bytes =
                self.files()
                    .get(from.as_str())
                    .cloned()
                    .ok_or_else(|| VfsError::NotFound {
                        path: from.to_string(),
                    })?;
            self.files().insert(to.to_string(), bytes);
            Ok(())
        }
    }

    fn handle(stub: &StubFs) -> VfsRef {
        VfsRef::new(stub.clone())
    }

    /// The claims tests ignore origins, so they acquire under one blanket
    /// label; the observability tests use specific labels.
    fn test_origin() -> Origin {
        Origin::new("handle test")
    }

    /// A policy whose verdict flips through shared state mid-run.
    struct FlipPolicy {
        verdict: Arc<Mutex<Verdict>>,
    }

    impl Policy for FlipPolicy {
        fn check(&self, op: Op, path: &VfsPath) -> Verdict {
            let _ = (op, path);
            self.verdict
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// Extracts the Conflict message or fails the test.
    fn conflict_message<T: std::fmt::Debug>(result: Result<T, VfsError>) -> String {
        match result {
            Err(VfsError::Conflict { detail, .. }) => detail,
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    #[test]
    fn an_access_reads_and_writes_through_the_handle() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let access = vfs.acquire(test_origin())?;
        access.write("/notes/a.txt", b"hello")?;
        assert_eq!(access.read("/notes/a.txt")?, b"hello");
        assert!(access.exists("/notes/a.txt")?);
        Ok(())
    }

    #[test]
    fn every_acquire_vends_a_process_unique_identity() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let first = vfs.acquire(test_origin())?;
        let second = vfs.acquire(test_origin())?;
        assert_ne!(first.id, second.id);
        Ok(())
    }

    #[test]
    fn one_identity_never_conflicts_with_itself() -> Result<(), VfsError> {
        // Borrow semantics: a blocking call chain uses the parent's
        // access, so sequential ops on one path by one identity stay
        // legal - no new identity, no false conflict.
        let vfs = handle(&StubFs::default());
        let access = vfs.acquire(test_origin())?;
        access.write("/f.txt", b"one")?;
        access.write("/f.txt", b"two")?;
        access.append("/f.txt", b"!")?;
        assert_eq!(access.read("/f.txt")?, b"two!");
        Ok(())
    }

    #[test]
    fn an_empty_anchor_on_an_empty_file_is_refused_and_leaves_the_file_empty()
    -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "")]));
        let access = vfs.acquire(test_origin())?;
        match access.str_replace("/f.txt", "", "x") {
            Err(VfsError::Anchor {
                path,
                anchor,
                count,
            }) => {
                assert_eq!(path, "/f.txt");
                assert!(anchor.is_empty());
                assert_eq!(count, 0);
            }
            other => panic!("expected the empty-anchor refusal, got {other:?}"),
        }
        assert_eq!(access.read("/f.txt")?, b"");
        Ok(())
    }

    #[test]
    fn an_empty_anchor_on_a_non_empty_file_is_refused_and_leaves_it_unchanged()
    -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "hello")]));
        let access = vfs.acquire(test_origin())?;
        match access.str_replace("/f.txt", "", "x") {
            Err(VfsError::Anchor {
                path,
                anchor,
                count,
            }) => {
                assert_eq!(path, "/f.txt");
                assert!(anchor.is_empty());
                assert_eq!(count, 0);
            }
            other => panic!("expected the empty-anchor refusal, got {other:?}"),
        }
        assert_eq!(access.read("/f.txt")?, b"hello");
        Ok(())
    }

    #[test]
    fn a_write_conflicts_with_another_identitys_read_claim() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "data")]));
        let reader = vfs.acquire(test_origin())?;
        let writer = vfs.acquire(test_origin())?;
        reader.read("/f.txt")?;
        let message = conflict_message(writer.write("/f.txt", b"new"));
        assert!(message.contains("/f.txt"), "names the path: {message}");
        assert!(
            message.contains("read"),
            "names the standing claim kind: {message}"
        );
        assert!(
            message.contains("write"),
            "names the attempted kind: {message}"
        );
        assert!(
            message.contains(&format!("{:?}", reader.id)),
            "names the claimant: {message}"
        );
        assert!(
            message.contains(&format!("{:?}", writer.id)),
            "names the attempter: {message}"
        );
        Ok(())
    }

    #[test]
    fn a_read_conflicts_with_another_identitys_write_claim() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let writer = vfs.acquire(test_origin())?;
        writer.write("/f.txt", b"x")?;
        let reader = vfs.acquire(test_origin())?;
        match reader.read("/f.txt") {
            Err(VfsError::Conflict { .. }) => {}
            other => panic!("expected a conflict, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn two_writes_by_two_identities_conflict() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let first = vfs.acquire(test_origin())?;
        first.write("/f.txt", b"1")?;
        let second = vfs.acquire(test_origin())?;
        let message = conflict_message(second.write("/f.txt", b"2"));
        assert!(message.contains("write claim"), "{message}");
        Ok(())
    }

    #[test]
    fn reads_by_two_identities_never_conflict() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "data")]));
        let first = vfs.acquire(test_origin())?;
        let second = vfs.acquire(test_origin())?;
        first.read("/f.txt")?;
        assert_eq!(second.read("/f.txt")?, b"data");
        Ok(())
    }

    #[test]
    fn a_copy_conflicts_with_another_identitys_write_on_the_source() -> Result<(), VfsError> {
        // Copy claims the source as a read, and a read booms on another
        // scope's write claim.
        let vfs = handle(&StubFs::default());
        let writer = vfs.acquire(test_origin())?;
        writer.write("/src.txt", b"data")?;
        let copier = vfs.acquire(test_origin())?;
        let message = conflict_message(copier.copy("/src.txt", "/dst.txt"));
        assert!(message.contains("/src.txt"), "names the source: {message}");
        Ok(())
    }

    #[test]
    fn a_copy_shares_the_source_with_another_identitys_read() -> Result<(), VfsError> {
        // The source claim is a read, not a write: another identity's
        // read claim on the source must not block the copy. Were the
        // source claimed as a write, this copy would conflict.
        let vfs = handle(&StubFs::seeded(&[("/src.txt", "data")]));
        let reader = vfs.acquire(test_origin())?;
        reader.read("/src.txt")?;
        let copier = vfs.acquire(test_origin())?;
        copier.copy("/src.txt", "/dst.txt")?;
        assert_eq!(copier.read("/dst.txt")?, b"data");
        Ok(())
    }

    #[test]
    fn a_copy_conflicts_with_another_identitys_claim_on_the_destination() -> Result<(), VfsError> {
        // The destination is claimed as a write, so any other live
        // identity's claim on it blocks the copy.
        let vfs = handle(&StubFs::seeded(&[
            ("/src.txt", "data"),
            ("/dst.txt", "old"),
        ]));
        let reader = vfs.acquire(test_origin())?;
        reader.read("/dst.txt")?;
        let copier = vfs.acquire(test_origin())?;
        let message = conflict_message(copier.copy("/src.txt", "/dst.txt"));
        assert!(
            message.contains("/dst.txt"),
            "names the destination: {message}"
        );
        Ok(())
    }

    #[test]
    fn a_rename_conflicts_with_a_claim_on_the_source_path() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/from.txt", "data")]));
        let reader = vfs.acquire(test_origin())?;
        reader.read("/from.txt")?;
        let renamer = vfs.acquire(test_origin())?;
        let message = conflict_message(renamer.rename("/from.txt", "/to.txt"));
        assert!(message.contains("/from.txt"), "names the source: {message}");
        Ok(())
    }

    #[test]
    fn a_rename_conflicts_with_a_claim_on_the_destination_path() -> Result<(), VfsError> {
        // Both paths are claimed as writes; were the second gate dropped,
        // this rename would sail through against the standing claim.
        let vfs = handle(&StubFs::seeded(&[
            ("/from.txt", "data"),
            ("/to.txt", "old"),
        ]));
        let reader = vfs.acquire(test_origin())?;
        reader.read("/to.txt")?;
        let renamer = vfs.acquire(test_origin())?;
        let message = conflict_message(renamer.rename("/from.txt", "/to.txt"));
        assert!(
            message.contains("/to.txt"),
            "names the destination: {message}"
        );
        Ok(())
    }

    #[test]
    fn dropping_an_access_releases_its_identity_and_claims() -> Result<(), VfsError> {
        let stub = StubFs::default();
        let vfs = handle(&stub);
        let first = vfs.acquire(test_origin())?;
        let first_id = first.id;
        first.write("/f.txt", b"1")?;
        drop(first);
        assert!(stub.released().contains(&first_id));
        let second = vfs.acquire(test_origin())?;
        second.write("/f.txt", b"2")?;
        assert_eq!(second.read("/f.txt")?, b"2");
        Ok(())
    }

    #[test]
    fn an_ended_scopes_claims_are_ignored() -> Result<(), VfsError> {
        // The scope model's release rule: a scope's claims die with its
        // last identity, so a dropped sole identity never blocks the next
        // scope. Were the ended scope's claims still live, this write
        // would conflict.
        let vfs = handle(&StubFs::default());
        let first = vfs.acquire(test_origin())?;
        first.write("/f.txt", b"1")?;
        drop(first);
        let second = vfs.acquire(test_origin())?;
        second.write("/f.txt", b"2")?;
        assert_eq!(second.read("/f.txt")?, b"2");
        Ok(())
    }

    #[test]
    fn a_child_sees_its_parents_pre_spawn_writes() -> Result<(), VfsError> {
        // The fork's snapshot: everything the parent did before the
        // spawn happens before the child's first step, so the child can
        // touch the same path without a false conflict.
        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire(test_origin())?;
        parent.write("/f.txt", b"1")?;
        let child = parent.spawn(test_origin())?;
        assert_ne!(parent.id, child.id);
        child.write("/f.txt", b"2")?;
        assert_eq!(child.read("/f.txt")?, b"2");
        Ok(())
    }

    #[test]
    fn a_child_of_a_directly_wrapped_handle_reads_its_parents_pre_spawn_write()
    -> Result<(), VfsError> {
        // The wrapped handle keeps its own claims table, so the child
        // must reach it in the parent's scope or the read conflicts.
        let vfs = VfsRef::new(handle(&StubFs::default()));
        let parent = vfs.acquire(test_origin())?;
        parent.write("/p", b"1")?;
        let child = parent.spawn(test_origin())?;
        assert_eq!(child.read("/p")?, b"1");
        Ok(())
    }

    #[test]
    fn a_forked_child_shares_its_parents_clock_snapshot_instead_of_copying_it()
    -> Result<(), VfsError> {
        // The Memory item: a spawn reuses the parent's seen snapshot -
        // one Arc clone - instead of copying the whole map, so a fanout
        // of N holds N views over one shared map rather than N copies of
        // size O(N).
        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire(test_origin())?;
        parent.write("/seed.txt", b"1")?;
        let child = parent.spawn(test_origin())?;
        let shared = {
            let inner = parent.scope.lock();
            let parent_identity = inner
                .identities
                .get(&parent.id)
                .expect("the parent's identity is registered");
            let child_identity = inner
                .identities
                .get(&child.id)
                .expect("the child's identity is registered");
            Arc::ptr_eq(&parent_identity.seen, &child_identity.seen)
        };
        assert!(shared, "the child reuses the parent's seen snapshot Arc");
        Ok(())
    }

    #[test]
    fn a_parents_post_spawn_write_conflicts_with_the_child_reading_it() -> Result<(), VfsError> {
        // The fork's other half: the parent's entry advanced at the
        // spawn, so a write after it is unordered with the child's reads.
        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire(test_origin())?;
        let child = parent.spawn(test_origin())?;
        parent.write("/f.txt", b"1")?;
        let message = conflict_message(child.read("/f.txt"));
        assert!(message.contains("/f.txt"), "names the path: {message}");
        Ok(())
    }

    #[test]
    fn a_join_makes_a_finished_arms_writes_readable() -> Result<(), VfsError> {
        // The fanout pattern happens-before teaches: the parent joins
        // each arm in turn, and the join orders the arm's writes before
        // the next arm's first step.
        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire(test_origin())?;
        let arm_one = parent.spawn(test_origin())?;
        arm_one.write("/evidence.md", b"one\n")?;
        let arm_one_id = arm_one.id;
        drop(arm_one);
        parent.join(arm_one_id);
        let arm_two = parent.spawn(test_origin())?;
        arm_two.append("/evidence.md", b"two\n")?;
        assert_eq!(arm_two.read("/evidence.md")?, b"one\ntwo\n");
        Ok(())
    }

    #[test]
    fn a_sibling_read_of_a_siblings_write_conflicts() -> Result<(), VfsError> {
        // The unordered read-write pair: one arm writes a file, a sibling
        // reads it, and nothing ordered the two arms, so the read fails
        // in every interleaving.
        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire(test_origin())?;
        let writer = parent.spawn(test_origin())?;
        writer.write("/research/5.md", b"five")?;
        let reader = parent.spawn(test_origin())?;
        let message = conflict_message(reader.read("/research/5.md"));
        assert!(
            message.contains("/research/5.md"),
            "names the path: {message}"
        );
        Ok(())
    }

    #[test]
    fn a_task_forked_after_a_write_reads_it_and_one_forked_before_conflicts() -> Result<(), VfsError>
    {
        // The fork's ordering: A's clock was forked before the owner's
        // write, so A's read always conflicts; B was forked after, so B's
        // read always passes.
        let vfs = handle(&StubFs::default());
        let owner = vfs.acquire(test_origin())?;
        let a = owner.spawn(test_origin())?;
        owner.write("/x.txt", b"1")?;
        let b = owner.spawn(test_origin())?;
        let message = conflict_message(a.read("/x.txt"));
        assert!(message.contains("/x.txt"), "A conflicts: {message}");
        assert_eq!(b.read("/x.txt")?, b"1");
        Ok(())
    }

    #[test]
    fn a_seeding_scope_and_the_runs_scope_do_not_conflict() -> Result<(), VfsError> {
        // Host seeding and the run are separate scopes: the seeding
        // scope ends with its access, so the run reads freely.
        let vfs = handle(&StubFs::default());
        let seeding = vfs.acquire(test_origin())?;
        seeding.write("/brief.md", b"seeded")?;
        drop(seeding);
        let run = vfs.acquire(test_origin())?;
        assert_eq!(run.read("/brief.md")?, b"seeded");
        Ok(())
    }

    #[test]
    fn two_live_scopes_writing_one_path_conflict() -> Result<(), VfsError> {
        // Two concurrent runs share a host base: their scopes are both
        // live and nothing orders two scopes, so the second write
        // conflicts exactly as it did under the liveness model.
        let vfs = handle(&StubFs::default());
        let first = vfs.acquire(test_origin())?;
        first.write("/shared.txt", b"1")?;
        let second = vfs.acquire(test_origin())?;
        let message = conflict_message(second.write("/shared.txt", b"2"));
        assert!(message.contains("/shared.txt"), "names the path: {message}");
        Ok(())
    }

    #[test]
    fn a_glob_racing_a_siblings_write_conflicts_in_either_order() -> Result<(), VfsError> {
        // The pattern-overlaps-path rule, both ways round: whichever
        // access runs second detects the overlap between the pattern and
        // the path it matches.
        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire(test_origin())?;
        let globber = parent.spawn(test_origin())?;
        let writer = parent.spawn(test_origin())?;
        globber.glob("/research/*")?;
        let message = conflict_message(writer.write("/research/5.md", b"five"));
        assert!(
            message.contains("/research"),
            "the write detects the glob: {message}"
        );

        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire(test_origin())?;
        let writer = parent.spawn(test_origin())?;
        let globber = parent.spawn(test_origin())?;
        writer.write("/research/5.md", b"five")?;
        let message = conflict_message(globber.glob("/research/*"));
        assert!(
            message.contains("/research"),
            "the glob detects the write: {message}"
        );
        Ok(())
    }

    #[test]
    fn an_exists_racing_a_write_into_the_directory_conflicts() -> Result<(), VfsError> {
        // A write covers the ancestors it may create, so a sibling's
        // `exists` on the directory conflicts however the two land.
        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire(test_origin())?;
        let prober = parent.spawn(test_origin())?;
        let writer = parent.spawn(test_origin())?;
        prober.exists("/research")?;
        let message = conflict_message(writer.write("/research/5.md", b"five"));
        assert!(
            message.contains("/research"),
            "the write detects the probe: {message}"
        );
        Ok(())
    }

    #[test]
    fn transfer_of_control_moves_the_identity_with_the_access() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "data")]));
        let original = vfs.acquire(test_origin())?;
        original.read("/f.txt")?;
        // Transfer of control moves the access object; the identity - and
        // with it the scope's reference count - moves with it.
        let moved = original;
        let other = vfs.acquire(test_origin())?;
        let message = conflict_message(other.write("/f.txt", b"new"));
        assert!(message.contains(&format!("{:?}", moved.id)));
        assert_eq!(moved.read("/f.txt")?, b"data");
        Ok(())
    }

    #[test]
    fn alias_spellings_of_one_file_land_on_one_claim_key() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/a/b.txt", "x")]));
        let reader = vfs.acquire(test_origin())?;
        reader.read("/a/./b.txt")?;
        let writer = vfs.acquire(test_origin())?;
        let message = conflict_message(writer.write("/a//b.txt", b"y"));
        assert!(message.contains("/a/b.txt"), "the canonical key: {message}");
        Ok(())
    }

    #[test]
    fn claims_are_shared_across_handle_clones() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let clone = vfs.clone();
        let first = vfs.acquire(test_origin())?;
        first.write("/f.txt", b"1")?;
        let second = clone.acquire(test_origin())?;
        let message = conflict_message(second.write("/f.txt", b"2"));
        assert!(message.contains("/f.txt"), "{message}");
        Ok(())
    }

    #[test]
    fn a_denied_operation_never_registers_a_claim() -> Result<(), VfsError> {
        let verdict = Arc::new(Mutex::new(Verdict::Deny("writes are sealed".to_owned())));
        let vfs = VfsRef::with_policy(
            StubFs::default(),
            FlipPolicy {
                verdict: Arc::clone(&verdict),
            },
        );
        let denied = vfs.acquire(test_origin())?;
        match denied.write("/f.txt", b"x") {
            Err(VfsError::PermissionDenied { path, reason }) => {
                assert_eq!(path, "/f.txt");
                assert_eq!(reason, "writes are sealed");
            }
            other => panic!("expected a denial, got {other:?}"),
        }
        // The host flips the policy mid-run through shared state.
        *verdict.lock().unwrap_or_else(PoisonError::into_inner) = Verdict::Allow;
        let allowed = vfs.acquire(test_origin())?;
        // Had the denied attempt registered a write claim, this write
        // would conflict with it.
        allowed.write("/f.txt", b"x")?;
        assert_eq!(allowed.read("/f.txt")?, b"x");
        Ok(())
    }

    #[test]
    fn read_range_slices_lines_one_based_and_inclusive() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "one\ntwo\nthree\n")]));
        let access = vfs.acquire(test_origin())?;
        assert_eq!(access.read_range("/f.txt", 2, None)?, "two\nthree");
        assert_eq!(access.read_range("/f.txt", 2, Some(99))?, "two\nthree");
        assert_eq!(access.read_range("/f.txt", 99, None)?, "");
        assert_eq!(access.read_range("/f.txt", 1, Some(1))?, "one");
        Ok(())
    }

    #[test]
    fn read_range_rejects_invalid_bounds() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "one\ntwo\n")]));
        let access = vfs.acquire(test_origin())?;
        assert_eq!(
            access.read_range("/f.txt", 0, None),
            Err(VfsError::InvalidRange {
                path: "/f.txt".to_owned(),
                reason: "start is below 1",
            })
        );
        assert_eq!(
            access.read_range("/f.txt", 2, Some(1)),
            Err(VfsError::InvalidRange {
                path: "/f.txt".to_owned(),
                reason: "end is before start",
            })
        );
        Ok(())
    }

    #[test]
    fn read_range_numbered_numbers_absolutely_from_start() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "one\ntwo\nthree\n")]));
        let access = vfs.acquire(test_origin())?;
        assert_eq!(
            access.read_range_numbered("/f.txt", 1, None)?,
            "1| one\n2| two\n3| three"
        );
        assert_eq!(
            access.read_range_numbered("/f.txt", 2, Some(3))?,
            "2| two\n3| three"
        );
        assert_eq!(access.read_range_numbered("/f.txt", 99, None)?, "");
        Ok(())
    }

    #[test]
    fn read_range_numbered_pads_to_the_widest_emitted_number() -> Result<(), VfsError> {
        let lines: Vec<String> = (1..=10).map(|n| format!("line{n}")).collect();
        let text = lines.join("\n");
        let vfs = handle(&StubFs::seeded(&[("/f.txt", &text)]));
        let access = vfs.acquire(test_origin())?;
        assert_eq!(
            access.read_range_numbered("/f.txt", 9, Some(10))?,
            " 9| line9\n10| line10"
        );
        Ok(())
    }

    #[test]
    fn read_string_rejects_non_utf8() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let access = vfs.acquire(test_origin())?;
        access.write("/bin.dat", &[0xff, 0xfe])?;
        match access.read_string("/bin.dat") {
            Err(VfsError::NotUtf8 { path }) => {
                assert_eq!(path, "/bin.dat");
            }
            other => panic!("expected a UTF-8 failure, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn the_handle_and_capability_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<VfsRef>();
        assert_send_sync::<Access>();
        assert_send_sync::<AcquireContext>();
    }

    /// A backend that refuses every acquisition: the trait's contract
    /// allows refusal, so the handle must surface it as an error rather
    /// than panic.
    struct RefusingFs;

    impl Vfs for RefusingFs {
        fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
            let _ = cx;
            Err(VfsError::Backend {
                message: "the backend refuses acquisition".to_owned(),
            })
        }

        fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
            let _ = id;
            Ok(())
        }
    }

    #[test]
    fn a_backend_refusal_fails_acquire_with_an_error_instead_of_panicking() {
        let vfs = VfsRef::new(RefusingFs);
        match vfs.acquire(test_origin()) {
            Err(VfsError::Backend { message }) => {
                assert_eq!(message, "the backend refuses acquisition");
            }
            other => panic!("expected a backend refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_backend_refusal_fails_spawn_and_releases_the_refused_child() -> Result<(), VfsError> {
        /// A backend that refuses exactly its second acquisition: the
        /// spawn is the second.
        struct RefuseSecond {
            vended: Arc<Mutex<usize>>,
        }

        impl Vfs for RefuseSecond {
            fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
                let _ = cx;
                let mut vended = self.vended.lock().unwrap_or_else(PoisonError::into_inner);
                *vended += 1;
                if *vended == 2 {
                    return Err(VfsError::Backend {
                        message: "the backend refuses acquisition".to_owned(),
                    });
                }
                Ok(Box::new(StubAccess {
                    files: Arc::new(Mutex::new(BTreeMap::new())),
                }))
            }

            fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
                let _ = id;
                Ok(())
            }
        }

        let vfs = VfsRef::new(RefuseSecond {
            vended: Arc::new(Mutex::new(0)),
        });
        let parent = vfs.acquire(test_origin())?;
        parent.write("/f.txt", b"1")?;
        match parent.spawn(test_origin()) {
            Err(VfsError::Backend { message }) => {
                assert_eq!(message, "the backend refuses acquisition");
            }
            other => panic!("expected a backend refusal, got {other:?}"),
        }
        // The refused child's reference was released, so only the
        // parent keeps the scope live, and the parent's write still
        // conflicts with another live scope's.
        assert_eq!(parent.scope.live.load(Ordering::Acquire), 1);
        let other = vfs.acquire(test_origin())?;
        let message = conflict_message(other.write("/f.txt", b"2"));
        assert!(message.contains("/f.txt"), "{message}");
        Ok(())
    }

    #[test]
    fn a_sink_receives_events_in_order_with_op_path_and_label() -> Result<(), VfsError> {
        /// One recorded event: op, canonical path, label, and line.
        type Recorded = (Op, String, String, u32);

        let events: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        let vfs = VfsRef::builder()
            .mount("/", StubFs::seeded(&[("/a.txt", "x")]))
            .on_op(move |event: OpEvent<'_>| {
                recorded
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((
                        event.op(),
                        event.path().to_string(),
                        event.origin().label.clone(),
                        event.origin().line,
                    ));
            })
            .build();
        let access = vfs.acquire(Origin::at("the section", "the prompt", 7))?;
        access.read("/a.txt")?;
        access.write("/b.txt", b"y")?;
        access.glob("/*.txt")?;
        // A spawned child's events report the child's own origin.
        let child = access.spawn(Origin::at("the arm", "the prompt", 9))?;
        child.append("/b.txt", b"!")?;
        let events = events.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(
            *events,
            vec![
                (Op::Read, "/a.txt".to_owned(), "the section".to_owned(), 7),
                (Op::Write, "/b.txt".to_owned(), "the section".to_owned(), 7),
                (Op::Glob, "/*.txt".to_owned(), "the section".to_owned(), 7),
                (Op::Append, "/b.txt".to_owned(), "the arm".to_owned(), 9),
            ]
        );
        Ok(())
    }

    #[test]
    fn a_handle_without_a_sink_serves_operations_without_firing() -> Result<(), VfsError> {
        // The None sink branch: no `on_op`, and operations behave
        // exactly as before - not firing is a no-op, never a panic.
        let vfs = VfsRef::builder().mount("/", StubFs::default()).build();
        let access = vfs.acquire(test_origin())?;
        access.write("/f.txt", b"x")?;
        assert_eq!(access.read("/f.txt")?, b"x");
        Ok(())
    }

    #[test]
    fn a_policy_denied_operation_never_fires_the_sink() -> Result<(), VfsError> {
        let verdict = Arc::new(Mutex::new(Verdict::Deny("writes are sealed".to_owned())));
        let events: Arc<Mutex<Vec<Op>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        let vfs = VfsRef::builder()
            .mount("/", StubFs::default())
            .policy(FlipPolicy {
                verdict: Arc::clone(&verdict),
            })
            .on_op(move |event: OpEvent<'_>| {
                recorded
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(event.op());
            })
            .build();
        let access = vfs.acquire(test_origin())?;
        assert!(matches!(
            access.write("/f.txt", b"x"),
            Err(VfsError::PermissionDenied { .. })
        ));
        assert!(
            events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty(),
            "a denied operation fired the sink"
        );
        // The host flips the policy mid-run; the admitted write fires.
        *verdict.lock().unwrap_or_else(PoisonError::into_inner) = Verdict::Allow;
        access.write("/f.txt", b"x")?;
        assert_eq!(
            events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            &[Op::Write]
        );
        Ok(())
    }

    /// The rooted-path, idempotent-remove, and split-glob semantics of
    /// the public capability, exercised over the memory backend.
    mod semantics {
        use crate::memory::MemoryBackend;

        use super::{VfsError, VfsRef, test_origin};
        use crate::PathReason;

        fn memory() -> VfsRef {
            VfsRef::new(MemoryBackend::new())
        }

        #[test]
        fn a_relative_path_joins_onto_the_access_root() -> Result<(), VfsError> {
            let access = memory().acquire(test_origin())?;
            access.write("notes/a.txt", b"hi")?;
            assert_eq!(access.read("/notes/a.txt")?, b"hi");
            assert_eq!(access.read("notes/a.txt")?, b"hi");
            Ok(())
        }

        #[test]
        fn dotdot_stops_at_the_access_root() -> Result<(), VfsError> {
            let access = memory().acquire(test_origin())?;
            access.write("/drafts/f.txt", b"x")?;
            assert_eq!(access.read("drafts/../drafts/f.txt")?, b"x");
            assert_eq!(
                access.read("../f.txt"),
                Err(VfsError::InvalidPath {
                    path: "../f.txt".to_owned(),
                    reason: PathReason::Traversal,
                })
            );
            Ok(())
        }

        #[test]
        fn removing_a_missing_path_is_ok_false() -> Result<(), VfsError> {
            let access = memory().acquire(test_origin())?;
            assert!(!access.remove("/gone.txt", false)?);
            access.write("/f.txt", b"x")?;
            assert!(access.remove("/f.txt", false)?);
            assert!(!access.remove("/f.txt", false)?);
            Ok(())
        }

        #[test]
        fn glob_returns_files_and_a_trailing_slash_selects_directories() -> Result<(), VfsError> {
            let access = memory().acquire(test_origin())?;
            access.write("/d/a.txt", b"")?;
            access.write("/d/sub/c.txt", b"")?;
            assert_eq!(access.glob("/d/*")?, vec!["/d/a.txt".to_owned()]);
            assert_eq!(access.glob("/d/*/")?, vec!["/d/sub".to_owned()]);
            Ok(())
        }

        #[test]
        fn glob_refuses_a_backslash_in_the_raw_pattern() -> Result<(), VfsError> {
            let access = memory().acquire(test_origin())?;
            access.write("/a.txt", b"")?;
            // A backslash is refused as written, never turned into a
            // separator by canonicalization.
            assert_eq!(
                access.glob("/a\\b"),
                Err(VfsError::InvalidPath {
                    path: "/a\\b".to_owned(),
                    reason: PathReason::Backslash,
                })
            );
            assert_eq!(
                access.glob("/a/***/b"),
                Err(VfsError::InvalidPath {
                    path: "/a/***/b".to_owned(),
                    reason: PathReason::Wildcard,
                })
            );
            Ok(())
        }

        #[test]
        fn glob_names_the_rule_for_an_empty_control_or_over_long_pattern() -> Result<(), VfsError> {
            let access = memory().acquire(test_origin())?;
            assert_eq!(
                access.glob(""),
                Err(VfsError::InvalidPath {
                    path: String::new(),
                    reason: PathReason::Empty,
                })
            );
            assert_eq!(
                access.glob("/a\u{0}b"),
                Err(VfsError::InvalidPath {
                    path: "/a\u{0}b".to_owned(),
                    reason: PathReason::Control,
                })
            );
            let over_long = format!("/{}", "a".repeat(1024));
            assert_eq!(
                access.glob(&over_long),
                Err(VfsError::InvalidPath {
                    path: over_long,
                    reason: PathReason::TooLong,
                })
            );
            Ok(())
        }

        #[test]
        fn a_relative_pattern_yields_relative_results() -> Result<(), VfsError> {
            let access = memory().acquire(test_origin())?;
            access.write("/d/a.txt", b"")?;
            access.write("/d/b.md", b"")?;
            access.write("/d/sub/c.txt", b"")?;
            assert_eq!(access.glob("d/*.txt")?, vec!["d/a.txt".to_owned()]);
            assert_eq!(access.glob("d/*/")?, vec!["d/sub".to_owned()]);
            Ok(())
        }
    }
}
