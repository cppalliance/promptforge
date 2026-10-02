use crate::memory::MemoryBackend;

use super::{VfsError, VfsRef, test_origin};
use crate::PathReason;

fn memory() -> VfsRef {
    VfsRef::new(MemoryBackend::new())
}

#[test]
fn a_relative_path_joins_onto_the_access_root() -> Result<(), VfsError> {
    let access = memory().acquire(test_origin())?;
    access.write("notes/a.txt", b"hi")?;
    assert_eq!(access.read("/notes/a.txt")?, b"hi");
    assert_eq!(access.read("notes/a.txt")?, b"hi");
    Ok(())
}

#[test]
fn dotdot_stops_at_the_access_root() -> Result<(), VfsError> {
    let access = memory().acquire(test_origin())?;
    access.write("/drafts/f.txt", b"x")?;
    assert_eq!(access.read("drafts/../drafts/f.txt")?, b"x");
    assert_eq!(
        access.read("../f.txt"),
        Err(VfsError::InvalidPath {
            path: "../f.txt".to_owned(),
            reason: PathReason::Traversal,
        })
    );
    Ok(())
}

#[test]
fn removing_a_missing_path_is_ok_false() -> Result<(), VfsError> {
    let access = memory().acquire(test_origin())?;
    assert!(!access.remove("/gone.txt", false)?);
    access.write("/f.txt", b"x")?;
    assert!(access.remove("/f.txt", false)?);
    assert!(!access.remove("/f.txt", false)?);
    Ok(())
}

#[test]
fn glob_returns_files_and_a_trailing_slash_selects_directories() -> Result<(), VfsError> {
    let access = memory().acquire(test_origin())?;
    access.write("/d/a.txt", b"")?;
    access.write("/d/sub/c.txt", b"")?;
    assert_eq!(access.glob("/d/*")?, vec!["/d/a.txt".to_owned()]);
    assert_eq!(access.glob("/d/*/")?, vec!["/d/sub".to_owned()]);
    Ok(())
}

#[test]
fn glob_refuses_a_backslash_in_the_raw_pattern() -> Result<(), VfsError> {
    let access = memory().acquire(test_origin())?;
    access.write("/a.txt", b"")?;
    // A backslash is refused as written, never turned into a
    // separator by canonicalization.
    assert_eq!(
        access.glob("/a\\b"),
        Err(VfsError::InvalidPath {
            path: "/a\\b".to_owned(),
            reason: PathReason::Backslash,
        })
    );
    assert_eq!(
        access.glob("/a/***/b"),
        Err(VfsError::InvalidPath {
            path: "/a/***/b".to_owned(),
            reason: PathReason::Wildcard,
        })
    );
    Ok(())
}

#[test]
fn glob_names_the_rule_for_an_empty_control_or_over_long_pattern() -> Result<(), VfsError> {
    let access = memory().acquire(test_origin())?;
    assert_eq!(
        access.glob(""),
        Err(VfsError::InvalidPath {
            path: String::new(),
            reason: PathReason::Empty,
        })
    );
    assert_eq!(
        access.glob("/a\u{0}b"),
        Err(VfsError::InvalidPath {
            path: "/a\u{0}b".to_owned(),
            reason: PathReason::Control,
        })
    );
    let over_long = format!("/{}", "a".repeat(1024));
    assert_eq!(
        access.glob(&over_long),
        Err(VfsError::InvalidPath {
            path: over_long,
            reason: PathReason::TooLong,
        })
    );
    Ok(())
}

#[test]
fn a_relative_pattern_yields_relative_results() -> Result<(), VfsError> {
    let access = memory().acquire(test_origin())?;
    access.write("/d/a.txt", b"")?;
    access.write("/d/b.md", b"")?;
    access.write("/d/sub/c.txt", b"")?;
    assert_eq!(access.glob("d/*.txt")?, vec!["d/a.txt".to_owned()]);
    assert_eq!(access.glob("d/*/")?, vec!["d/sub".to_owned()]);
    Ok(())
}
