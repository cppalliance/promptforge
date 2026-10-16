use super::{RealBackend, TempRoot, VfsError};
use crate::{MemoryBackend, Origin, PathReason, VfsRef};

fn rooted(temp: &TempRoot) -> Result<VfsRef, VfsError> {
    Ok(VfsRef::new(RealBackend::rooted(temp.path())?))
}

#[test]
fn a_relative_path_joins_onto_the_access_root() -> Result<(), VfsError> {
    let temp = TempRoot::new("a_relative_path_joins_onto_the_access_root");
    let access = rooted(&temp)?.acquire(Origin::new("real semantics test"))?;
    access.write("a/b.txt", b"hi")?;
    assert_eq!(access.read("a/b.txt")?, b"hi");
    assert_eq!(access.read("/a/b.txt")?, b"hi");
    Ok(())
}

#[test]
fn dotdot_resolves_inside_the_access_root_and_is_refused_above_it() -> Result<(), VfsError> {
    let temp = TempRoot::new("dotdot_resolves_inside_the_access_root_and_is_refused_above_it");
    let access = rooted(&temp)?.acquire(Origin::new("real semantics test"))?;
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
    let temp = TempRoot::new("removing_a_missing_path_is_ok_false");
    let access = rooted(&temp)?.acquire(Origin::new("real semantics test"))?;
    assert!(!access.remove("missing.txt", false)?);
    access.write("f.txt", b"x")?;
    assert!(access.remove("f.txt", false)?);
    assert!(!access.remove("f.txt", false)?);
    Ok(())
}

#[test]
fn glob_returns_files_and_a_trailing_slash_selects_directories() -> Result<(), VfsError> {
    let temp = TempRoot::new("glob_returns_files_and_a_trailing_slash_selects_directories");
    let access = rooted(&temp)?.acquire(Origin::new("real semantics test"))?;
    access.write("/d/a.txt", b"")?;
    access.write("/d/sub/c.txt", b"")?;
    assert_eq!(access.glob("/d/*")?, vec!["/d/a.txt".to_owned()]);
    assert_eq!(access.glob("/d/*/")?, vec!["/d/sub".to_owned()]);
    Ok(())
}

#[test]
fn a_relative_pattern_yields_relative_results() -> Result<(), VfsError> {
    let temp = TempRoot::new("a_relative_pattern_yields_relative_results");
    let access = rooted(&temp)?.acquire(Origin::new("real semantics test"))?;
    access.write("/d/a.txt", b"")?;
    assert_eq!(access.glob("d/*.txt")?, vec!["d/a.txt".to_owned()]);
    Ok(())
}

#[test]
fn glob_refuses_a_backslash_in_the_raw_pattern() -> Result<(), VfsError> {
    let temp = TempRoot::new("glob_refuses_a_backslash_in_the_raw_pattern");
    let access = rooted(&temp)?.acquire(Origin::new("real semantics test"))?;
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
    let temp = TempRoot::new("glob_refuses_control_characters_and_bad_grammar_with_their_reasons");
    let access = rooted(&temp)?.acquire(Origin::new("real semantics test"))?;
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

#[test]
fn two_spellings_of_one_file_conflict_where_the_file_system_folds_case() -> Result<(), VfsError> {
    let temp = TempRoot::new("two_spellings_of_one_file_conflict_where_the_file_system_folds_case");
    let store =
        TempRoot::new("two_spellings_of_one_file_conflict_where_the_file_system_folds_case");
    let vfs = VfsRef::builder()
        .mount("/", MemoryBackend::new())
        .mount("/Real", RealBackend::rooted(temp.path())?)
        .store("/Store", RealBackend::rooted(store.path())?)
        .build();
    let first = vfs.acquire(Origin::new("first spelling"))?;
    let second = vfs.acquire(Origin::new("second spelling"))?;
    first.write("/Real/Notes.md", b"1")?;
    let mount = second.write("/Real/NOTES.md", b"2");
    let first_view = vfs.acquire_store(Origin::new("first view"))?;
    let second_view = vfs.acquire_store(Origin::new("second view"))?;
    first_view.write("Draft.md", b"1")?;
    let view = second_view.write("draft.md", b"2");
    // The memory mount keeps case, so its two spellings are two files.
    first.write("/Mem.md", b"1")?;
    second.write("/mem.md", b"2")?;
    if cfg!(any(windows, target_os = "macos")) {
        match mount {
            Err(VfsError::Conflict { path, .. }) => {
                assert_eq!(path, "/Real/notes.md", "the key folds below the mount");
            }
            other => panic!("expected the folded conflict, got {other:?}"),
        }
        match view {
            Err(VfsError::Conflict { path, .. }) => assert_eq!(path, "draft.md"),
            other => panic!("expected the folded conflict, got {other:?}"),
        }
    } else {
        mount?;
        view?;
    }
    Ok(())
}
