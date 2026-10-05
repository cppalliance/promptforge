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
//! and a scope ends with its last identity or when its run closes it
//! through [`crate::detail::end_scope`], whichever comes first; its
//! claims are then ignored and purged lazily, and a closed scope's
//! accesses refuse every later operation. Two live scopes never order each other, so their
//! claims always conflict. See the reference docs on the facade for the
//! region model: a read claims the path, directory children, or pattern
//! it observes; a write claims its path, the ancestors it may create, or
//! the whole subtree it removes.

mod access;
mod claims;
mod claims_tree;
mod forward;
mod operations;
mod prune;
mod region;
mod scope;
mod store_view;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::VfsError;
use crate::observe::{OpSink, Origin};
use crate::path::{VfsPath, canonicalize_absolute};
use crate::router::{Mounts, Router, StoreDecl, VfsRefBuilder};
use crate::traits::{AcquireContext, AllowAll, ExecId, Policy, Vfs, VfsAccess};

use claims::Claims;
pub(crate) use scope::Scope;

/// One mounted filesystem instance: its backend and the ledger of who is
/// touching what. The two are separately `Arc`-shareable so `overlay()`
/// can share the claims table while swapping the backend.
struct Volume {
    backend: Arc<Mutex<Box<dyn Vfs>>>,
    claims: Arc<Claims>,
    sink: Option<OpSink>,
    /// The declared store, when the builder that built this handle
    /// declared one. The store view derives from it; a mounted
    /// handle's declaration stays invisible behind its backend.
    store: Option<StoreDecl>,
}

/// A cloneable handle to a virtual filesystem: one backend plus a claims
/// ledger that records which parts of the filesystem each access has
/// read or written.
///
/// Clones share the backend, the claims ledger, and the policy. Claims
/// registered through one clone are checked against operations through
/// every other clone.
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

    /// Returns a handle over `backend` that consults `policy` on every
    /// operation.
    ///
    /// The policy can share its state with the caller, for example
    /// through an `Arc`. When the caller changes that state during a run,
    /// the next operation sees the change.
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

    /// Returns a builder for installing mounts.
    ///
    /// The mount table is fixed when [`VfsRefBuilder::build`] runs, so
    /// clones share it cheaply.
    #[must_use]
    pub fn builder() -> VfsRefBuilder {
        VfsRefBuilder::new()
    }

    /// Returns a new handle that mounts `backend` at `prefix` on top of
    /// this handle's namespace.
    ///
    /// The new handle shares this handle's claims table, so conflicts are
    /// detected across both views of the same storage. It also inherits
    /// this handle's store declaration, when it has one.
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

    /// Acquires an `Access` capability for a new serial thread of
    /// execution.
    ///
    /// Every acquire creates a fresh [`ExecId`] and starts a new *scope*.
    /// A scope is the acquired identity together with every identity
    /// later forked from it. Accesses are ordered only within one scope,
    /// so while two scopes live, a write in one conflicts with any
    /// overlapping read or write in the other. A scope's claims are
    /// ignored once its last identity ends.
    ///
    /// `origin` only labels the operation events this capability fires.
    /// It never decides whether an operation may proceed.
    ///
    /// # Errors
    /// Returns an error when the backend refuses to acquire the identity.
    pub fn acquire(&self, origin: Origin) -> Result<Access, VfsError> {
        self.acquire_with(
            &AcquireContext::new(ExecId::vend(), Scope::start()),
            Some(origin),
        )
    }

    /// Acquires a store view for a new serial thread of execution.
    ///
    /// The store view is an [`Access`] rooted at the declared store root.
    /// Its operations reach only the store's own mount. It is
    /// [`VfsRef::acquire`] followed by switching to the store view, so it
    /// starts a new scope of its own.
    ///
    /// Paths are relative to the store root and follow strict rules. A
    /// path must be 1 to 1024 bytes long and free of control characters
    /// and backslashes. No segment may be empty, `.`, or `..`, end in a
    /// dot or a space, or be a reserved device name such as `CON`. Error
    /// paths come back relative to the store root, in the form the caller
    /// supplied. So the caller can seed and extract store files by the
    /// names the prompt uses, wherever the store is mounted.
    ///
    /// # Errors
    /// Returns [`VfsError::Unsupported`] when the handle declares no
    /// store. Returns the backend's error when the backend refuses to
    /// acquire the identity.
    pub fn acquire_store(&self, origin: Origin) -> Result<Access, VfsError> {
        self.acquire(origin)?.store_view()
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
    /// see [`VfsRef::acquire`]. Returns [`VfsError::PermissionDenied`],
    /// before any backend call, when the scope in `cx` belongs to a run
    /// that has ended.
    fn acquire_with(
        &self,
        cx: &AcquireContext,
        origin: Option<Origin>,
    ) -> Result<Access, VfsError> {
        let scope = self.join_scope(cx)?;
        let inner = match self.backend().acquire(cx) {
            Ok(inner) => inner,
            Err(err) => {
                scope.release(cx.id());
                return Err(err);
            }
        };
        Ok(Access {
            id: cx.id(),
            origin,
            root: VfsPath::root(),
            store_root: None,
            volume: self.volume.clone(),
            policy: self.policy.clone(),
            scope,
            inner: Mutex::new(inner),
        })
    }

    /// Joins the scope in `cx`: attaches the identity, fresh or
    /// forwarded, and registers the scope in this handle's claims
    /// tables. A closed scope is refused before either.
    fn join_scope(&self, cx: &AcquireContext) -> Result<Arc<Scope>, VfsError> {
        let scope = Arc::clone(cx.scope());
        scope.attach(cx.id(), &VfsPath::root())?;
        self.volume.claims.register_scope(&scope);
        Ok(scope)
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

/// A capability that performs filesystem operations through a `VfsRef`
/// as one identity.
///
/// It holds an [`ExecId`] and the backend's access object. Every
/// operation canonicalizes the path, checks the policy, checks the
/// claims tables, and reports the operation to the observer installed
/// with `VfsRefBuilder::on_op`, if any. It then locks the backend for
/// that one call.
///
/// Dropping an `Access` drops one reference to its identity. The
/// identity ends when its last access drops, and the scope ends when
/// its last identity ends.
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
