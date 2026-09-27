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
use crate::grep::{GrepQuery, GrepResults};
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

    fn grep(&self, query: &GrepQuery) -> Result<GrepResults, VfsError> {
        let root = canonicalize_absolute(query.root.as_str())?;
        let (prefix, _) =
            resolve(&self.mounts, root.as_str()).ok_or_else(|| VfsError::NotFound {
                path: root.to_string(),
            })?;
        let mut scoped = query.clone();
        scoped.root = canonicalize_absolute(strip_mount(prefix.as_str(), root.as_str()))?.to_buf();
        let mut results = self.with_mount(&root, |session| session.grep(&scoped))?;
        for hit in &mut results.matches {
            hit.path = rejoin(prefix.as_str(), &hit.path);
        }
        Ok(results)
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

/// Mount installation for [`VfsRef`]. Mounts are fixed at
/// [`VfsRefBuilder::build`], so the table is immutable and cheap to
/// `Arc`-share thereafter.
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
    /// builder. The root prefix `/` serves the whole namespace; a
    /// longer prefix shadows a shorter one (longest prefix wins).
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
    /// store, consuming and returning the builder. The store is the
    /// mount every store call is scoped to. Only the outermost built
    /// handle's declaration counts: an overlay inherits its base's,
    /// and a handle mounted as a backend keeps its declaration to
    /// itself.
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

    /// Installs the operation sink, consuming and returning the builder.
    /// The sink fires on every admitted operation - after policy and
    /// claims pass, before the backend executes - with the op kind, the
    /// canonical path, and the caller's origin. Fire-and-forget: no
    /// outcome flows back, and a policy-denied operation never fires.
    /// The sink must be cheap: store operations fire it from the
    /// blocking pool.
    #[must_use]
    pub fn on_op(mut self, sink: impl Fn(OpEvent<'_>) + Send + Sync + 'static) -> VfsRefBuilder {
        self.sink = Some(Arc::new(sink));
        self
    }

    /// Freezes the mount table into a handle with the installed policy
    /// and op sink (the [`AllowAll`] policy and no sink by default).
    ///
    /// [`AllowAll`]: crate::AllowAll
    #[must_use]
    pub fn build(self) -> VfsRef {
        let policy = self.policy.unwrap_or_else(|| Arc::new(AllowAll));
        VfsRef::from_router(Router::new(self.mounts), policy, self.sink, self.store)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use super::{Mounts, Router};
    use crate::error::VfsError;
    use crate::handle::{Scope, VfsRef};
    use crate::memory::MemoryBackend;
    use crate::observe::Origin;
    use crate::path::{VfsPath, canonicalize_absolute};
    use crate::stat::{Entry, FileType, Stat};
    use crate::traits::{AcquireContext, ExecId, Vfs, VfsAccess};

    /// A recording in-memory stub. Files are keyed by the exact paths
    /// the backend is handed, so tests observe prefix stripping
    /// directly; every served path is recorded; the acquire count pins
    /// lazy per-mount acquisition.
    #[derive(Clone, Default)]
    struct StubFs {
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        seen: Arc<Mutex<Vec<String>>>,
        acquires: Arc<Mutex<usize>>,
        read_only: bool,
    }

    impl StubFs {
        fn seeded(files: &[(&str, &str)]) -> StubFs {
            let stub = StubFs::default();
            for (name, text) in files {
                stub.files()
                    .insert((*name).to_owned(), text.as_bytes().to_vec());
            }
            stub
        }

        fn with_read_only(mut self) -> StubFs {
            self.read_only = true;
            self
        }

        fn files(&self) -> MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
            self.files.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn seen(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn acquire_count(&self) -> usize {
            *self.acquires.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Vfs for StubFs {
        fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
            let _ = cx;
            *self.acquires.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            Ok(Box::new(StubAccess {
                files: Arc::clone(&self.files),
                seen: Arc::clone(&self.seen),
            }))
        }

        fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
            let _ = id;
            Ok(())
        }

        fn read_only(&self) -> bool {
            self.read_only
        }
    }

    struct StubAccess {
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl StubAccess {
        fn files(&self) -> MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
            self.files.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn record(&self, path: &VfsPath) {
            self.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(path.to_string());
        }
    }

    impl VfsAccess for StubAccess {
        fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
            self.record(path);
            self.files()
                .get(path.as_str())
                .cloned()
                .ok_or_else(|| VfsError::NotFound {
                    path: path.to_string(),
                })
        }

        fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
            self.record(path);
            self.files().insert(path.to_string(), contents.to_vec());
            Ok(())
        }

        fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
            self.record(path);
            self.files()
                .entry(path.to_string())
                .or_default()
                .extend_from_slice(contents);
            Ok(())
        }

        fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
            let _ = recursive;
            self.record(path);
            self.files()
                .remove(path.as_str())
                .map(|_| ())
                .ok_or_else(|| VfsError::NotFound {
                    path: path.to_string(),
                })
        }

        fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
            self.record(path);
            Ok(self.files().contains_key(path.as_str()))
        }

        fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
            let prefix = pattern.split('*').next().unwrap_or(pattern);
            let mut matches: Vec<String> = self
                .files()
                .keys()
                .filter(|name| name.starts_with(prefix))
                .cloned()
                .collect();
            matches.sort();
            Ok(matches)
        }

        fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
            Err(VfsError::Unsupported {
                path: path.to_string(),
                detail: "the stub does not list".into(),
            })
        }

        fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
            // Every stored key is a file; the stub holds no directories.
            let bytes =
                self.files()
                    .get(path.as_str())
                    .cloned()
                    .ok_or_else(|| VfsError::NotFound {
                        path: path.to_string(),
                    })?;
            Ok(Stat {
                file_type: FileType::File,
                size: bytes.len() as u64,
                mode: None,
                modified: None,
                created: None,
            })
        }

        fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
            let _ = (path, recursive);
            Ok(())
        }

        fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
            self.record(from);
            self.record(to);
            let bytes = self
                .files()
                .remove(from.as_str())
                .ok_or_else(|| VfsError::NotFound {
                    path: from.to_string(),
                })?;
            self.files().insert(to.to_string(), bytes);
            Ok(())
        }

        fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
            self.record(from);
            self.record(to);
            let bytes =
                self.files()
                    .get(from.as_str())
                    .cloned()
                    .ok_or_else(|| VfsError::NotFound {
                        path: from.to_string(),
                    })?;
            self.files().insert(to.to_string(), bytes);
            Ok(())
        }
    }

    /// The routing tests ignore origins, so they acquire under one
    /// blanket label.
    fn test_origin() -> Origin {
        Origin::new("router test")
    }

    #[test]
    fn the_longest_prefix_mount_wins_and_acquires_lazily() -> Result<(), VfsError> {
        let outer = StubFs::default();
        let inner = StubFs::default();
        let untouched = StubFs::default();
        let vfs = VfsRef::builder()
            .mount("/a", outer.clone())
            .mount("/a/b", inner.clone())
            .mount("/elsewhere", untouched.clone())
            .build();
        let access = vfs.acquire(test_origin())?;
        access.write("/a/b/f.txt", b"inner")?;
        access.write("/a/f.txt", b"outer")?;
        // Each backend keyed the file by its mount-relative path.
        assert!(inner.files().contains_key("/f.txt"));
        assert!(outer.files().contains_key("/f.txt"));
        assert_eq!(access.read("/a/b/f.txt")?, b"inner");
        assert_eq!(access.read("/a/f.txt")?, b"outer");
        // Lazy per-mount acquire: the untouched mount never opened one.
        assert_eq!(inner.acquire_count(), 1);
        assert_eq!(outer.acquire_count(), 1);
        assert_eq!(untouched.acquire_count(), 0);
        Ok(())
    }

    #[test]
    fn a_longer_mount_shadows_the_same_prefix_of_a_shorter_one() -> Result<(), VfsError> {
        let base = StubFs::seeded(&[("/f.txt", "base-f"), ("/mnt/f.txt", "base-mnt")]);
        let shadow = StubFs::seeded(&[("/f.txt", "shadow-mnt")]);
        let vfs = VfsRef::builder()
            .mount("/", base.clone())
            .mount("/mnt", shadow.clone())
            .build();
        let access = vfs.acquire(test_origin())?;
        // The shadow mount owns everything under /mnt.
        assert_eq!(access.read("/mnt/f.txt")?, b"shadow-mnt");
        // The base still owns the rest of the namespace.
        assert_eq!(access.read("/f.txt")?, b"base-f");
        // The base's own /mnt/f.txt is unreachable through the handle.
        assert_eq!(
            base.files().get("/mnt/f.txt").map(Vec::as_slice),
            Some(b"base-mnt".as_slice())
        );
        Ok(())
    }

    #[test]
    fn a_mounted_handle_applies_its_own_claims_under_the_callers_identity() -> Result<(), VfsError>
    {
        // Nesting: a base handle mounted under a child router.
        let base_storage = StubFs::default();
        let base = VfsRef::new(base_storage.clone());
        let local = StubFs::default();
        let child = VfsRef::builder()
            .mount("/base", base.clone())
            .mount("/local", local.clone())
            .build();
        let writer = child.acquire(test_origin())?;
        writer.write("/base/f.txt", b"nested")?;
        // The child router stripped its mount prefix: the base backend
        // keyed the file at its own root.
        assert!(base_storage.files().contains_key("/f.txt"));
        // A second child identity conflicts on the same path: the claim
        // registered through the mounted handle is visible.
        let reader = child.acquire(test_origin())?;
        match reader.read("/base/f.txt") {
            Err(VfsError::Conflict { .. }) => {}
            other => panic!("expected a conflict, got {other:?}"),
        }
        // The local mount routes to its own backend.
        writer.write("/local/g.txt", b"local")?;
        assert!(local.files().contains_key("/g.txt"));
        Ok(())
    }

    #[test]
    fn writes_to_a_read_only_mount_are_denied_without_partial_application() -> Result<(), VfsError>
    {
        let ro = StubFs::seeded(&[("/a.txt", "keep")]).with_read_only();
        let rw = StubFs::default();
        let vfs = VfsRef::builder()
            .mount("/", rw.clone())
            .mount("/ro", ro.clone())
            .build();
        let access = vfs.acquire(test_origin())?;
        // Reads are not gated.
        assert_eq!(access.read("/ro/a.txt")?, b"keep");
        // A write is denied with a clear read-only error.
        match access.write("/ro/new.txt", b"x") {
            Err(VfsError::PermissionDenied { reason, .. }) => {
                assert!(reason.contains("read-only"), "names the cause: {reason}");
            }
            other => panic!("expected a read-only denial, got {other:?}"),
        }
        assert!(!ro.files().contains_key("/new.txt"));
        // A rename wholly inside the mount is denied before the source
        // is touched.
        match access.rename("/ro/a.txt", "/ro/b.txt") {
            Err(VfsError::PermissionDenied { .. }) => {}
            other => panic!("expected a read-only denial, got {other:?}"),
        }
        assert_eq!(access.read("/ro/a.txt")?, b"keep");
        assert!(!ro.files().contains_key("/b.txt"));
        // A copy whose destination is read-only is denied; the source
        // is untouched.
        access.write("/x.txt", b"data")?;
        match access.copy("/x.txt", "/ro/x.txt") {
            Err(VfsError::PermissionDenied { .. }) => {}
            other => panic!("expected a read-only denial, got {other:?}"),
        }
        assert!(!ro.files().contains_key("/x.txt"));
        assert_eq!(access.read("/x.txt")?, b"data");
        Ok(())
    }

    #[test]
    fn traversal_that_escapes_the_namespace_root_is_rejected() -> Result<(), VfsError> {
        let vfs = VfsRef::builder().mount("/mnt", StubFs::default()).build();
        let access = vfs.acquire(test_origin())?;
        assert!(matches!(
            access.read("/mnt/../../etc/passwd"),
            Err(VfsError::InvalidPath { .. })
        ));
        assert!(matches!(
            access.glob("/mnt/../../*"),
            Err(VfsError::InvalidPath { .. })
        ));
        Ok(())
    }

    #[test]
    fn a_path_that_climbs_out_of_its_mount_is_not_served_by_that_mount() -> Result<(), VfsError> {
        // No root mount: the only storage lives at /mnt.
        let storage = StubFs::seeded(&[("/f.txt", "inside")]);
        let vfs = VfsRef::builder().mount("/mnt", storage.clone()).build();
        let access = vfs.acquire(test_origin())?;
        // Dot segments within the mount resolve within the mount: the
        // backend sees the clean mount-relative path.
        assert_eq!(access.read("/mnt/sub/../f.txt")?, b"inside");
        assert!(storage.seen().contains(&"/f.txt".to_owned()));
        // A path that climbs out of the mount re-roots absolutely: it
        // canonicalizes to /secret.txt, no mount serves it, and the
        // mount's backend never sees the traversal spelling. The error's
        // path field names that unrouted path, not a routing sentence.
        match access.read("/mnt/../secret.txt") {
            Err(VfsError::NotFound { path }) => {
                assert_eq!(path, "/secret.txt");
            }
            other => panic!("expected no serving mount, got {other:?}"),
        }
        assert!(
            storage.seen().iter().all(|path| !path.contains("..")),
            "the backend never sees a traversal: {:?}",
            storage.seen()
        );
        Ok(())
    }

    #[test]
    fn glob_routes_to_the_serving_mount_and_restores_the_prefix() -> Result<(), VfsError> {
        let inner = StubFs::seeded(&[("/x.txt", "x")]);
        let vfs = VfsRef::builder()
            .mount("/a", StubFs::default())
            .mount("/a/b", inner.clone())
            .build();
        let access = vfs.acquire(test_origin())?;
        let matches = access.glob("/a/b/*.txt")?;
        assert_eq!(matches, vec!["/a/b/x.txt".to_owned()]);
        Ok(())
    }

    #[test]
    fn a_directory_only_glob_keeps_its_flag_across_a_mounted_handle() -> Result<(), VfsError> {
        // The overlay's base is a mounted handle: the dirs-only flag must
        // survive the mount forward, or the base's capability re-splits
        // the already-stripped pattern as files-only and the directories
        // vanish from the result.
        let base = VfsRef::builder().mount("/x", MemoryBackend::new()).build();
        let overlay = base.overlay("/overlay", StubFs::default());
        let access = overlay.acquire(test_origin())?;
        access.write("/x/d/a.txt", b"x")?;
        access.write("/x/d/sub/b.txt", b"x")?;
        assert_eq!(access.glob("x/d/*/")?, vec!["x/d/sub".to_owned()]);
        assert_eq!(access.glob("x/d/*")?, vec!["x/d/a.txt".to_owned()]);
        Ok(())
    }

    #[test]
    fn removing_an_absent_path_through_a_mounted_handle_reports_not_found() -> Result<(), VfsError>
    {
        // The mount-forward chain at the backend level: the router's
        // session resolves the mount, and the mounted handle's forward
        // maps the capability's Ok(false) back onto the backend trait's
        // NotFound spelling.
        let base = VfsRef::new(StubFs::default());
        let mut mounts = Mounts::new();
        mounts.insert(
            canonicalize_absolute("/base")?.to_buf(),
            Arc::new(Mutex::new(Box::new(base.clone()))),
        );
        let mut router = Router::new(mounts);
        let mut session = router.acquire(&AcquireContext::new(ExecId::vend(), Scope::start()))?;
        assert!(matches!(
            session.remove(&canonicalize_absolute("/base/missing.txt")?, false),
            Err(VfsError::NotFound { .. })
        ));
        Ok(())
    }

    #[test]
    fn one_handle_serves_several_mounts_and_an_overlay_simultaneously() -> Result<(), VfsError> {
        let store = StubFs::default();
        let scratch = StubFs::default();
        let extra = StubFs::default();
        let base = VfsRef::builder()
            .mount("/store", store.clone())
            .mount("/scratch", scratch.clone())
            .build();
        let overlay = base.overlay("/overlay", extra.clone());

        let writer = overlay.acquire(test_origin())?;
        writer.write("/store/doc.md", b"store")?;
        writer.write("/scratch/tmp.txt", b"scratch")?;
        writer.write("/overlay/x.txt", b"overlay")?;
        // Dropping releases the writer's claims into the shared table.
        drop(writer);

        // The base handle serves its own mounts from the same storage.
        let reader = base.acquire(test_origin())?;
        assert_eq!(reader.read("/store/doc.md")?, b"store");
        assert_eq!(reader.read("/scratch/tmp.txt")?, b"scratch");
        // The overlay mount exists only in the overlay's view.
        assert!(matches!(
            reader.read("/overlay/x.txt"),
            Err(VfsError::NotFound { .. })
        ));
        Ok(())
    }

    #[test]
    fn an_overlay_shares_the_bases_claims_table() -> Result<(), VfsError> {
        let base = VfsRef::builder().mount("/store", StubFs::default()).build();
        let overlay = base.overlay("/overlay", StubFs::default());
        let first = base.acquire(test_origin())?;
        first.write("/store/shared.txt", b"1")?;
        // A write claim registered through the base conflicts with a
        // write attempted through the overlay: one claims table.
        let second = overlay.acquire(test_origin())?;
        match second.write("/store/shared.txt", b"2") {
            Err(VfsError::Conflict { detail, .. }) => {
                assert!(detail.contains("/store/shared.txt"), "{detail}");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    #[should_panic(expected = "invalid mount prefix")]
    fn the_builder_rejects_a_relative_mount_prefix() {
        let _ = VfsRef::builder().mount("relative", StubFs::default());
    }

    #[test]
    #[should_panic(expected = "invalid store root")]
    fn the_builder_rejects_a_relative_store_root() {
        let _ = VfsRef::builder().store("relative", StubFs::default());
    }

    #[test]
    #[should_panic(expected = "a mount already sits at")]
    fn the_builder_rejects_a_store_root_that_already_has_a_mount() {
        let _ = VfsRef::builder()
            .mount("/s", StubFs::default())
            .store("/s", StubFs::default());
    }
}
