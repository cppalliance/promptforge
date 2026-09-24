//! Operations on [`Access`] that only the engine performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host: a host passes a run's capability through, and
//! the engine alone forks it for concurrent arms and reads its identity.

use crate::{Access, ExecId, Origin, VfsError};

/// Returns the capability for a new concurrent thread of execution.
///
/// The child gets a fresh [`ExecId`], and `parent`'s claims are deleted
/// from the tables: they predate the child by construction, so a retired
/// claim can never conflict again. The spawn IS the happens-before edge -
/// no fence call, no epochs. `origin` labels the child's operation events,
/// as in [`VfsRef::acquire`](crate::VfsRef::acquire).
///
/// # Errors
/// Returns an error when the backend refuses to acquire the child's
/// identity. A failed spawn leaves `parent`'s claims untouched.
pub fn access_spawn(parent: &Access, origin: Origin) -> Result<Access, VfsError> {
    parent.spawn(origin)
}

/// Returns a capability's identity.
#[must_use]
pub fn access_id(access: &Access) -> ExecId {
    access.id()
}
