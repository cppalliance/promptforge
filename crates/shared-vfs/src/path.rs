//! Canonical, shared virtual paths.
//!
//! Paths are canonicalized at the moment the API receives them, so claim
//! lookups compare one canonical form and aliases cannot slip past the
//! claims tables. The only way to form a [`VfsPath`] is through
//! `canonicalize`, which is crate-private: canonicalization at receipt is
//! enforced by visibility, not convention.

use std::fmt;
use std::sync::Arc;

use crate::error::VfsError;

/// Canonical virtual path. Produced by `canonicalize` at the moment the
/// API receives a path. The string is `Arc`-shared per value lineage:
/// clones share one allocation, and the string frees when its last
/// owner drops. There is no global table and no lock.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VfsPath {
    text: Arc<str>,
}

impl VfsPath {
    /// Returns the canonical string for this path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Returns an owned copy of this path.
    #[must_use]
    pub fn to_buf(&self) -> VfsPathBuf {
        VfsPathBuf(self.as_str().into())
    }
}

impl fmt::Debug for VfsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VfsPath({:?})", self.as_str())
    }
}

impl fmt::Display for VfsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Owned canonical virtual path, for places that outlive an interned
/// reference or arrive owned (grep roots, symlink targets). Ordered for
/// the mount table's `BTreeMap`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VfsPathBuf(String);

impl VfsPathBuf {
    /// Returns the canonical string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<VfsPath> for VfsPathBuf {
    fn from(path: VfsPath) -> Self {
        path.to_buf()
    }
}

impl fmt::Display for VfsPathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Canonicalizes a virtual path at API receipt.
///
/// The internal namespace is POSIX-shaped: rooted, forward slashes, strict.
/// The lexical rules are: backslashes from Windows hosts count as
/// separators; duplicate separators collapse; `.` segments vanish; `..`
/// pops exactly one segment and popping past the root is rejected; a
/// trailing slash is dropped; the root canonicalizes to itself. Case is
/// preserved and significant (POSIX semantics): paths differing only in
/// case are distinct. Relative and empty paths are rejected.
pub(crate) fn canonicalize(path: &str) -> Result<VfsPath, VfsError> {
    if path.is_empty() {
        return Err(VfsError::InvalidPath("empty path".into()));
    }
    let normalized = path.replace('\\', "/");
    if !normalized.starts_with('/') {
        return Err(VfsError::InvalidPath(format!(
            "relative path is not in the virtual namespace: {path:?}"
        )));
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(VfsError::InvalidPath(format!(
                        "path escapes the namespace root: {path:?}"
                    )));
                }
            }
            _ => segments.push(segment),
        }
    }
    let canonical = if segments.is_empty() {
        "/".to_owned()
    } else {
        let mut s = String::with_capacity(normalized.len() + 1);
        for segment in &segments {
            s.push('/');
            s.push_str(segment);
        }
        s
    };
    Ok(VfsPath {
        text: canonical.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{VfsPath, canonicalize};
    use crate::VfsError;

    fn canonical(path: &str) -> Result<String, VfsError> {
        Ok(canonicalize(path)?.as_str().to_owned())
    }

    #[test]
    fn the_namespace_root_canonicalizes_to_itself() -> Result<(), VfsError> {
        assert_eq!(canonical("/")?, "/");
        Ok(())
    }

    #[test]
    fn duplicate_separators_collapse_to_one() -> Result<(), VfsError> {
        assert_eq!(canonical("/a//b///c")?, "/a/b/c");
        Ok(())
    }

    #[test]
    fn dot_segments_are_removed() -> Result<(), VfsError> {
        assert_eq!(canonical("/a/./b/./c")?, "/a/b/c");
        Ok(())
    }

    #[test]
    fn dotdot_pops_exactly_one_segment() -> Result<(), VfsError> {
        assert_eq!(canonical("/a/b/../c")?, "/a/c");
        assert_eq!(canonical("/a/..")?, "/");
        Ok(())
    }

    #[test]
    fn a_trailing_slash_is_dropped() -> Result<(), VfsError> {
        assert_eq!(canonical("/a/b/")?, "/a/b");
        Ok(())
    }

    #[test]
    fn backslashes_from_windows_hosts_are_separators() -> Result<(), VfsError> {
        assert_eq!(canonical("/a\\b/c")?, "/a/b/c");
        Ok(())
    }

    #[test]
    fn traversal_past_the_root_is_rejected() {
        assert!(canonicalize("/..").is_err());
        assert!(canonicalize("/a/../../b").is_err());
    }

    #[test]
    fn relative_and_empty_paths_are_rejected() {
        assert!(canonicalize("").is_err());
        assert!(canonicalize("a/b").is_err());
        assert!(canonicalize("./a").is_err());
    }

    #[test]
    fn case_is_preserved_and_significant() -> Result<(), VfsError> {
        assert_eq!(canonical("/ReadMe.md")?, "/ReadMe.md");
        let upper: VfsPath = canonicalize("/ReadMe.md")?;
        let lower: VfsPath = canonicalize("/readme.md")?;
        assert_ne!(upper, lower);
        Ok(())
    }

    #[test]
    fn identical_paths_canonicalize_to_equal_values() -> Result<(), VfsError> {
        let first = canonicalize("/a/b")?;
        let second = canonicalize("/a/./b/")?;
        assert_eq!(first, second);
        assert_eq!(first.as_str(), second.as_str());
        Ok(())
    }

    #[test]
    fn clones_share_one_allocation() -> Result<(), VfsError> {
        let path = canonicalize("/a/b")?;
        let clone = path.clone();
        assert_eq!(Arc::strong_count(&path.text), 2);
        drop(clone);
        assert_eq!(Arc::strong_count(&path.text), 1);
        Ok(())
    }

    #[test]
    fn canonicalizing_distinct_paths_in_a_loop_does_not_retain_their_strings()
    -> Result<(), VfsError> {
        // Regression: the interner leaked every distinct string for the
        // process's life, so a loop like this grew the heap
        // monotonically. Each path's string must free with its last
        // owner - here, at the end of its own iteration.
        let mut dangling = Vec::new();
        for index in 0..1000 {
            let path = canonicalize(&format!("/loop/{index}"))?;
            dangling.push(Arc::downgrade(&path.text));
        }
        assert!(
            dangling.iter().all(|weak| weak.upgrade().is_none()),
            "a dropped path's string must free with its last owner"
        );
        Ok(())
    }
}
