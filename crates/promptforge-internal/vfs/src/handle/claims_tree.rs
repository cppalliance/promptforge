//! The claims that cover more than one path - a directory listing, a
//! glob, a whole subtree, and a rename's or a copy's two paths - and the
//! tail every successful claim ends on.

use std::sync::Arc;

use super::claims::{ClaimKind, Claims, ClaimsTables, Epoch, conflict};
use super::prune::prune;
use super::region::{
    parent_of, pattern_base, pattern_matches_path, pattern_overlaps_subtree, subtree_covers,
};
use super::scope::{Scope, View};
use crate::error::VfsError;
use crate::path::VfsPath;
use crate::traits::ExecId;

impl Claims {
    /// Checks and records a list of `dir`'s children by `id` of `scope`:
    /// the may-create writes that would change the listing, every
    /// subtree that covers it, and every subtree rooted at one of its
    /// direct children. A subtree rooted deeper leaves the listing
    /// unchanged, so it is not a conflict.
    pub(super) fn claim_list(
        &self,
        scope: &Arc<Scope>,
        id: ExecId,
        dir: &VfsPath,
    ) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = &scope.view(id, dir)?;
        if let Some(region) = tables.ancestors.get(dir) {
            for (&other, &(other_scope, other_clock)) in &region.created {
                if tables.conflicts(scope, view, other_scope, other, other_clock) {
                    return Err(conflict(dir, id, ClaimKind::Read, other, ClaimKind::Write));
                }
            }
        }
        for (subtree, region) in &tables.subtrees {
            if !subtree_covers(subtree, dir) && parent_of(subtree).as_ref() != Some(dir) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, view, other_scope, epoch.id, epoch.clock)
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
        Self::finish_claim(scope, id, &mut tables, view.own, 1);
        Ok(())
    }

    /// Checks and records a glob of `pattern` by `id` of `scope`: every
    /// path the pattern matches, the may-create writes under its literal
    /// base, and every subtree it overlaps. The pattern itself is the
    /// claim, not each match.
    pub(super) fn claim_glob(
        &self,
        scope: &Arc<Scope>,
        id: ExecId,
        pattern: &VfsPath,
    ) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = &scope.view(id, pattern)?;
        for (path, region) in &tables.paths {
            if !pattern_matches_path(pattern, path) {
                continue;
            }
            if let Some((other_scope, epoch)) = region.write
                && tables.conflicts(scope, view, other_scope, epoch.id, epoch.clock)
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
                    if tables.conflicts(scope, view, other_scope, other, other_clock) {
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
                && tables.conflicts(scope, view, other_scope, epoch.id, epoch.clock)
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
        Self::finish_claim(scope, id, &mut tables, view.own, 1);
        Ok(())
    }

    /// Checks and records a whole-subtree claim on `path` by `id` of
    /// `scope` (a recursive remove): every check of
    /// [`Claims::check_subtree`] first, then the record.
    pub(super) fn claim_subtree(
        &self,
        scope: &Arc<Scope>,
        id: ExecId,
        path: &VfsPath,
    ) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id, path)?;
        Self::check_subtree(&tables, scope, &view, path)?;
        Self::record_subtree(&mut tables, scope, &view, path);
        Self::finish_claim(scope, id, &mut tables, view.own, 1);
        Ok(())
    }

    /// Checks and records a rename's two claims by `id` of `scope`: the
    /// source as the whole subtree it moves and the destination as a
    /// write. Both are checked under one tables lock before either is
    /// recorded, so a refusal of either leaves neither behind.
    pub(super) fn claim_rename(
        &self,
        scope: &Arc<Scope>,
        id: ExecId,
        from: &VfsPath,
        to: &VfsPath,
    ) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id, from)?;
        Self::check_subtree(&tables, scope, &view, from)?;
        Self::check_write(&tables, scope, &view, to)?;
        Self::record_subtree(&mut tables, scope, &view, from);
        Self::record_write(&mut tables, scope, &view, to);
        Self::finish_claim(scope, id, &mut tables, view.own, 2);
        Ok(())
    }

    /// Checks and records a copy's two claims by `id` of `scope`: the
    /// source as a read and the destination as a write, both checked
    /// under one tables lock before either is recorded.
    pub(super) fn claim_copy(
        &self,
        scope: &Arc<Scope>,
        id: ExecId,
        from: &VfsPath,
        to: &VfsPath,
    ) -> Result<(), VfsError> {
        let mut tables = self.tables();
        let view = scope.view(id, from)?;
        Self::check_read(&tables, scope, &view, from)?;
        Self::check_write(&tables, scope, &view, to)?;
        Self::record_read(&mut tables, scope, &view, from);
        Self::record_write(&mut tables, scope, &view, to);
        Self::finish_claim(scope, id, &mut tables, view.own, 2);
        Ok(())
    }

    /// The checks of a whole-subtree claim on `path` by `view` of
    /// `scope`: every subtree that covers it or that it covers, every
    /// path and listing under it, the listing of its parent, and every
    /// pattern that overlaps it. Records nothing.
    fn check_subtree(
        tables: &ClaimsTables,
        scope: &Scope,
        view: &View,
        path: &VfsPath,
    ) -> Result<(), VfsError> {
        let id = view.id;
        for (other, region) in &tables.subtrees {
            if !subtree_covers(path, other) && !subtree_covers(other, path) {
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
        if let Some(parent) = parent_of(path)
            && let Some(region) = tables.children.get(&parent)
        {
            for (&reader, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, view, other_scope, reader, other_clock) {
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
        for (other, region) in &tables.paths {
            if !subtree_covers(path, other) {
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
            for (&reader, &(other_scope, other_clock)) in &region.reads {
                if tables.conflicts(scope, view, other_scope, reader, other_clock) {
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
                if tables.conflicts(scope, view, other_scope, reader, other_clock) {
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
                if tables.conflicts(scope, view, other_scope, reader, other_clock) {
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
        Ok(())
    }

    /// Records a checked whole-subtree claim on `path` by `view` of
    /// `scope`.
    fn record_subtree(tables: &mut ClaimsTables, scope: &Scope, view: &View, path: &VfsPath) {
        let region = tables.subtrees.entry(path.clone()).or_default();
        region.write = Some((
            scope.id,
            Epoch {
                id: view.id,
                clock: view.own,
            },
        ));
    }

    /// The shared tail of a successful operation's `recorded` claims: the
    /// identity's clock advances past its recorded epoch, and a table
    /// that has grown past its threshold prunes.
    pub(super) fn finish_claim(
        scope: &Arc<Scope>,
        id: ExecId,
        tables: &mut ClaimsTables,
        tick: u64,
        recorded: usize,
    ) {
        scope.advance(id, tick);
        tables.entries += recorded;
        if tables.entries > tables.prune_at {
            prune(tables);
        }
    }
}
