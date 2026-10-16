//! Tests for links under a rooted backend: escapes, dangling links,
//! and path operations that act on a link itself.

use super::*;

/// Creates a directory link. Windows uses a junction (no privilege
/// required, unlike `symlink_dir`); Unix uses a plain symlink.
#[cfg(windows)]
fn make_dir_link(link: &Path, target: &Path) -> Result<(), VfsError> {
    let status = std::process::Command::new("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .status()
        .map_err(|err| map_io("creating the directory link", &err))?;
    if status.success() {
        Ok(())
    } else {
        Err(VfsError::Backend {
            message: format!("mklink /J failed with {status}"),
        })
    }
}

/// Creates a directory link.
#[cfg(unix)]
fn make_dir_link(link: &Path, target: &Path) -> Result<(), VfsError> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|err| map_io("creating the directory link", &err))
}

/// Creates a file link. On Windows that needs the symlink privilege,
/// so the tests that call this are ignored there.
#[cfg(windows)]
fn make_file_link(link: &Path, target: &Path) -> Result<(), VfsError> {
    std::os::windows::fs::symlink_file(target, link)
        .map_err(|err| map_io("creating the file link", &err))
}

/// Creates a file link.
#[cfg(unix)]
fn make_file_link(link: &Path, target: &Path) -> Result<(), VfsError> {
    std::os::unix::fs::symlink(target, link).map_err(|err| map_io("creating the file link", &err))
}

/// Whether `real` itself is a link, without following it.
fn is_link(real: &Path) -> bool {
    fs::symlink_metadata(real).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// Makes `link` a dangling directory link: a link to `target`, which
/// is then removed. A directory link needs no privilege on any machine.
fn make_dangling_dir_link(link: &Path, target: &Path) -> Result<(), VfsError> {
    fs::create_dir(target).map_err(|err| map_io("creating the link target", &err))?;
    make_dir_link(link, target)?;
    fs::remove_dir(target).map_err(|err| map_io("removing the link target", &err))
}

/// Asserts `result` is the dangling-link refusal, not an escape or an
/// OS failure. Both refusals are `OutsideRoot`, told apart by detail.
fn assert_dangling_refusal<T: std::fmt::Debug>(result: Result<T, VfsError>, operation: &str) {
    match result {
        Err(VfsError::OutsideRoot { detail, .. }) => assert!(
            detail.contains("passes through a dangling symbolic link"),
            "{operation}: {detail}"
        ),
        other => panic!("{operation} must be refused as a dangling link, got {other:?}"),
    }
}

#[test]
fn a_rooted_backend_rejects_links_that_escape_the_mount_root() -> Result<(), VfsError> {
    let label = "a_rooted_backend_rejects_links_that_escape_the_mount_root";
    let outside = TempRoot::new(label);
    fs::write(outside.path().join("secret.txt"), b"classified")
        .map_err(|err| map_io("seeding the outside file", &err))?;
    let root = TempRoot::new(label);
    make_dir_link(&root.path().join("link"), outside.path())?;
    let mut access = rooted_access(root.path())?;
    assert!(
        matches!(
            access.read(&path("/link/secret.txt")?),
            Err(VfsError::OutsideRoot { .. })
        ),
        "a read through the escaping link must be denied"
    );
    assert!(
        matches!(
            access.write(&path("/link/new.txt")?, b"x"),
            Err(VfsError::OutsideRoot { .. })
        ),
        "a write through the escaping link must be denied"
    );
    assert!(
        matches!(
            access.append(&path("/link/secret.txt")?, b"x"),
            Err(VfsError::OutsideRoot { .. })
        ),
        "an append through the escaping link must be denied"
    );
    assert_eq!(
        fs::read(outside.path().join("secret.txt"))
            .map_err(|err| map_io("reading the outside file", &err))?,
        b"classified"
    );
    assert!(!outside.path().join("new.txt").exists());
    Ok(())
}

#[test]
#[cfg_attr(windows, ignore = "needs the symlink privilege")]
fn removing_a_link_to_an_in_root_file_removes_the_link_and_keeps_the_target() -> Result<(), VfsError>
{
    let root =
        TempRoot::new("removing_a_link_to_an_in_root_file_removes_the_link_and_keeps_the_target");
    fs::write(root.path().join("target.txt"), b"kept")
        .map_err(|err| map_io("seeding the target file", &err))?;
    make_file_link(&root.path().join("link"), &root.path().join("target.txt"))?;
    let mut access = rooted_access(root.path())?;
    access.remove(&path("/link")?, false)?;
    assert!(!is_link(&root.path().join("link")), "the link must be gone");
    assert_eq!(access.read(&path("/target.txt")?)?, b"kept");
    Ok(())
}

#[test]
#[cfg_attr(windows, ignore = "needs the symlink privilege")]
fn path_operations_act_on_a_link_to_an_outside_file_as_a_link() -> Result<(), VfsError> {
    let label = "path_operations_act_on_a_link_to_an_outside_file_as_a_link";
    let outside = TempRoot::new(label);
    let secret = outside.path().join("secret.txt");
    fs::write(&secret, b"classified").map_err(|err| map_io("seeding the outside file", &err))?;
    let root = TempRoot::new(label);
    make_file_link(&root.path().join("link"), &secret)?;
    let mut access = rooted_access(root.path())?;
    assert!(access.exists(&path("/link")?)?);
    assert_eq!(access.stat(&path("/link")?)?.file_type, FileType::Symlink);
    assert!(matches!(
        access.mkdir(&path("/link")?, false),
        Err(VfsError::AlreadyExists { .. })
    ));
    assert!(
        matches!(
            access.read(&path("/link")?),
            Err(VfsError::OutsideRoot { .. })
        ),
        "a read through the escaping link must be denied"
    );
    assert!(
        matches!(
            access.write(&path("/link")?, b"x"),
            Err(VfsError::OutsideRoot { .. })
        ),
        "a write through the escaping link must be denied"
    );
    access.rename(&path("/link")?, &path("/moved")?)?;
    assert!(!access.exists(&path("/link")?)?);
    assert!(
        is_link(&root.path().join("moved")),
        "the link itself must move"
    );
    assert!(
        secret.is_file(),
        "the outside target must stay where it was"
    );
    access.remove(&path("/moved")?, false)?;
    assert!(!access.exists(&path("/moved")?)?);
    assert_eq!(
        fs::read(&secret).map_err(|err| map_io("reading the outside file", &err))?,
        b"classified"
    );
    Ok(())
}

#[test]
#[cfg_attr(windows, ignore = "needs the symlink privilege")]
fn removing_a_dangling_link_succeeds() -> Result<(), VfsError> {
    let root = TempRoot::new("removing_a_dangling_link_succeeds");
    make_file_link(
        &root.path().join("dangling"),
        &root.path().join("missing.txt"),
    )?;
    let mut access = rooted_access(root.path())?;
    access.remove(&path("/dangling")?, false)?;
    assert!(
        !is_link(&root.path().join("dangling")),
        "the link must be gone"
    );
    Ok(())
}

#[test]
fn content_operations_refuse_a_path_through_a_dangling_link() -> Result<(), VfsError> {
    let label = "content_operations_refuse_a_path_through_a_dangling_link";
    let outside = TempRoot::new(label);
    for target_in_root in [true, false] {
        let root = TempRoot::new(label);
        let target = if target_in_root {
            root.path().join("gone")
        } else {
            outside.path().join("gone")
        };
        make_dangling_dir_link(&root.path().join("link"), &target)?;
        let mut access = rooted_access(root.path())?;
        for spelled in ["/link", "/link/new.txt"] {
            let at = path(spelled)?;
            assert_dangling_refusal(access.append(&at, b"x"), &format!("append {spelled}"));
            assert_dangling_refusal(access.write(&at, b"x"), &format!("write {spelled}"));
            assert_dangling_refusal(access.read(&at), &format!("read {spelled}"));
            assert_dangling_refusal(access.list(&at), &format!("list {spelled}"));
        }
        assert!(
            fs::symlink_metadata(&target).is_err(),
            "nothing may appear at the link's target {}",
            target.display()
        );
    }
    Ok(())
}

#[test]
fn path_operations_act_on_a_dangling_link_itself_and_refuse_a_path_through_it()
-> Result<(), VfsError> {
    let root =
        TempRoot::new("path_operations_act_on_a_dangling_link_itself_and_refuse_a_path_through_it");
    let link = root.path().join("link");
    make_dangling_dir_link(&link, &root.path().join("gone"))?;
    let mut access = rooted_access(root.path())?;
    assert!(access.exists(&path("/link")?)?, "the link itself exists");
    assert_dangling_refusal(
        access.exists(&path("/link/new.txt")?),
        "exists /link/new.txt",
    );
    assert_dangling_refusal(access.mkdir(&path("/link/sub")?, false), "mkdir /link/sub");
    access.remove(&path("/link")?, false)?;
    assert!(!is_link(&link), "the link must be gone");
    assert!(!access.exists(&path("/link")?)?);
    Ok(())
}

#[test]
fn removing_a_directory_link_keeps_the_target_directory_and_its_contents() -> Result<(), VfsError> {
    let root =
        TempRoot::new("removing_a_directory_link_keeps_the_target_directory_and_its_contents");
    let target = root.path().join("real");
    fs::create_dir(&target).map_err(|err| map_io("creating the target directory", &err))?;
    fs::write(target.join("keep.txt"), b"kept")
        .map_err(|err| map_io("seeding the target file", &err))?;
    let mut access = rooted_access(root.path())?;
    for recursive in [false, true] {
        let link = root.path().join("dirlink");
        make_dir_link(&link, &target)?;
        access.remove(&path("/dirlink")?, recursive)?;
        assert!(
            !is_link(&link),
            "the link must be gone (recursive: {recursive})"
        );
        assert_eq!(access.read(&path("/real/keep.txt")?)?, b"kept");
    }
    Ok(())
}

#[test]
fn a_glob_through_an_in_root_directory_link_keeps_the_links_spelling() -> Result<(), VfsError> {
    let root = TempRoot::new("a_glob_through_an_in_root_directory_link_keeps_the_links_spelling");
    let target = root.path().join("real");
    fs::create_dir(&target).map_err(|err| map_io("creating the target directory", &err))?;
    fs::write(target.join("a.txt"), b"a").map_err(|err| map_io("seeding the target file", &err))?;
    make_dir_link(&root.path().join("link"), &target)?;
    let access = rooted_access(root.path())?;
    assert_eq!(access.glob("/link/*")?, ["/link/a.txt".to_owned()]);
    assert_eq!(access.glob("/real/*")?, ["/real/a.txt".to_owned()]);
    Ok(())
}

#[test]
fn path_operations_act_on_a_directory_link_to_an_outside_directory_as_a_link()
-> Result<(), VfsError> {
    let label = "path_operations_act_on_a_directory_link_to_an_outside_directory_as_a_link";
    let outside = TempRoot::new(label);
    let secret = outside.path().join("secret.txt");
    fs::write(&secret, b"classified").map_err(|err| map_io("seeding the outside file", &err))?;
    let root = TempRoot::new(label);
    make_dir_link(&root.path().join("link"), outside.path())?;
    let mut access = rooted_access(root.path())?;
    assert!(access.exists(&path("/link")?)?);
    assert_eq!(access.stat(&path("/link")?)?.file_type, FileType::Symlink);
    access.rename(&path("/link")?, &path("/moved")?)?;
    assert!(!access.exists(&path("/link")?)?);
    assert!(
        is_link(&root.path().join("moved")),
        "the link itself must move"
    );
    access.remove(&path("/moved")?, true)?;
    assert!(!access.exists(&path("/moved")?)?);
    assert_eq!(
        fs::read(&secret).map_err(|err| map_io("reading the outside file", &err))?,
        b"classified"
    );
    Ok(())
}
