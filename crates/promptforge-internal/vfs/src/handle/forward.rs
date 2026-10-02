//! The forwarding wrappers: a handle mounted as another handle's
//! backend, and the store view's mount and sessions, each forwarding
//! every operation and re-spelling error paths where they are produced.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::store_view::{relativize_error, trim_mount_error};
use super::{Access, VfsRef};
use crate::error::VfsError;
use crate::path::{VfsPath, VfsPathBuf};
use crate::stat::{Entry, Stat};
use crate::traits::{AcquireContext, ExecId, Vfs, VfsAccess};

/// A handle is itself a backend: mounting a base handle under a child
/// router - which is how [`VfsRef::overlay`] shares one claims table
/// across two views of the same storage - routes operations through the
/// base's policy and claims under the caller's identity.
impl Vfs for VfsRef {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        // The mount forward leaves the origin unset: the outer handle already
        // fired the caller's, and a fabricated one here would double the
        // event with a less precise label.
        Ok(Box::new(HandleAccess(self.acquire_with(cx, None)?)))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        // The vended session's Drop drops the identity's reference;
        // nothing is registered at this level.
        let _ = id;
        Ok(())
    }

    fn read_only(&self) -> bool {
        self.backend().read_only()
    }
}

/// The session vended by a mounted handle: forwards every operation
/// through the base handle's capability, so its policy and claims apply
/// under the caller's identity. Paths arrive canonical, so the
/// capability's canonicalization at receipt is an idempotent re-check.
///
/// Byte-range reads and the POSIX extras keep their trait defaults: the
/// public capability exposes neither, so there is nothing to forward to.
struct HandleAccess(Access);

impl VfsAccess for HandleAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.0.read(path.as_str())
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.0.write(path.as_str(), contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.0.append(path.as_str(), contents)
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        // The backend trait's contract reports an absent path as
        // NotFound; the capability's Ok(false) maps back onto it.
        if self.0.remove(path.as_str(), recursive)? {
            Ok(())
        } else {
            Err(VfsError::NotFound {
                path: path.to_string(),
            })
        }
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.0.exists(path.as_str())
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.0.glob(pattern)
    }

    fn glob_kind(&self, pattern: &str, dirs_only: bool) -> Result<Vec<String>, VfsError> {
        // The forward re-enters the full public pipeline with the flag
        // intact, so the dirs-only split survives the handle boundary.
        self.0.glob_pattern(pattern, dirs_only)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.0.list(path.as_str())
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.0.stat(path.as_str())
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.0.mkdir(path.as_str(), recursive)
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.rename(from.as_str(), to.as_str())
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.copy(from.as_str(), to.as_str())
    }

    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str) -> Result<(), VfsError> {
        self.0.str_replace(path.as_str(), old, new)
    }
}

/// The store's mount as the store view's router sees it: the declared
/// mount, whose sessions report error paths in the logical form. The
/// backend names mount-relative paths, which differ from the canonical
/// ones the router's own refusals name unless the store sits at `/`,
/// so each spelling is re-spelled where it is produced.
pub(super) struct StoreMount(pub(super) Arc<Mutex<Box<dyn Vfs>>>);

impl StoreMount {
    fn lock(&self) -> MutexGuard<'_, Box<dyn Vfs>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Vfs for StoreMount {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        let inner = self.lock().acquire(cx).map_err(trim_mount_error)?;
        Ok(Box::new(StoreMountSession(inner)))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        self.lock().release(id).map_err(trim_mount_error)
    }

    fn read_only(&self) -> bool {
        self.lock().read_only()
    }
}

/// A store mount session: forwards every operation, trimming its
/// error paths into the logical form.
struct StoreMountSession(Box<dyn VfsAccess>);

impl VfsAccess for StoreMountSession {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.0.read(path).map_err(trim_mount_error)
    }

    fn read_range(&self, path: &VfsPath, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        self.0
            .read_range(path, offset, len)
            .map_err(trim_mount_error)
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.0.write(path, contents).map_err(trim_mount_error)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.0.append(path, contents).map_err(trim_mount_error)
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.0.remove(path, recursive).map_err(trim_mount_error)
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.0.exists(path).map_err(trim_mount_error)
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.0.glob(pattern).map_err(trim_mount_error)
    }

    fn glob_kind(&self, pattern: &str, dirs_only: bool) -> Result<Vec<String>, VfsError> {
        self.0
            .glob_kind(pattern, dirs_only)
            .map_err(trim_mount_error)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.0.list(path).map_err(trim_mount_error)
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.0.stat(path).map_err(trim_mount_error)
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.0.mkdir(path, recursive).map_err(trim_mount_error)
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.rename(from, to).map_err(trim_mount_error)
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.copy(from, to).map_err(trim_mount_error)
    }

    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str) -> Result<(), VfsError> {
        self.0.str_replace(path, old, new).map_err(trim_mount_error)
    }

    fn symlink(&mut self, target: &VfsPath, link: &VfsPath) -> Result<(), VfsError> {
        self.0.symlink(target, link).map_err(trim_mount_error)
    }

    fn read_link(&self, path: &VfsPath) -> Result<VfsPathBuf, VfsError> {
        self.0.read_link(path).map_err(trim_mount_error)
    }

    fn chmod(&mut self, path: &VfsPath, mode: u32) -> Result<(), VfsError> {
        self.0.chmod(path, mode).map_err(trim_mount_error)
    }
}

/// The store view's backend session: the store's own mount behind a
/// one-mount router. The mount's errors arrive in the logical form;
/// the router's own refusals name canonical paths, re-spelled here.
pub(super) struct StoreScoped {
    /// The declared store root, for re-spelling error paths.
    pub(super) root: String,
    pub(super) inner: Box<dyn VfsAccess>,
}

impl StoreScoped {
    /// Re-spells one error into the caller's logical form.
    fn logical(&self, err: VfsError) -> VfsError {
        relativize_error(err, &self.root)
    }
}

impl VfsAccess for StoreScoped {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.inner.read(path).map_err(|err| self.logical(err))
    }

    fn read_range(&self, path: &VfsPath, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        self.inner
            .read_range(path, offset, len)
            .map_err(|err| self.logical(err))
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.inner
            .write(path, contents)
            .map_err(|err| self.logical(err))
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.inner
            .append(path, contents)
            .map_err(|err| self.logical(err))
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.inner
            .remove(path, recursive)
            .map_err(|err| self.logical(err))
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.inner.exists(path).map_err(|err| self.logical(err))
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.inner.glob(pattern).map_err(|err| self.logical(err))
    }

    fn glob_kind(&self, pattern: &str, dirs_only: bool) -> Result<Vec<String>, VfsError> {
        self.inner
            .glob_kind(pattern, dirs_only)
            .map_err(|err| self.logical(err))
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.inner.list(path).map_err(|err| self.logical(err))
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.inner.stat(path).map_err(|err| self.logical(err))
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.inner
            .mkdir(path, recursive)
            .map_err(|err| self.logical(err))
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.rename(from, to).map_err(|err| self.logical(err))
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.copy(from, to).map_err(|err| self.logical(err))
    }

    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str) -> Result<(), VfsError> {
        self.inner
            .str_replace(path, old, new)
            .map_err(|err| self.logical(err))
    }

    fn symlink(&mut self, target: &VfsPath, link: &VfsPath) -> Result<(), VfsError> {
        self.inner
            .symlink(target, link)
            .map_err(|err| self.logical(err))
    }

    fn read_link(&self, path: &VfsPath) -> Result<VfsPathBuf, VfsError> {
        self.inner.read_link(path).map_err(|err| self.logical(err))
    }

    fn chmod(&mut self, path: &VfsPath, mode: u32) -> Result<(), VfsError> {
        self.inner
            .chmod(path, mode)
            .map_err(|err| self.logical(err))
    }
}
