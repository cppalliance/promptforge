//! Operations on [`Access`] that only the engine performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host: a host passes a run's capability through, and
//! the engine alone forks it for concurrent arms and joins the arms'
//! identities back on delivery.

use crate::{Access, ExecId, Origin, VfsError};

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
/// a child whose identity ended contributes its final clock, which the
/// scope keeps for exactly this. Joining a timer's identity - one the
/// engine never spawned - is a no-op because no such identity exists.
pub fn access_join(owner: &Access, child: ExecId) {
    owner.join(child);
}

/// The identity `access` was acquired or spawned under: how the engine
/// records a task's [`ExecId`] at spawn, to join it on delivery.
#[must_use]
pub const fn access_id(access: &Access) -> ExecId {
    access.exec_id()
}
