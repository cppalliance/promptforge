//! Case-folded claim keys: on Windows and macOS the file system under a
//! rooted real mount treats paths that differ only in case as one file,
//! so the claims check keys each such path by its lowercase spelling
//! below the mount, and two spellings of one file conflict.

use std::any::Any;
use std::sync::Arc;

use super::VfsRef;
use crate::path::VfsPath;
use crate::real::RealBackend;
use crate::router::{mount_matches, rejoin};

/// Which mount prefixes of one handle fold case. The longest prefix
/// serving a path decides, as in the mount table, so every prefix is
/// kept, folding or not.
#[derive(Clone, Default)]
pub(crate) struct CaseFolds {
    /// Each mount prefix, canonical, and whether the paths it serves fold.
    mounts: Arc<[(String, bool)]>,
    /// Whether any prefix folds: when none does, keys are the paths.
    any: bool,
}

impl CaseFolds {
    /// The folds of `backend` mounted at `prefix`: a rooted real
    /// backend's own, a mounted handle's moved under `prefix`, and none
    /// for any other backend.
    pub(crate) fn of(prefix: &VfsPath, backend: &dyn Any) -> CaseFolds {
        let prefix = prefix.as_str();
        if let Some(real) = backend.downcast_ref::<RealBackend>() {
            return CaseFolds::from_mounts(vec![(prefix.to_owned(), real.folds_case())]);
        }
        if let Some(handle) = backend.downcast_ref::<VfsRef>() {
            let moved = handle
                .volume
                .folds
                .mounts
                .iter()
                .map(|(inner, folds)| (rejoin(prefix, inner), *folds))
                .collect();
            return CaseFolds::from_mounts(moved);
        }
        CaseFolds::from_mounts(vec![(prefix.to_owned(), false)])
    }

    /// These folds and `other`'s, as one mount table holding both. A
    /// prefix in both takes `other`'s.
    pub(crate) fn with(&self, other: &CaseFolds) -> CaseFolds {
        let mut mounts: Vec<(String, bool)> = self
            .mounts
            .iter()
            .filter(|(prefix, _)| other.mounts.iter().all(|(theirs, _)| theirs != prefix))
            .cloned()
            .collect();
        mounts.extend(other.mounts.iter().cloned());
        CaseFolds::from_mounts(mounts)
    }

    /// The claim key of `path`: the path itself, or, when the mount
    /// serving it folds case, the path with its part below the mount
    /// prefix lowercased. The prefix keeps its spelling, so a store
    /// view still re-spells the key relative to its root.
    pub(crate) fn key(&self, path: &VfsPath) -> VfsPath {
        if !self.any {
            return path.clone();
        }
        let text = path.as_str();
        let serving = self
            .mounts
            .iter()
            .filter(|(prefix, _)| mount_matches(prefix, text))
            .max_by_key(|(prefix, _)| prefix.len());
        let Some((prefix, true)) = serving else {
            return path.clone();
        };
        let (kept, below) = text.split_at(if prefix == "/" { 0 } else { prefix.len() });
        let folded = below.to_lowercase();
        if folded == below {
            return path.clone();
        }
        VfsPath::from_canonical(&format!("{kept}{folded}"))
    }

    fn from_mounts(mounts: Vec<(String, bool)>) -> CaseFolds {
        let any = mounts.iter().any(|(_, folds)| *folds);
        CaseFolds {
            mounts: mounts.into(),
            any,
        }
    }
}
