//! The shape check every path and pattern the catalog serves must pass:
//! relative to the prompts directory, and free of any component that could
//! climb out of it.
//!
//! Both the configuration boundary (where a `[prompts.NAME].file` or a
//! `[catalog]` pattern is turned into a validated newtype) and the resolution
//! pass (where the canonical [`confined`](crate::catalog) check catches a
//! symlink that only the filesystem can reveal) rest on this one function, so
//! the escape rule is stated once and cannot drift between the two.

use std::path::{Component, Path};

/// Refuses a pattern or path that could reach outside the prompts directory by
/// its shape alone, before it is joined to the root.
///
/// An absolute path ignores the root entirely, a `..` component climbs above
/// it, and a Windows drive or verbatim `\\?\` prefix is a disguised absolute
/// path.
///
/// # Errors
/// Returns a human-readable detail when `candidate` is absolute, carries a
/// drive or verbatim prefix, or contains a `..` component.
pub(crate) fn reject_traversal(candidate: &Path) -> Result<(), String> {
    for component in candidate.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(format!(
                    "{} must be relative to the prompts directory",
                    candidate.display()
                ));
            }
            Component::ParentDir => {
                return Err(format!(
                    "{} must not climb above the prompts directory with `..`",
                    candidate.display()
                ));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::reject_traversal;

    #[test]
    fn an_absolute_pattern_is_refused() {
        let absolute = if cfg!(windows) {
            PathBuf::from(r"C:\etc\secrets\*.md")
        } else {
            PathBuf::from("/etc/secrets/*.md")
        };
        let err = reject_traversal(&absolute).expect_err("an absolute pattern escapes the root");
        assert!(err.contains("relative"), "{err}");
    }

    #[test]
    fn a_parent_dir_component_is_refused() {
        let err = reject_traversal(Path::new("../outside/*.md"))
            .expect_err("a `..` component climbs above the root");
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn a_nested_parent_dir_component_is_refused() {
        let err = reject_traversal(Path::new("governance/../../escape.md"))
            .expect_err("a `..` anywhere climbs above the root");
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn a_plain_relative_pattern_is_allowed() {
        reject_traversal(Path::new("governance/**/*.md")).expect("a relative pattern is fine");
        reject_traversal(Path::new("./local.md")).expect("a leading ./ is fine");
    }
}
