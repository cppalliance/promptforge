//! The backend traits, the policy hook, and execution identity.
//!
//! `Vfs` is one backend behind the virtual namespace; `VfsAccess` is one
//! identity's session with it and declares every filesystem operation.
//! `Policy` is the per-handle hook consulted before the claims check, and
//! `ExecId` is the identity every operation is attributed to.

mod backend;
mod identity;
mod policy;
#[cfg(test)]
mod tests;

pub use backend::{Depth, GlobKind, Vfs, VfsAccess};
pub use identity::{AcquireContext, ExecId};
pub use policy::{AllowAll, Op, Policy, Verdict};
