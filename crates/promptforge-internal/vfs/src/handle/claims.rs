//! The claims ledger: the tables of who touched what, shared by every
//! scope of one storage view, and the checks and records of a single
//! path's reads and writes.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use super::region::{may_create, parent_of, pattern_matches_path, subtree_covers};
use super::scope::{Scope, ScopeId, View};
use crate::error::VfsError;
use crate::path::VfsPath;
use crate::traits::ExecId;

/// Whether an operation claims read or write intent on its region.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ClaimKind {
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
pub(super) struct Epoch {
    pub(super) id: ExecId,
    pub(super) clock: u64,
}

/// The bookkeeping of who touched what, shared by every scope of one
/// storage view. A claim holds for its scope's whole life - a prune only
/// collapses the epochs its own scope has ordered - and is ignored once
/// its scope ends and purged lazily.
pub(super) struct Claims {
    inner: Mutex<ClaimsTables>,
}

/// How large the tables must grow before a prune runs.
pub(super) const PRUNE_AT: usize = 4096;

/// One region's claims: the last write as a single epoch, reads as the
/// latest epoch per identity, and may-create writes the same way - the
/// FastTrack representation, which never stores a full vector clock per
/// claim. Every claim is tagged with its scope.
#[derive(Default)]
pub(super) struct RegionClaims {
    /// The last write epoch and its scope.
    pub(super) write: Option<(ScopeId, Epoch)>,
    /// Read epochs: the latest per identity, tagged by scope.
    pub(super) reads: HashMap<ExecId, (ScopeId, u64)>,
    /// May-create write epochs: the latest per identity, tagged by scope.
    pub(super) created: HashMap<ExecId, (ScopeId, u64)>,
}

pub(super) struct ClaimsTables {
    /// Path-keyed regions, one map per kind, so the conservative scans
    /// (patterns matching paths, subtrees covering paths) walk only the
    /// regions that can overlap them.
    pub(super) paths: HashMap<VfsPath, RegionClaims>,
    pub(super) children: HashMap<VfsPath, RegionClaims>,
    pub(super) patterns: HashMap<VfsPath, RegionClaims>,
    pub(super) subtrees: HashMap<VfsPath, RegionClaims>,
    pub(super) ancestors: HashMap<VfsPath, RegionClaims>,
    /// The scopes with claims in these tables, for the liveness check.
    pub(super) scopes: HashMap<ScopeId, Weak<Scope>>,
    /// The approximate claim count, for the prune trigger.
    pub(super) entries: usize,
    /// The count past which the next prune runs: at least [`PRUNE_AT`],
    /// and twice what survived the last prune, so a table of live claims
    /// does not prune on every claim.
    pub(super) prune_at: usize,
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
            prune_at: PRUNE_AT,
        }
    }

    /// Whether `other`'s claim conflicts with an access by `view` of
    /// `scope`. Within one scope the claim is ordered before the access
    /// exactly when the access's view has seen it; claims from another
    /// live scope always conflict, and an ended scope's are ignored.
    pub(super) fn conflicts(
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
    pub(super) fn new() -> Self {
        Self {
            inner: Mutex::new(ClaimsTables::new()),
        }
    }

    /// Poison-safe lock: each guard scope is one complete table mutation,
    /// so a panicking claimant cannot leave the tables half-updated.
    pub(super) fn tables(&self) -> MutexGuard<'_, ClaimsTables> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers `scope` in these tables' registry: claims tagged with
    /// the scope's id stay checkable for liveness.
    pub(super) fn register_scope(&self, scope: &Arc<Scope>) {
        self.tables()
            .scopes
            .entry(scope.id)
            .or_insert_with(|| Arc::downgrade(scope));
    }

    /// Checks and records a write of `path` by `id` of `scope`: every
    /// check of [`Claims::check_write`] first, then the record, so a
    /// refused write leaves no claim behind.
    pub(super) fn claim_write(
        &self,
        scope: &Arc<Scope>,
        id: ExecId,
        path: &VfsPath,
    ) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id, path)?;
        Self::check_write(&tables, scope, &view, path)?;
        Self::record_write(&mut tables, scope, &view, path);
        Self::finish_claim(scope, id, &mut tables, view.own, 1);
        Ok(())
    }

    /// The checks of a write of `path` by `view` of `scope` against the
    /// regions it overlaps - the path's own reads and writes, the writes
    /// that may create the path as a directory, its parent's children,
    /// every pattern that matches it, and every subtree that covers it -
    /// and of the ancestors it may create against their reads and
    /// writes, their parents' children, and the patterns that match
    /// them. Records nothing.
    pub(super) fn check_write(
        tables: &ClaimsTables,
        scope: &Scope,
        view: &View,
        path: &VfsPath,
    ) -> Result<(), VfsError> {
        let id = view.id;
        if let Some(region) = tables.paths.get(path) {
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, view, other_scope, epoch.id, epoch.clock)
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
                if tables.conflicts(scope, view, other_scope, other, other_clock) {
                    return Err(conflict(path, id, ClaimKind::Write, other, ClaimKind::Read));
                }
            }
        }
        if let Some(region) = tables.ancestors.get(path) {
            for (&other, &(other_scope, other_clock)) in &region.created {
                if tables.conflicts(scope, view, other_scope, other, other_clock) {
                    return Err(conflict(
                        path,
                        id,
                        ClaimKind::Write,
                        other,
                        ClaimKind::Write,
                    ));
                }
            }
        }
        Self::check_observers(tables, scope, view, path, path)?;
        for (subtree, region) in &tables.subtrees {
            if !subtree_covers(subtree, path) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, view, other_scope, epoch.id, epoch.clock)
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
        for ancestor in may_create(path) {
            // An ancestor it may create is checked, like the leaf,
            // against its parent's listings and the patterns that match
            // it, since creating it changes what those observe. It is
            // also checked against its own reads and writes: a read
            // observes the entry this write may create under it, and a
            // write or remove of the ancestor itself races with the
            // creation, while another write may create alongside.
            Self::check_observers(tables, scope, view, path, &ancestor)?;
            let Some(region) = tables.paths.get(&ancestor) else {
                continue;
            };
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, view, other_scope, epoch.id, epoch.clock)
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
                if tables.conflicts(scope, view, other_scope, other, other_clock) {
                    return Err(conflict(path, id, ClaimKind::Write, other, ClaimKind::Read));
                }
            }
        }
        Ok(())
    }

    /// The checks of a write of `path` by `view` of `scope` that creates
    /// `entry` (the path itself or an ancestor it may create) against the
    /// accesses that observe the entry's presence: listings of its parent
    /// and patterns that match it. Records nothing.
    fn check_observers(
        tables: &ClaimsTables,
        scope: &Scope,
        view: &View,
        path: &VfsPath,
        entry: &VfsPath,
    ) -> Result<(), VfsError> {
        let id = view.id;
        if let Some(parent) = parent_of(entry)
            && let Some(region) = tables.children.get(&parent)
        {
            for (&other, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, view, other_scope, other, other_clock) {
                    return Err(conflict(path, id, ClaimKind::Write, other, ClaimKind::Read));
                }
            }
        }
        for (pattern, region) in &tables.patterns {
            if !pattern_matches_path(pattern, entry) {
                continue;
            }
            for (&other, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, view, other_scope, other, other_clock) {
                    return Err(conflict(path, id, ClaimKind::Write, other, ClaimKind::Read));
                }
            }
        }
        Ok(())
    }

    /// Records a checked write of `path` by `view` of `scope`: the last
    /// write replaces the standing one, reads ordered before it clear (an
    /// access racing with one of them races with the write instead), and
    /// each ancestor it may create gains a may-create claim.
    pub(super) fn record_write(
        tables: &mut ClaimsTables,
        scope: &Scope,
        view: &View,
        path: &VfsPath,
    ) {
        let region = tables.paths.entry(path.clone()).or_default();
        region.write = Some((
            scope.id,
            Epoch {
                id: view.id,
                clock: view.own,
            },
        ));
        region
            .reads
            .retain(|&other, &mut (other_scope, other_clock)| {
                other_scope != scope.id || other_clock > view.seen_clock(other)
            });
        for ancestor in may_create(path) {
            let region = tables.ancestors.entry(ancestor).or_default();
            region.created.insert(view.id, (scope.id, view.own));
        }
    }

    /// Checks and records a read of `path` by `id` of `scope`: every
    /// check of [`Claims::check_read`] first, then the record.
    pub(super) fn claim_read(
        &self,
        scope: &Arc<Scope>,
        id: ExecId,
        path: &VfsPath,
    ) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id, path)?;
        Self::check_read(&tables, scope, &view, path)?;
        Self::record_read(&mut tables, scope, &view, path);
        Self::finish_claim(scope, id, &mut tables, view.own, 1);
        Ok(())
    }

    /// The checks of a read of `path` by `view` of `scope`: the path's
    /// last write, the may-create writes on the path itself, and every
    /// subtree that covers it. Reads never conflict with reads. Records
    /// nothing.
    pub(super) fn check_read(
        tables: &ClaimsTables,
        scope: &Scope,
        view: &View,
        path: &VfsPath,
    ) -> Result<(), VfsError> {
        let id = view.id;
        if let Some(region) = tables.paths.get(path)
            && let Some((other_scope, epoch)) = region.write
            && tables.conflicts(scope, view, other_scope, epoch.id, epoch.clock)
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
                if tables.conflicts(scope, view, other_scope, other, other_clock) {
                    return Err(conflict(path, id, ClaimKind::Read, other, ClaimKind::Write));
                }
            }
        }
        for (subtree, region) in &tables.subtrees {
            if !subtree_covers(subtree, path) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, view, other_scope, epoch.id, epoch.clock)
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
        Ok(())
    }

    /// Records a checked read of `path` by `view` of `scope`.
    pub(super) fn record_read(
        tables: &mut ClaimsTables,
        scope: &Scope,
        view: &View,
        path: &VfsPath,
    ) {
        let region = tables.paths.entry(path.clone()).or_default();
        region.reads.insert(view.id, (scope.id, view.own));
    }
}

/// The conflict error names the path, both identities, and both claim
/// kinds: the executor maps it to a fatal run error, and the message is
/// the whole diagnosis.
pub(super) fn conflict(
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
