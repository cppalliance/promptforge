//! The mount table and its builder.
//!
//! A `Router` is itself a [`Vfs`], so routers nest; the public concept
//! is "a `VfsRef` with these mounts," expressed through
//! [`VfsRefBuilder`]. Mounts are fixed at build, so the table is
//! immutable and cheap to `Arc`-share. The routing access resolves the
//! longest-prefix mount per operation, strips the mount prefix so each
//! backend sees a rooted path within its own mount, and acquires each
//! backend's access lazily on first touch.

#[path = "router-access.rs"]
mod access;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::VfsError;
use crate::handle::{CaseFolds, VfsRef};
use crate::observe::{OpEvent, OpSink};
use crate::path::{VfsPath, VfsPathBuf, canonicalize_absolute};
use crate::traits::{AcquireContext, AllowAll, ExecId, Policy, Vfs, VfsAccess};

use access::RoutingAccess;

/// One mounted backend behind a shared lock.
type Mounted = Arc<Mutex<Box<dyn Vfs>>>;

/// The mount table: backends at canonical prefixes, longest prefix
/// wins. Immutable after construction, so it is cheap to `Arc`-share
/// with every routing access the router vends.
pub(crate) type Mounts = BTreeMap<VfsPathBuf, Mounted>;

/// Poison-safe lock on a mounted backend.
fn lock(backend: &Mounted) -> MutexGuard<'_, Box<dyn Vfs>> {
    backend.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the mount at `prefix` serves `path`: the root mount serves
/// everything; any other mount serves itself and its descendants.
pub(crate) fn mount_matches(prefix: &str, path: &str) -> bool {
    if prefix == "/" {
        return true;
    }
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Resolves the longest-prefix mount serving `path`.
fn resolve<'m>(mounts: &'m Mounts, path: &str) -> Option<(&'m VfsPathBuf, &'m Mounted)> {
    mounts
        .iter()
        .filter(|(prefix, _)| mount_matches(prefix.as_str(), path))
        .max_by_key(|(prefix, _)| prefix.as_str().len())
}

/// The path a mounted backend sees: the mount prefix stripped, rooted
/// at the mount. The mount itself is its own root.
fn strip_mount<'a>(prefix: &str, path: &'a str) -> &'a str {
    if prefix == "/" {
        return path;
    }
    match path.strip_prefix(prefix) {
        Some("") => "/",
        Some(rest) => rest,
        None => path, // unreachable: resolve() matched the prefix
    }
}

/// Restores the mount prefix on a path a backend returned, so callers
/// see full virtual paths.
pub(crate) fn rejoin(prefix: &str, path: &str) -> String {
    if prefix == "/" {
        return path.to_owned();
    }
    if path == "/" {
        return prefix.to_owned();
    }
    format!("{prefix}{path}")
}

/// The mount table. Backends install at prefixes; longest prefix wins.
/// A Router is itself a [`Vfs`], so routers nest. Crate-private: the
/// public concept is "a `VfsRef` with these mounts," expressed through
/// [`VfsRefBuilder`]; privacy enforces mounts-fixed-at-construction,
/// since nobody outside the crate can hold one.
pub(crate) struct Router {
    mounts: Arc<Mounts>,
}

impl Router {
    pub(crate) fn new(mounts: Mounts) -> Router {
        Router {
            mounts: Arc::new(mounts),
        }
    }
}

impl Vfs for Router {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        Ok(Box::new(RoutingAccess {
            cx: cx.clone(),
            mounts: Arc::clone(&self.mounts),
            acquired: RefCell::new(BTreeMap::new()),
        }))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        // The routing access's close releases the identity at every
        // touched mount; the router itself holds no per-identity state.
        let _ = id;
        Ok(())
    }

    // read_only() keeps the default: a router mixes mounts, and the
    // routing access enforces each mount's own flag per operation.
}

/// A builder that mounts backends at path prefixes and then builds a
/// [`VfsRef`] over them.
///
/// The mount table is fixed for good when [`VfsRefBuilder::build`]
/// runs, so it is cheap to share through an `Arc`.
pub struct VfsRefBuilder {
    mounts: Mounts,
    policy: Option<Arc<dyn Policy + Sync>>,
    sink: Option<OpSink>,
    store: Option<StoreDecl>,
    folds: CaseFolds,
}

/// The handle's declared store: the mount serving the run's store,
/// when the builder declared one. `root` is the canonical store root
/// the store view joins logical paths onto; `mount` is the store's
/// backend, shared with the mount table; `folds` is whether its claim
/// keys fold case. Only the declaration of the handle a builder builds
/// counts: an overlay inherits its base's, while a handle mounted as a
/// backend keeps its declaration to itself.
#[derive(Clone)]
pub(crate) struct StoreDecl {
    pub(crate) root: VfsPath,
    pub(crate) mount: Mounted,
    pub(crate) folds: CaseFolds,
}

impl fmt::Debug for VfsRefBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VfsRefBuilder").finish_non_exhaustive()
    }
}

impl VfsRefBuilder {
    pub(crate) fn new() -> VfsRefBuilder {
        VfsRefBuilder {
            mounts: BTreeMap::new(),
            policy: None,
            sink: None,
            store: None,
            folds: CaseFolds::default(),
        }
    }

    /// Mounts `backend` at `prefix`, consuming and returning the
    /// builder.
    ///
    /// The root prefix `/` serves the whole namespace. A longer prefix
    /// shadows a shorter one, so the longest matching prefix serves
    /// each path.
    ///
    /// # Panics
    /// Panics when `prefix` is not an absolute virtual path or a mount
    /// already sits at `prefix`: both are construction-time bugs.
    #[must_use]
    #[track_caller]
    pub fn mount(mut self, prefix: &str, backend: impl Vfs + 'static) -> VfsRefBuilder {
        let canonical = self.free_prefix(prefix, "mount prefix");
        self.folds = self.folds.with(&CaseFolds::of(&canonical, &backend));
        self.mounts
            .insert(canonical.to_buf(), Arc::new(Mutex::new(Box::new(backend))));
        self
    }

    /// Mounts `backend` at `root` and declares that mount the handle's
    /// store, consuming and returning the builder.
    ///
    /// A store view, such as the access that `VfsRef::acquire_store`
    /// returns, reaches only this mount, and its paths are relative to
    /// `root`.
    ///
    /// Only the outermost handle's declaration counts. A handle made
    /// with `VfsRef::overlay` inherits the declaration of its base. A
    /// handle mounted as a backend keeps its declaration to itself.
    ///
    /// # Panics
    /// Panics when `root` is not an absolute virtual path or a mount
    /// already sits at `root`: both are construction-time bugs.
    #[must_use]
    #[track_caller]
    pub fn store(mut self, root: &str, backend: impl Vfs + 'static) -> VfsRefBuilder {
        let canonical = self.free_prefix(root, "store root");
        let folds = CaseFolds::of(&canonical, &backend);
        self.folds = self.folds.with(&folds);
        let mount: Mounted = Arc::new(Mutex::new(Box::new(backend)));
        self.mounts.insert(canonical.to_buf(), Arc::clone(&mount));
        self.store = Some(StoreDecl {
            root: canonical,
            mount,
            folds,
        });
        self
    }

    /// Canonicalizes `prefix`, a mount prefix or store root (`what`),
    /// and checks that no mount sits there yet.
    #[track_caller]
    fn free_prefix(&self, prefix: &str, what: &str) -> VfsPath {
        let canonical = match canonicalize_absolute(prefix) {
            Ok(canonical) => canonical,
            Err(err) => panic!("invalid {what} {prefix:?}: {err}"),
        };
        assert!(
            !self.mounts.contains_key(&canonical.to_buf()),
            "a mount already sits at {prefix:?}"
        );
        canonical
    }

    /// Installs the policy consulted on every operation, consuming and
    /// returning the builder. The default is [`AllowAll`].
    ///
    /// [`AllowAll`]: crate::AllowAll
    #[must_use]
    pub fn policy(mut self, policy: impl Policy + Sync + 'static) -> VfsRefBuilder {
        self.policy = Some(Arc::new(policy));
        self
    }

    /// Installs a callback that observes operations, consuming and
    /// returning the builder.
    ///
    /// The built handle calls this operation sink just before the
    /// backend runs each operation that passes the policy and claims
    /// checks. Only those operations reach the sink. Each call receives
    /// an `OpEvent` with the operation kind, the canonical path, and the
    /// `Origin` of the access that made the call. The sink only
    /// observes: it returns `()`, and the operation's result stays
    /// hidden from it.
    ///
    /// The sink runs inline on the thread that performs the operation,
    /// so it must be cheap.
    #[must_use]
    pub fn on_op(mut self, sink: impl Fn(OpEvent<'_>) + Send + Sync + 'static) -> VfsRefBuilder {
        self.sink = Some(Arc::new(sink));
        self
    }

    /// Builds the handle with the installed mounts, policy, and
    /// operation sink.
    ///
    /// The mount table is fixed from this point on. The policy defaults
    /// to [`AllowAll`], and the handle has an operation sink only when
    /// `on_op` installed one.
    ///
    /// [`AllowAll`]: crate::AllowAll
    #[must_use]
    pub fn build(self) -> VfsRef {
        let policy = self.policy.unwrap_or_else(|| Arc::new(AllowAll));
        VfsRef::from_router(
            Router::new(self.mounts),
            policy,
            self.sink,
            self.store,
            self.folds,
        )
    }
}

#[cfg(test)]
#[path = "router-tests.rs"]
mod tests;
