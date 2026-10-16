//! The store view: the store's strict logical-path rules, the
//! re-spelling of error paths into the caller's logical form, and the
//! access rooted at the declared store root.

use std::sync::{Arc, Mutex};

use super::forward::{StoreMount, StoreScoped};
use super::{Access, Volume};
use crate::error::{PathReason, VfsError};
use crate::router::{Mounts, Router};
use crate::traits::{AcquireContext, Vfs};

/// Re-spells a namespace-absolute result relative to the access's root,
/// or `None` when the result does not sit under it (never, for results
/// of a pattern that joined onto the root).
pub(super) fn strip_root(root: &str, matched: &str) -> Option<String> {
    if root == "/" {
        return matched.strip_prefix('/').map(str::to_owned);
    }
    let prefix = format!("{root}/");
    matched.strip_prefix(&prefix).map(str::to_owned)
}

/// The largest logical store path, in bytes, accepted by the store
/// view, so a store path stays a bounded denial-of-service lever.
const MAX_STORE_PATH_BYTES: usize = 1024;

/// The store's strict logical-path rules, applied by the store view
/// before a path reaches canonicalization, in the store contract's
/// check order: the first rule broken is the one reported, with the
/// path exactly as supplied.
pub(super) fn validate_store_path(raw: &str) -> Result<(), VfsError> {
    let reject = |reason| {
        Err(VfsError::InvalidPath {
            path: raw.to_owned(),
            reason,
        })
    };
    if raw.is_empty() {
        return reject(PathReason::Empty);
    }
    if raw.len() > MAX_STORE_PATH_BYTES {
        return reject(PathReason::TooLong);
    }
    if raw.starts_with('/') {
        return reject(PathReason::Absolute);
    }
    if raw.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return reject(PathReason::Control);
    }
    // A backslash is a separator on some backends and a literal on
    // others; refuse it so a canonical `/`-separated path cannot be
    // reinterpreted.
    if raw.contains('\\') {
        return reject(PathReason::Backslash);
    }
    // A colon names a drive or an alternate data stream on Windows, so
    // the stored file would not be the one named.
    if raw.contains(':') {
        return reject(PathReason::ReservedName);
    }
    for segment in raw.split('/') {
        if segment.is_empty() {
            return reject(PathReason::EmptySegment);
        }
        if segment == "." || segment == ".." {
            return reject(PathReason::Traversal);
        }
        // Trailing `.`/space are stripped by some backends, so the
        // stored name would not round-trip.
        if segment.ends_with('.') || segment.ends_with(' ') {
            return reject(PathReason::UnsafeSuffix);
        }
        if is_reserved_device_name(segment) {
            return reject(PathReason::ReservedName);
        }
    }
    Ok(())
}

/// Whether `segment` is a platform-reserved device name. Windows
/// treats names like `CON`, `NUL`, `COM1`, and `LPT1` as devices even
/// with an extension (`con.txt`), so the base name before the first
/// `.` is checked case-insensitively.
fn is_reserved_device_name(segment: &str) -> bool {
    let base = segment.split('.').next().unwrap_or(segment);
    let upper = base.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || is_numbered_device(&upper, "COM")
        || is_numbered_device(&upper, "LPT")
}

/// Whether `name` is `<prefix>N` for one device digit: `0` through `9`,
/// or the superscript `¹`, `²`, or `³`, which Windows also reserves.
fn is_numbered_device(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|rest| {
        let mut digits = rest.chars();
        matches!(
            (digits.next(), digits.next()),
            (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
        )
    })
}

/// Re-spells an error's canonical paths for a store view's caller: a
/// path under the store root becomes the logical form the caller
/// supplied. A logical path is never absolute, so one already in that
/// form passes unchanged.
pub(super) fn relativize_error(err: VfsError, root: &str) -> VfsError {
    respell_error(err, |path| {
        strip_root(root, path).unwrap_or_else(|| path.trim_start_matches('/').to_owned())
    })
}

/// Re-spells a store mount's mount-relative error paths into the
/// logical form: only the leading `/` goes.
pub(super) fn trim_mount_error(err: VfsError) -> VfsError {
    respell_error(err, |path| path.trim_start_matches('/').to_owned())
}

/// Re-spells every path in `err` through `logical`; prose fields that
/// embed the path (a conflict's diagnosis, a denial's reason) follow.
fn respell_error(err: VfsError, logical: impl Fn(&str) -> String) -> VfsError {
    match err {
        VfsError::NotFound { path } => VfsError::NotFound {
            path: logical(&path),
        },
        VfsError::AlreadyExists { path } => VfsError::AlreadyExists {
            path: logical(&path),
        },
        VfsError::NotADirectory { path } => VfsError::NotADirectory {
            path: logical(&path),
        },
        VfsError::IsADirectory { path } => VfsError::IsADirectory {
            path: logical(&path),
        },
        VfsError::DirectoryNotEmpty { path } => VfsError::DirectoryNotEmpty {
            path: logical(&path),
        },
        VfsError::NotUtf8 { path, valid_up_to } => VfsError::NotUtf8 {
            path: logical(&path),
            valid_up_to,
        },
        VfsError::InvalidPath { path, reason } => VfsError::InvalidPath {
            path: logical(&path),
            reason,
        },
        VfsError::InvalidRange { path, reason } => VfsError::InvalidRange {
            path: logical(&path),
            reason,
        },
        VfsError::Anchor {
            path,
            anchor,
            count,
        } => VfsError::Anchor {
            path: logical(&path),
            anchor,
            count,
        },
        VfsError::PermissionDenied { path, reason } => VfsError::PermissionDenied {
            reason: reason.replace(&path, &logical(&path)),
            path: logical(&path),
        },
        VfsError::ApprovalRequired { path, reason } => VfsError::ApprovalRequired {
            reason: reason.replace(&path, &logical(&path)),
            path: logical(&path),
        },
        VfsError::ScopeEnded { path } => VfsError::ScopeEnded {
            path: logical(&path),
        },
        VfsError::ReadOnly { path } => VfsError::ReadOnly {
            path: logical(&path),
        },
        VfsError::OutsideRoot { path, detail } => VfsError::OutsideRoot {
            detail: detail.replace(&path, &logical(&path)),
            path: logical(&path),
        },
        VfsError::RootProtected { path } => VfsError::RootProtected {
            path: logical(&path),
        },
        VfsError::NoStore => VfsError::NoStore,
        VfsError::CrossMount { path, detail } => VfsError::CrossMount {
            detail: detail.replace(&path, &logical(&path)),
            path: logical(&path),
        },
        VfsError::Io {
            path,
            kind,
            raw_os_error,
            message,
        } => VfsError::Io {
            path: logical(&path),
            kind,
            raw_os_error,
            message,
        },
        VfsError::Unsupported { path, detail } => VfsError::Unsupported {
            detail: detail.replace(&path, &logical(&path)),
            path: logical(&path),
        },
        VfsError::Conflict { path, detail } => VfsError::Conflict {
            detail: detail.replace(&path, &logical(&path)),
            path: logical(&path),
        },
        VfsError::Backend { message } => VfsError::Backend { message },
    }
}

impl Access {
    /// The store view: an access rooted at the declared store root
    /// whose operations reach the store's own mount alone, sharing
    /// this access's identity, scope, claims table, policy, and op
    /// sink. The view applies the store's strict logical-path rules
    /// and reports error paths in the caller's logical form.
    /// Crate-internal: backs [`crate::detail::store_view`].
    ///
    /// # Errors
    /// Returns [`VfsError::NoStore`] when the handle declares no store,
    /// or [`VfsError::ScopeEnded`], before any backend call, when the run
    /// or the tool call that owned this access has ended.
    pub(crate) fn store_view(&self) -> Result<Access, VfsError> {
        self.scope
            .refuse_if_closed(&self.root)
            .map_err(|err| self.relativize(err))?;
        let Some(store) = &self.volume.store else {
            return Err(VfsError::NoStore);
        };
        // One mount - the store's own - behind a fresh router:
        // operations are confined to the store mount, so a store at
        // `/` never reaches a real directory mounted beneath it.
        let mut mounts = Mounts::new();
        mounts.insert(
            store.root.to_buf(),
            Arc::new(Mutex::new(Box::new(StoreMount(Arc::clone(&store.mount))))),
        );
        let mut router = Router::new(mounts);
        // The view holds one more reference to the identity, like a
        // mount forward: the identity - and its scope - ends with its
        // last access, the view included.
        self.scope
            .attach(self.id, &self.root)
            .map_err(|err| self.relativize(err))?;
        let inner = match router.acquire(&AcquireContext::new(self.id, Arc::clone(&self.scope))) {
            Ok(inner) => inner,
            Err(err) => {
                self.scope.release(self.id);
                return Err(err);
            }
        };
        Ok(Access {
            id: self.id,
            // The view keeps the chain's origin: store operations fire
            // the op sink under the chain's label.
            origin: self.origin.clone(),
            root: store.root.clone(),
            store_root: Some(store.root.clone()),
            volume: Arc::new(Volume {
                backend: Arc::new(Mutex::new(Box::new(router))),
                claims: Arc::clone(&self.volume.claims),
                sink: self.volume.sink.clone(),
                store: None,
                folds: store.folds.clone(),
            }),
            policy: Arc::clone(&self.policy),
            scope: Arc::clone(&self.scope),
            inner: Mutex::new(Box::new(StoreScoped {
                root: store.root.as_str().to_owned(),
                inner,
            })),
            released: false,
        })
    }

    /// Stats this access's root straight through its backend session,
    /// with no policy check, claim, or event, so a lazily acquiring
    /// router opens the serving mount's session. A root the backend
    /// reports missing passes. Crate-internal: backs
    /// [`crate::detail::probe_store`].
    pub(crate) fn probe_root(&self) -> Result<(), VfsError> {
        match self.inner().stat(&self.root) {
            Ok(_) | Err(VfsError::NotFound { .. }) => Ok(()),
            Err(err) => Err(err),
        }
    }
}
