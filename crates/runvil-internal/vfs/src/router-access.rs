//! The routing access: one identity's session with the router, which
//! resolves the longest-prefix mount per operation and acquires each
//! backend's session lazily on first touch.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Mounts, lock, rejoin, resolve, strip_mount};
use crate::error::VfsError;
use crate::path::{VfsPath, VfsPathBuf, canonicalize_absolute};
use crate::stat::{Entry, Stat};
use crate::traits::{AcquireContext, VfsAccess};

/// One identity's session with the router. Resolves the longest-prefix
/// mount per operation and acquires each backend's access lazily on
/// first touch of that mount.
pub(super) struct RoutingAccess {
    /// The caller's context, passed to each mount's lazy acquire.
    pub(super) cx: AcquireContext,
    pub(super) mounts: Arc<Mounts>,
    /// Per-mount sessions, keyed by mount prefix. `RefCell` because
    /// read-only trait methods take `&self`; the enclosing capability
    /// serializes every call, so the cell is never contended.
    pub(super) acquired: RefCell<BTreeMap<VfsPathBuf, Box<dyn VfsAccess>>>,
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
        let (_, backend) =
            resolve(&self.mounts, path.as_str()).ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })?;
        if lock(backend).read_only() {
            return Err(VfsError::ReadOnly {
                path: path.to_string(),
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
            return Err(VfsError::CrossMount {
                path: from.to_string(),
                detail: format!(
                    "{op} needs one mount, but {from} is served by the mount at {from_prefix} \
                     and {to} by the mount at {to_prefix}"
                ),
            });
        }
        Ok(())
    }

    /// Runs a glob through the mount serving `pattern` and restores the
    /// mount prefix on every match.
    fn glob_through(
        &self,
        pattern: &str,
        glob: impl FnOnce(&mut dyn VfsAccess, &str) -> Result<Vec<String>, VfsError>,
    ) -> Result<Vec<String>, VfsError> {
        // Patterns are not paths, but they route like one:
        // canonicalizing resolves dot segments and rejects escapes past
        // the namespace root; wildcards are ordinary segments.
        let canonical = canonicalize_absolute(pattern)?;
        let (prefix, _) =
            resolve(&self.mounts, canonical.as_str()).ok_or_else(|| VfsError::NotFound {
                path: canonical.to_string(),
            })?;
        let scoped = strip_mount(prefix.as_str(), canonical.as_str()).to_owned();
        let mut matches = self.with_mount(&canonical, |session| glob(session, &scoped))?;
        for path in &mut matches {
            *path = rejoin(prefix.as_str(), path);
        }
        Ok(matches)
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
        self.glob_through(pattern, |session, scoped| session.glob(scoped))
    }

    fn glob_kind(&self, pattern: &str, dirs_only: bool) -> Result<Vec<String>, VfsError> {
        // Mirrors `glob`: the dirs-only flag must survive the mount
        // boundary. The trait default re-splits inside the serving
        // session with the already-stripped pattern, which loses the
        // flag and filters the files-only result down to nothing.
        self.glob_through(pattern, |session, scoped| {
            session.glob_kind(scoped, dirs_only)
        })
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

    fn close(&mut self) -> Result<(), VfsError> {
        // Release the identity at every touched mount, reporting the
        // first failure. Each session closes and drops first: a mounted
        // handle's session releases through its own capability, and the
        // backend-level release after it is its no-op counterpart. Plain
        // backends get their one release here. The sessions are taken,
        // so a later drop releases nothing twice.
        let mut outcome = Ok(());
        for (prefix, mut session) in std::mem::take(self.acquired.get_mut()) {
            let closed = session.close();
            drop(session);
            let released = match self.mounts.get(&prefix) {
                Some(backend) => lock(backend).release(self.cx.id()),
                None => Ok(()),
            };
            outcome = outcome.and(closed).and(released);
        }
        outcome
    }
}

impl Drop for RoutingAccess {
    fn drop(&mut self) {
        // A session dropped without `close` still releases every
        // touched mount; the failures go unreported.
        let _ = self.close();
    }
}
