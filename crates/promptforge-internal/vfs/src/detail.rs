//! Operations on [`Access`] that only the Engine performs.
//!
//! The `promptforge` facade never re-exports this module, so only Engine
//! crates reach it: the Harness passes a run's capability through, and
//! the Engine alone forks it for concurrent arms and for each tool call,
//! joins an arm's identity back on delivery, ends a tool call's identity
//! when the call is answered or aborted, derives the store view from a
//! chain's access for store calls, and ends the run's scope when the run
//! ends. The
//! Harness, holding the handle, acquires a store view in a scope of its own
//! with [`VfsRef::acquire_store`](crate::VfsRef::acquire_store).

use std::fmt;
use std::sync::{Arc, Weak};

use crate::handle::Scope;
use crate::{Access, ExecId, Origin, VfsError};

/// An opaque handle on the scope an [`Access`] belongs to, which the
/// Engine holds for the life of a run so [`end_scope`] can close it. It
/// never keeps the scope alive.
#[derive(Clone)]
pub struct ScopeHandle(Weak<Scope>);

impl fmt::Debug for ScopeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ScopeHandle").finish_non_exhaustive()
    }
}

/// The handle on `access`'s scope: the scope of every identity spawned
/// from it, however deep.
#[must_use]
pub fn scope_handle(access: &Access) -> ScopeHandle {
    ScopeHandle(Arc::downgrade(access.scope()))
}

/// Ends the scope behind `scope`: its claims stop conflicting at once,
/// and every access still held in it - a store view the Harness kept past the
/// run, a forwarded mount session - refuses its next operation, spawn,
/// or store view with [`VfsError::PermissionDenied`]. Idempotent, and a
/// no-op once every access in the scope has dropped.
pub fn end_scope(scope: &ScopeHandle) {
    if let Some(scope) = scope.0.upgrade() {
        scope.close();
    }
}

/// Derives the store view from a chain's [`Access`]: an ordinary
/// [`Access`] rooted at the handle's declared store root, whose
/// operations reach the store's own mount alone. The view keeps the
/// chain's identity and scope - so one chain never conflicts with
/// itself through its view - applies the store's strict logical-path
/// rules, and reports error paths as the caller supplied them.
///
/// # Errors
/// Returns an error when the handle the chain's access came from
/// declares no store.
pub fn store_view(access: &Access) -> Result<Access, VfsError> {
    access.store_view()
}

/// Probes the declared store before a run starts: derives the store
/// view from `access` and stats the store root through it, which makes
/// the view's router acquire the store backend. A root the backend
/// reports [`VfsError::NotFound`] passes, so a real directory created
/// lazily still runs.
///
/// # Errors
/// Returns an error when the handle declares no store, or when the
/// store backend refuses the session or fails the stat.
pub fn probe_store(access: &Access) -> Result<(), VfsError> {
    access.store_view()?.probe_root()
}

/// Returns the capability for a new concurrent thread of execution.
///
/// The child gets a fresh [`ExecId`] in `parent`'s scope,
/// and the spawn is the fork: the child shares a snapshot of the parent's
/// vector clock plus its own entry, while the parent's entry advances, so
/// the parent's later accesses are not ordered before the child's.
/// Everything the parent did before the spawn happens before the child's
/// first step. `origin` labels the child's operation events, as in
/// [`VfsRef::acquire`](crate::VfsRef::acquire).
///
/// # Errors
/// Returns an error when the backend refuses to acquire the child's
/// identity. A failed spawn leaves `parent`'s clock untouched.
pub fn access_spawn(parent: &Access, origin: Origin) -> Result<Access, VfsError> {
    parent.spawn(origin)
}

/// Merges `child`'s final clock into `owner`'s: the join, the matching
/// edge of [`access_spawn`]'s fork. Everything `child` did - its writes
/// included - happens before `owner`'s next step, so the owner's reads of
/// the child's files no longer conflict. A child whose identity is still
/// live (its last access not yet dropped) contributes its current clock;
/// a child whose last access dropped contributes its final clock, which
/// the scope keeps for exactly this. A child ended through [`end_access`]
/// keeps no seen snapshot, so joining it again contributes only its own
/// entry and fork chain. Joining a timer's identity - one the Engine
/// never spawned - is a no-op because no such identity exists.
pub fn access_join(owner: &Access, child: ExecId) {
    owner.join(child);
}

/// Ends `child`, the identity of one tool call, in the scope behind
/// `scope`: merges its clock into `owner`'s, as [`access_join`] does,
/// when `owner` is `Some`, and drops its seen snapshot. Every access
/// still holding `child` - the tool's own, a store view derived from it -
/// refuses its next operation, spawn, or store view with
/// [`VfsError::PermissionDenied`] while the scope stays open. A no-op
/// once every access in the scope has dropped.
pub fn end_access(scope: &ScopeHandle, owner: Option<ExecId>, child: ExecId) {
    if let Some(scope) = scope.0.upgrade() {
        scope.end(owner, child);
    }
}

/// The identity `access` was acquired or spawned under: how the Engine
/// records a task's [`ExecId`] at spawn, to join it on delivery.
#[must_use]
pub const fn access_id(access: &Access) -> ExecId {
    access.exec_id()
}

#[cfg(test)]
#[path = "detail-tests.rs"]
mod tests;
