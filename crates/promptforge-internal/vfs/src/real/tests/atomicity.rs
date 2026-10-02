//! Tests that a failed write, copy, or rename leaves the tree
//! unchanged and no temp file behind.

use super::*;
use crate::error::PathReason;

/// Asserts no failure-atomic temp file survived under `dir`.
fn assert_no_temp_files_left(dir: &Path) -> Result<(), VfsError> {
    for entry in fs::read_dir(dir).map_err(|err| map_io("listing the temp dir", &err))? {
        let entry = entry.map_err(|err| map_io("listing the temp dir", &err))?;
        assert!(
            !entry.file_name().to_string_lossy().contains(".vfs-tmp-"),
            "a temp file survived: {}",
            entry.path().display()
        );
    }
    Ok(())
}

#[test]
fn a_failed_write_leaves_the_destination_unchanged_and_no_temp_file_behind() -> Result<(), VfsError>
{
    let temp = TempDir::new()?;
    let mut access = rooted_access(temp.path())?;
    // A write over an existing directory fails before the temp
    // file is created; the directory survives.
    access.mkdir(&path("/dir")?, false)?;
    assert!(matches!(
        access.write(&path("/dir")?, b"x"),
        Err(VfsError::IsADirectory { .. })
    ));
    assert!(temp.path().join("dir").is_dir());
    // A write through a file ancestor fails; the file is unchanged.
    access.write(&path("/f.txt")?, b"original")?;
    assert!(access.write(&path("/f.txt/g.txt")?, b"x").is_err());
    assert_eq!(access.read(&path("/f.txt")?)?, b"original");
    assert_no_temp_files_left(temp.path())?;
    Ok(())
}

#[test]
fn a_failed_copy_leaves_source_and_destination_unchanged() -> Result<(), VfsError> {
    let temp = TempDir::new()?;
    let mut access = rooted_access(temp.path())?;
    access.write(&path("/dst.txt")?, b"old")?;
    assert!(matches!(
        access.copy(&path("/missing.txt")?, &path("/dst.txt")?),
        Err(VfsError::NotFound { .. })
    ));
    assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
    assert_no_temp_files_left(temp.path())?;
    Ok(())
}

#[test]
fn a_failed_rename_leaves_source_and_destination_unchanged() -> Result<(), VfsError> {
    let temp = TempDir::new()?;
    let mut access = rooted_access(temp.path())?;
    access.write(&path("/dst.txt")?, b"old")?;
    assert!(matches!(
        access.rename(&path("/missing.txt")?, &path("/dst.txt")?),
        Err(VfsError::NotFound { .. })
    ));
    assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
    // Renaming a directory into its own descendant is rejected, and
    // the rejection names the descendant rule.
    access.mkdir(&path("/d")?, false)?;
    assert_eq!(
        access.rename(&path("/d")?, &path("/d/inner")?),
        Err(VfsError::InvalidPath {
            path: "/d".to_owned(),
            reason: PathReason::IntoDescendant,
        })
    );
    assert!(access.exists(&path("/d")?)?);
    assert_no_temp_files_left(temp.path())?;
    Ok(())
}
