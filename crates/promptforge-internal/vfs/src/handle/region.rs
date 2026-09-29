//! Region geometry: the parent, the created ancestors, the subtree
//! cover, and the pattern overlap that the claims checks compare
//! regions by.

use crate::glob::{compile_glob, matches_tokens, validate_glob_grammar};
use crate::path::{VfsPath, canonicalize_absolute};

/// The parent directory of `path`, or `None` at the namespace root.
pub(super) fn parent_of(path: &VfsPath) -> Option<VfsPath> {
    let (parent, _) = path.as_str().rsplit_once('/')?;
    if parent.is_empty() {
        None
    } else {
        Some(
            canonicalize_absolute(parent)
                .unwrap_or_else(|err| panic!("a canonical path's parent canonicalizes: {err}")),
        )
    }
}

/// The ancestors a write to `path` may create, nearest first, excluding
/// the namespace root: creating the file may create its parent
/// directories.
pub(super) fn may_create(path: &VfsPath) -> Vec<VfsPath> {
    let mut ancestors = Vec::new();
    let mut rest = path.as_str();
    while let Some((parent, _)) = rest.rsplit_once('/') {
        if parent.is_empty() {
            break;
        }
        rest = parent;
        ancestors.push(
            canonicalize_absolute(parent)
                .unwrap_or_else(|err| panic!("a canonical path's parent canonicalizes: {err}")),
        );
    }
    ancestors
}

/// Whether `subtree` covers `path`: the subtree is the path itself or
/// everything under it.
pub(super) fn subtree_covers(subtree: &VfsPath, path: &VfsPath) -> bool {
    path.as_str() == subtree.as_str()
        || path
            .as_str()
            .strip_prefix(subtree.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The literal base of a canonical glob `pattern`: the text before the
/// first wildcard, with any trailing slash dropped. The base and its own
/// ancestors are the directories a matching write may create.
pub(super) fn pattern_base(pattern: &VfsPath) -> Vec<VfsPath> {
    let Some((prefix, _)) = pattern.as_str().split_once('*') else {
        // No wildcard: the pattern names one path, and a write creating
        // it claims that path's ancestors.
        return may_create(pattern);
    };
    let base = prefix.trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }
    let base = canonicalize_absolute(base)
        .unwrap_or_else(|err| panic!("a canonical pattern's literal prefix canonicalizes: {err}"));
    let mut bases = vec![base.clone()];
    let mut rest = base;
    while let Some((parent, _)) = rest.as_str().rsplit_once('/') {
        if parent.is_empty() {
            break;
        }
        rest = canonicalize_absolute(parent).unwrap_or_else(|err| {
            panic!("a canonical pattern's literal prefix canonicalizes: {err}")
        });
        bases.push(rest.clone());
    }
    bases
}

/// Whether `pattern` matches `path`, as the glob grammar reads it. A
/// pattern whose grammar did not pass validation falls back to its
/// literal prefix, which is conservative.
pub(super) fn pattern_matches_path(pattern: &VfsPath, path: &VfsPath) -> bool {
    if validate_glob_grammar(pattern.as_str()).is_err() {
        return pattern_base(pattern)
            .first()
            .is_some_and(|base| path.as_str().starts_with(base.as_str()));
    }
    let tokens = compile_glob(pattern.as_str().as_bytes());
    matches_tokens(&tokens, path.as_str().as_bytes())
}

/// Whether `pattern` overlaps `subtree`, conservatively by literal
/// prefix: one's literal prefix is a prefix of the other's.
pub(super) fn pattern_overlaps_subtree(pattern: &VfsPath, subtree: &VfsPath) -> bool {
    let Some((prefix, _)) = pattern.as_str().split_once('*') else {
        return pattern_matches_path(pattern, subtree) || subtree_covers(subtree, pattern);
    };
    let prefix = prefix.trim_end_matches('/');
    let subtree = subtree.as_str();
    prefix == subtree
        || prefix
            .strip_prefix(subtree)
            .is_some_and(|rest| rest.starts_with('/'))
        || subtree
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}
