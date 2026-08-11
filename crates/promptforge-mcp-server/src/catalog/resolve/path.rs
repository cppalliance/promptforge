//! Root confinement for catalog resolution.
//!
//! Every file the catalog serves is named relative to the prompts directory,
//! so a pattern or a `[prompts.NAME].file` that reaches outside it is a
//! configuration mistake, not a prompt. Two checks enforce that:
//!
//! - the shape check [`crate::relpath::reject_traversal`] refuses a pattern or
//!   path whose components could climb out of the root; it runs at the
//!   configuration boundary, so a value that reaches resolution is already
//!   known to be relative.
//! - [`confined`] refuses a resolved file whose canonical location is not a
//!   descendant of the canonical root, which is what catches a symlink or
//!   reparse point placed under the root that points outside it - an escape no
//!   shape check can see.

use std::path::Path;

/// Whether `file` resolves to a location inside the prompts directory.
///
/// Both the root and the file are canonicalized, so a symlink under the root
/// that points outside it resolves to its target and is refused. A file that
/// cannot be canonicalized (it was removed mid-pass, or the root does not
/// exist) is treated as not confined rather than admitted on faith.
pub(super) fn confined(root: &Path, file: &Path) -> bool {
    match (root.canonicalize(), file.canonicalize()) {
        (Ok(root), Ok(real)) => real.starts_with(&root),
        _ => false,
    }
}

/// Whether `a` and `b` name the same file by canonical identity rather than
/// lexical spelling.
///
/// This is what lets a `[prompts.NAME]` block spelled `./top.md` recognize the
/// globbed `top.md` as the file it overrides, and a block naming a symlink
/// recognize the target a glob already matched, so the block replaces that
/// entry rather than adding a second that then collides on name. A path that
/// cannot be canonicalized falls back to lexical equality, which errs toward
/// treating the paths as distinct rather than overwriting on an unproven match.
pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::confined;

    #[test]
    fn a_file_under_the_root_is_confined_and_one_outside_is_not() {
        let root = tempfile::tempdir().expect("temporary root");
        let outside = tempfile::tempdir().expect("temporary outside directory");
        let inside_file = root.path().join("prompt.md");
        std::fs::write(&inside_file, "x").expect("write inside file");
        let outside_file = outside.path().join("prompt.md");
        std::fs::write(&outside_file, "x").expect("write outside file");

        assert!(confined(root.path(), &inside_file));
        assert!(!confined(root.path(), &outside_file));
    }
}
