//! Execution identity: the [`ExecId`] every operation is attributed to,
//! and the [`AcquireContext`] a backend receives when a session opens.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::handle::Scope;

/// The identity of one serial thread of execution.
///
/// Each identity is unique within the process, because it comes from a
/// process-wide counter that only counts up. Only a `VfsRef` and its
/// `Access` capabilities create identities. A backend receives them
/// through [`AcquireContext::id`] and [`Vfs::release`](crate::Vfs::release).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExecId(u64);

impl ExecId {
    /// Vends the next process-unique identity.
    pub(crate) fn vend() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// The context that [`Vfs::acquire`](crate::Vfs::acquire) receives: the
/// identity being acquired and the scope it belongs to.
///
/// A `VfsRef` or one of its `Access` capabilities builds each context.
/// A backend that wraps another [`Vfs`](crate::Vfs) must pass it the
/// context exactly as received. A wrapped `VfsRef` then joins the outer
/// session's scope, so its claims stay ordered with the outer session's
/// claims.
#[derive(Clone)]
pub struct AcquireContext {
    id: ExecId,
    scope: Arc<Scope>,
}

impl AcquireContext {
    /// A context acquiring `id` into `scope`.
    pub(crate) fn new(id: ExecId, scope: Arc<Scope>) -> Self {
        Self { id, scope }
    }

    /// The identity being acquired. Every operation on the access
    /// object the backend returns is attributed to it.
    #[must_use]
    pub fn id(&self) -> ExecId {
        self.id
    }

    /// The scope the identity belongs to.
    pub(crate) fn scope(&self) -> &Arc<Scope> {
        &self.scope
    }
}

impl fmt::Debug for AcquireContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcquireContext")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
