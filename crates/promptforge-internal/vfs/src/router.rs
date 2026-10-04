//! The mount table and its routing access.
//!
//! A `Router` is itself a [`Vfs`], so routers nest; the public concept
//! is "a `VfsRef` with these mounts," expressed through
//! [`VfsRefBuilder`]. Mounts are fixed at build, so the table is
//! immutable and cheap to `Arc`-share. The routing access resolves the
//! longest-prefix mount per operation, strips the mount prefix so each
//! backend sees a rooted path within its own mount, and acquires each
//! backend's access lazily on first touch.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::VfsError;
use crate::handle::VfsRef;
use crate::observe::{OpEvent, OpSink};
use crate::path::{VfsPath, VfsPathBuf, canonicalize_absolute};
use crate::stat::{Entry, Stat};
use crate::traits::{AcquireContext, AllowAll, ExecId, Policy, Vfs, VfsAccess};

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
fn mount_matches(prefix: &str, path: &str) -> bool {
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
fn rejoin(prefix: &str, path: &str) -> String {
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
        // The routing access's Drop releases the identity at every
        // touched mount; the router itself holds no per-identity state.
        let _ = id;
        Ok(())
    }

    // read_only() keeps the default: a router mixes mounts, and the
    // routing access enforces each mount's own flag per operation.
}

/// One identity's session with the router. Resolves the longest-prefix
/// mount per operation and acquires each backend's access lazily on
/// first touch of that mount.
struct RoutingAccess {
    /// The caller's context, passed to each mount's lazy acquire.
    cx: AcquireContext,
    mounts: Arc<Mounts>,
    /// Per-mount sessions, keyed by mount prefix. `RefCell` because
    /// read-only trait methods take `&self`; the enclosing capability
    /// serializes every call, so the cell is never contended.
    acquired: RefCell<BTreeMap<VfsPathBuf, Box<dyn VfsAccess>>>,
}

impl RoutingAccess {
    /// Runs `op` against the session of the mount serving `path`,
    /// acquiring that session on first touch.
    fn with_mount<R>(
        &self,
        path: &VfsPath,
        op: impl FnOnce(&mut dyn VfsAccess) -> Result<R, VfsError>,
    ) -> Result<R, VfsError> {
        let (prefix, backend) =
            resolve(&self.mounts, path.as_str()).ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })?;
        let mut acquired = self.acquired.borrow_mut();
        if !acquired.contains_key(prefix) {
            let session = lock(backend).acquire(&self.cx)?;
            acquired.insert(prefix.clone(), session);
        }
        let Some(session) = acquired.get_mut(prefix) else {
            unreachable!("the session was just acquired")
        };
        op(session.as_mut())
    }

    /// The mount-relative path the serving backend sees.
    fn strip(&self, path: &VfsPath) -> Result<VfsPath, VfsError> {
        let (prefix, _) =
            resolve(&self.mounts, path.as_str()).ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })?;
        canonicalize_absolute(strip_mount(prefix.as_str(), path.as_str()))
    }

    /// Rejects mutations on read-only mounts before anything is
    /// touched: a denied operation never partially applies.
    fn check_writable(&self, path: &VfsPath) -> Result<(), VfsError> {
        let (prefix, backend) =
            resolve(&self.mounts, path.as_str()).ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })?;
        if lock(backend).read_only() {
            return Err(VfsError::PermissionDenied {
                path: path.to_string(),
                reason: format!("the mount at {prefix} is read-only, so {path} cannot be mutated"),
            });
        }
        Ok(())
    }

    /// Two-path operations require one mount: backend atomicity
    /// guarantees stop at the mount boundary.
    fn one_mount(&self, from: &VfsPath, to: &VfsPath, op: &str) -> Result<(), VfsError> {
        let from_prefix = resolve(&self.mounts, from.as_str())
            .ok_or_else(|| VfsError::NotFound {
                path: from.to_string(),
            })?
            .0;
        let to_prefix = resolve(&self.mounts, to.as_str())
            .ok_or_else(|| VfsError::NotFound {
                path: to.to_string(),
            })?
            .0;
        if from_prefix != to_prefix {
            return Err(VfsError::Unsupported {
                path: from.to_string(),
                detail: format!(
                    "{op} across mounts is unsupported: {from} and {to} are served by different mounts"
                ),
            });
        }
        Ok(())
    }
}

impl VfsAccess for RoutingAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.read(&stripped))
    }

    fn read_range(&self, path: &VfsPath, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        // Delegated, not defaulted, so backends that can seek never
        // materialize the file.
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.read_range(&stripped, offset, len))
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.write(&stripped, contents))
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.append(&stripped, contents))
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.remove(&stripped, recursive))
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.exists(&stripped))
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        // Patterns are not paths, but they route like one:
        // canonicalizing resolves dot segments and rejects escapes past
        // the namespace root; wildcards are ordinary segments.
        let canonical = canonicalize_absolute(pattern)?;
        let (prefix, _) =
            resolve(&self.mounts, canonical.as_str()).ok_or_else(|| VfsError::NotFound {
                path: canonical.to_string(),
            })?;
        let scoped = strip_mount(prefix.as_str(), canonical.as_str()).to_owned();
        let mut matches = self.with_mount(&canonical, |session| session.glob(&scoped))?;
        for path in &mut matches {
            *path = rejoin(prefix.as_str(), path);
        }
        Ok(matches)
    }

    fn glob_kind(&self, pattern: &str, dirs_only: bool) -> Result<Vec<String>, VfsError> {
        // Mirrors `glob`: the dirs-only flag must survive the mount
        // boundary. The trait default re-splits inside the serving
        // session with the already-stripped pattern, which loses the
        // flag and filters the files-only result down to nothing.
        let canonical = canonicalize_absolute(pattern)?;
        let (prefix, _) =
            resolve(&self.mounts, canonical.as_str()).ok_or_else(|| VfsError::NotFound {
                path: canonical.to_string(),
            })?;
        let scoped = strip_mount(prefix.as_str(), canonical.as_str()).to_owned();
        let mut matches =
            self.with_mount(&canonical, |session| session.glob_kind(&scoped, dirs_only))?;
        for path in &mut matches {
            *path = rejoin(prefix.as_str(), path);
        }
        Ok(matches)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.list(&stripped))
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.stat(&stripped))
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.mkdir(&stripped, recursive))
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.check_writable(from)?;
        self.check_writable(to)?;
        self.one_mount(from, to, "rename")?;
        let from_stripped = self.strip(from)?;
        let to_stripped = self.strip(to)?;
        self.with_mount(from, |session| session.rename(&from_stripped, &to_stripped))
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.check_writable(to)?;
        self.one_mount(from, to, "copy")?;
        let from_stripped = self.strip(from)?;
        let to_stripped = self.strip(to)?;
        self.with_mount(from, |session| session.copy(&from_stripped, &to_stripped))
    }

    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str) -> Result<(), VfsError> {
        // Delegated so backends can push down; the read-only check here
        // covers the backend's default read-plus-write as well.
        self.check_writable(path)?;
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.str_replace(&stripped, old, new))
    }

    fn symlink(&mut self, target: &VfsPath, link: &VfsPath) -> Result<(), VfsError> {
        self.check_writable(link)?;
        let stripped = self.strip(link)?;
        // The target is a stored name, not resolved: it passes verbatim.
        self.with_mount(link, |session| session.symlink(target, &stripped))
    }

    fn read_link(&self, path: &VfsPath) -> Result<VfsPathBuf, VfsError> {
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.read_link(&stripped))
    }

    fn chmod(&mut self, path: &VfsPath, mode: u32) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let stripped = self.strip(path)?;
        self.with_mount(path, |session| session.chmod(&stripped, mode))
    }
}

impl Drop for RoutingAccess {
    fn drop(&mut self) {
        // Release the identity at every touched mount. Sessions drop
        // first: a mounted handle's session releases through its own
        // capability's Drop, and the backend-level release below is its
        // no-op counterpart. Plain backends get their one release here.
        for (prefix, session) in std::mem::take(self.acquired.get_mut()) {
            drop(session);
            if let Some(backend) = self.mounts.get(&prefix) {
                let _ = lock(backend).release(self.cx.id());
            }
        }
    }
}

/// A builder that mounts backends at path prefixes and then builds a
/// [`VfsRef`] over them.
///
/// The mounts are fixed when [`VfsRefBuilder::build`] runs. After that
/// the mount table never changes, so it is cheap to share through an
/// `Arc`.
pub struct VfsRefBuilder {
    mounts: Mounts,
    policy: Option<Arc<dyn Policy + Sync>>,
    sink: Option<OpSink>,
    store: Option<StoreDecl>,
}

/// The handle's declared store: the mount serving the run's store,
/// when the builder declared one. `root` is the canonical store root
/// the store view joins logical paths onto; `mount` is the store's
/// backend, shared with the mount table. Only the declaration of the
/// handle a builder builds counts: an overlay inherits its base's,
/// while a handle mounted as a backend keeps its declaration to
/// itself.
#[derive(Clone)]
pub(crate) struct StoreDecl {
    pub(crate) root: VfsPath,
    pub(crate) mount: Mounted,
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
    pub fn mount(mut self, prefix: &str, backend: impl Vfs + 'static) -> VfsRefBuilder {
        let canonical = canonicalize_absolute(prefix)
            .unwrap_or_else(|err| panic!("invalid mount prefix {prefix:?}: {err}"));
        let key = canonical.to_buf();
        assert!(
            !self.mounts.contains_key(&key),
            "a mount already sits at {prefix:?}"
        );
        self.mounts
            .insert(key, Arc::new(Mutex::new(Box::new(backend))));
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
    pub fn store(mut self, root: &str, backend: impl Vfs + 'static) -> VfsRefBuilder {
        let canonical = canonicalize_absolute(root)
            .unwrap_or_else(|err| panic!("invalid store root {root:?}: {err}"));
        let key = canonical.to_buf();
        assert!(
            !self.mounts.contains_key(&key),
            "a mount already sits at {root:?}"
        );
        let mount: Mounted = Arc::new(Mutex::new(Box::new(backend)));
        self.mounts.insert(key, Arc::clone(&mount));
        self.store = Some(StoreDecl {
            root: canonical,
            mount,
        });
        self
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
    /// The built handle calls this operation sink for every operation
    /// that passes the policy and claims checks, just before the backend
    /// runs it. Each call receives an `OpEvent` with the operation kind,
    /// the canonical path, and the `Origin` of the access that made the
    /// call. The sink returns nothing and never learns the operation's
    /// result. An operation that the policy denies never reaches it.
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
    /// to [`AllowAll`], and the handle has no operation sink unless
    /// `on_op` installed one.
    ///
    /// [`AllowAll`]: crate::AllowAll
    #[must_use]
    pub fn build(self) -> VfsRef {
        let policy = self.policy.unwrap_or_else(|| Arc::new(AllowAll));
        VfsRef::from_router(Router::new(self.mounts), policy, self.sink, self.store)
    }
}

#[cfg(test)]
#[path = "router-tests.rs"]
mod tests;
