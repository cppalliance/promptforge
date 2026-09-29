use super::{HostBackend, TempDir, VfsError};
use crate::{Origin, PathReason, VfsRef};

fn rooted(temp: &TempDir) -> Result<VfsRef, VfsError> {
    Ok(VfsRef::new(HostBackend::rooted(temp.path())?))
}

#[test]
fn a_relative_path_joins_onto_the_access_root() -> Result<(), VfsError> {
    let temp = TempDir::new()?;
    let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
    access.write("a/b.txt", b"hi")?;
    assert_eq!(access.read("a/b.txt")?, b"hi");
    assert_eq!(access.read("/a/b.txt")?, b"hi");
    Ok(())
}

#[test]
fn dotdot_resolves_inside_the_access_root_and_is_refused_above_it() -> Result<(), VfsError> {
    let temp = TempDir::new()?;
    let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
    access.write("/drafts/f.txt", b"x")?;
    assert_eq!(access.read("drafts/../drafts/f.txt")?, b"x");
    assert!(matches!(
        access.read("../f.txt"),
        Err(VfsError::InvalidPath { .. })
    ));
    Ok(())
}

#[test]
fn removing_a_missing_path_is_ok_false() -> Result<(), VfsError> {
    let temp = TempDir::new()?;
    let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
    assert!(!access.remove("missing.txt", false)?);
    access.write("f.txt", b"x")?;
    assert!(access.remove("f.txt", false)?);
    assert!(!access.remove("f.txt", false)?);
    Ok(())
}

#[test]
fn glob_returns_files_and_a_trailing_slash_selects_directories() -> Result<(), VfsError> {
    let temp = TempDir::new()?;
    let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
    access.write("/d/a.txt", b"")?;
    access.write("/d/sub/c.txt", b"")?;
    assert_eq!(access.glob("/d/*")?, vec!["/d/a.txt".to_owned()]);
    assert_eq!(access.glob("/d/*/")?, vec!["/d/sub".to_owned()]);
    Ok(())
}

#[test]
fn a_relative_pattern_yields_relative_results() -> Result<(), VfsError> {
    let temp = TempDir::new()?;
    let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
    access.write("/d/a.txt", b"")?;
    assert_eq!(access.glob("d/*.txt")?, vec!["d/a.txt".to_owned()]);
    Ok(())
}

#[test]
fn glob_refuses_a_backslash_in_the_raw_pattern() -> Result<(), VfsError> {
    let temp = TempDir::new()?;
    let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
    assert_eq!(
        access.glob("/a\\b"),
        Err(VfsError::InvalidPath {
            path: "/a\\b".to_owned(),
            reason: PathReason::Backslash,
        })
    );
    Ok(())
}

#[test]
fn glob_refuses_control_characters_and_bad_grammar_with_their_reasons() -> Result<(), VfsError> {
    let temp = TempDir::new()?;
    let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
    assert_eq!(
        access.glob("/a\u{0}b"),
        Err(VfsError::InvalidPath {
            path: "/a\u{0}b".to_owned(),
            reason: PathReason::Control,
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
