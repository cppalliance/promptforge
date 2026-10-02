//! The capability's machinery: spawning, joining, and the gate, the
//! admission, and the event every operation passes through.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::Access;
#[cfg(doc)]
use super::VfsRef;
use super::claims::Claims;
use super::forward::StoreScoped;
use super::scope::Scope;
use super::store_view::{relativize_error, validate_store_path};
use crate::error::VfsError;
use crate::observe::{OpEvent, Origin};
use crate::path::{VfsPath, canonicalize};
use crate::traits::{AcquireContext, ExecId, Op, Verdict, Vfs, VfsAccess};

impl Access {
    /// Returns the capability for a new concurrent thread of execution.
    /// The spawn is the fork: the child gets a fresh [`ExecId`] and a
    /// shared snapshot of this capability's clock plus its own entry,
    /// while this capability's entry advances, so its later accesses are
    /// not ordered before the child's. [`crate::detail::access_join`] is
    /// the matching join. `origin` labels the child's operation events,
    /// as in [`VfsRef::acquire`].
    ///
    /// # Errors
    /// Returns an error when the backend refuses to acquire the child's
    /// identity; see [`VfsRef::acquire`]. A failed spawn still advances
    /// this capability's own clock entry, which orders nothing new: its
    /// later accesses are merely no longer ordered before a child that
    /// never ran. Returns [`VfsError::PermissionDenied`], before the
    /// fork, when the run that owned this capability has ended.
    ///
    /// Crate-internal: backs [`crate::detail::access_spawn`].
    pub(crate) fn spawn(&self, origin: Origin) -> Result<Access, VfsError> {
        self.scope
            .refuse_if_closed(&self.root)
            .map_err(|err| self.relativize(err))?;
        let id = ExecId::vend();
        // The fork comes first so a wrapped handle acquiring the child
        // finds it already registered in the scope.
        self.scope.fork(self.id, id);
        let inner = match self
            .backend()
            .acquire(&AcquireContext::new(id, Arc::clone(&self.scope)))
        {
            Ok(inner) => inner,
            Err(err) => {
                self.scope.release(id);
                return Err(err);
            }
        };
        // A store view's arms keep the view's confinement to the store
        // mount and its logical error paths.
        let inner: Box<dyn VfsAccess> = match &self.store_root {
            Some(root) => Box::new(StoreScoped {
                root: root.as_str().to_owned(),
                inner,
            }),
            None => inner,
        };
        Ok(Access {
            id,
            origin: Some(origin),
            // The child shares the parent's root: a store view's arms
            // see the same store.
            root: self.root.clone(),
            store_root: self.store_root.clone(),
            volume: self.volume.clone(),
            policy: self.policy.clone(),
            scope: Arc::clone(&self.scope),
            inner: Mutex::new(inner),
        })
    }

    /// The identity every operation through this capability is
    /// attributed to. Crate-internal: the Engine reads it through
    /// [`crate::detail::access_id`].
    pub(crate) const fn exec_id(&self) -> ExecId {
        self.id
    }

    /// The scope this identity belongs to. Crate-internal: backs
    /// [`crate::detail::scope_handle`].
    pub(crate) const fn scope(&self) -> &Arc<Scope> {
        &self.scope
    }

    /// Crate-internal: backs [`crate::detail::access_join`].
    pub(crate) fn join(&self, child: ExecId) {
        self.scope.join(self.id, child);
    }

    /// The claims-table half of one operation: refuses once the run that
    /// owned this access has ended, runs `claim` against this access's
    /// identity and scope, and re-spells a store view's error paths into
    /// the caller's logical form.
    pub(super) fn admit(
        &self,
        claim: impl FnOnce(&Claims, &Arc<Scope>, ExecId, &VfsPath) -> Result<(), VfsError>,
        path: &VfsPath,
    ) -> Result<(), VfsError> {
        self.scope
            .refuse_if_closed(path)
            .and_then(|()| claim(&self.volume.claims, &self.scope, self.id, path))
            .map_err(|err| self.relativize(err))
    }

    /// Re-spells an error's paths for the store view's caller; a plain
    /// access's errors pass through untouched.
    pub(super) fn relativize(&self, err: VfsError) -> VfsError {
        match &self.store_root {
            Some(root) => relativize_error(err, root.as_str()),
            None => err,
        }
    }

    /// Canonicalizes at receipt and consults the policy - in that order,
    /// so a denied operation never registers a claim and every claim key
    /// is the canonical path. A path without a leading `/` joins onto the
    /// access's root. The store view applies the store's strict
    /// logical-path rules before canonicalization, and re-spells its
    /// policy denials into the caller's logical form.
    pub(super) fn gate(&self, op: Op, path: &str) -> Result<VfsPath, VfsError> {
        // The strict rules run on every caller-supplied path, glob
        // patterns included: a leading `/` is the one pattern shape
        // that would canonicalize namespace-absolute and carry the
        // claim outside the store mount. A pattern's wildcard grammar
        // is validated separately, in `Access::glob`, before the gate.
        if self.store_root.is_some() {
            validate_store_path(path)?;
        }
        let path = canonicalize(&self.root, path)?;
        self.check_policy(op, &path)
            .map_err(|err| self.relativize(err))?;
        Ok(path)
    }

    /// Consults the handle's policy. v1 maps `Ask` to `PermissionDenied`:
    /// the approval dialog is a Host concern above this layer, and the
    /// reason string still names what was asked and which rule fired.
    fn check_policy(&self, op: Op, path: &VfsPath) -> Result<(), VfsError> {
        match self.policy.check(op, path) {
            Verdict::Allow => Ok(()),
            Verdict::Deny(reason) | Verdict::Ask(reason) => Err(VfsError::PermissionDenied {
                path: path.to_string(),
                reason,
            }),
        }
    }

    /// Fires the installed sink for one admitted path of `op` -
    /// fire-and-forget, after policy and claims pass, before the backend
    /// executes. A handle without a sink, or a capability vended to a
    /// mounted-handle forward (the outer handle already fired with the
    /// caller's origin), fires nothing.
    pub(super) fn fire(&self, op: Op, path: &VfsPath) {
        let (Some(sink), Some(origin)) = (&self.volume.sink, &self.origin) else {
            return;
        };
        sink(OpEvent { op, path, origin });
    }

    /// Reads the file and resolves one line range while its contents
    /// remain live.
    pub(super) fn with_line_range(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
        render: impl FnOnce(&[&str], usize) -> String,
    ) -> Result<String, VfsError> {
        // The store view validates the path before the bounds, in the
        // store contract's order.
        if self.store_root.is_some() {
            validate_store_path(path)?;
        }
        if start < 1 {
            return Err(VfsError::InvalidRange {
                path: path.to_owned(),
                reason: "start is below 1",
            });
        }
        let contents = self.read_string(path)?;
        let lines: Vec<&str> = contents.lines().collect();
        if start > lines.len() {
            return Ok(String::new());
        }
        let end = end.unwrap_or(lines.len()).min(lines.len());
        if end < start {
            return Err(VfsError::InvalidRange {
                path: path.to_owned(),
                reason: "end is before start",
            });
        }
        Ok(render(&lines[start - 1..end], start))
    }

    /// Poison-safe lock on the backend's access object, held per call.
    pub(super) fn inner(&self) -> MutexGuard<'_, Box<dyn VfsAccess>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Poison-safe lock on the backend.
    pub(super) fn backend(&self) -> MutexGuard<'_, Box<dyn Vfs>> {
        self.volume
            .backend
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
