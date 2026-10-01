//! Tests for links under a rooted backend: escapes, dangling links,
//! and path operations that act on a link itself.

use super::*;

/// Creates a directory link, returning false when the OS refuses.
/// Windows uses a junction (no privilege required, unlike
/// `symlink_dir`); Unix uses a plain symlink.
#[cfg(windows)]
fn make_dir_link(link: &Path, target: &Path) -> bool {
    std::process::Command::new("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .status()
        .is_ok_and(|status| status.success())
}

/// Creates a directory link, returning false when the OS refuses.
#[cfg(unix)]
fn make_dir_link(link: &Path, target: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

/// Creates a file link, returning false only when Windows refuses
/// for want of the symlink privilege (raw OS error 1314,
/// `ERROR_PRIVILEGE_NOT_HELD`). Every other failure is an error.
#[cfg(windows)]
fn make_file_link(link: &Path, target: &Path) -> Result<bool, VfsError> {
    const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
    match std::os::windows::fs::symlink_file(target, link) {
        Ok(()) => Ok(true),
        Err(err) if err.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
            eprintln!(
                "skipped: Windows refused a file symlink without the symlink privilege \
                 (enable Developer Mode or run elevated)"
            );
            Ok(false)
        }
        Err(err) => Err(map_io("creating the file link", &err)),
    }
}

/// Creates a file link.
#[cfg(unix)]
fn make_file_link(link: &Path, target: &Path) -> Result<bool, VfsError> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|err| map_io("creating the file link", &err))?;
    Ok(true)
}

/// Whether `host` itself is a link, without following it.
fn is_link(host: &Path) -> bool {
    fs::symlink_metadata(host).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// Makes `link` a dangling directory link: a link to `target`, which
/// is then removed. A directory link needs no privilege on any machine.
fn make_dangling_dir_link(link: &Path, target: &Path) -> Result<(), VfsError> {
    fs::create_dir(target).map_err(|err| map_io("creating the link target", &err))?;
    assert!(
        make_dir_link(link, target),
        "the directory link must be created"
    );
    fs::remove_dir(target).map_err(|err| map_io("removing the link target", &err))
}

/// Asserts `result` is the dangling-link refusal, not some other
/// denial or an OS failure.
fn assert_dangling_refusal<T: std::fmt::Debug>(result: Result<T, VfsError>, operation: &str) {
    match result {
        Err(VfsError::PermissionDenied { reason, .. }) => assert!(
            reason.contains("passes through a dangling symbolic link"),
            "{operation}: {reason}"
        ),
        other => panic!("{operation} must be refused as a dangling link, got {other:?}"),
    }
}

#[test]
fn a_rooted_backend_rejects_links_that_escape_the_mount_root() -> Result<(), VfsError> {
    let outside = TempDir::new()?;
    fs::write(outside.path().join("secret.txt"), b"classified")
        .map_err(|err| map_io("seeding the outside file", &err))?;
    let root = TempDir::new()?;
    if !make_dir_link(&root.path().join("link"), outside.path()) {
        eprintln!(
            "skipped: the host refused to create a directory link, so there is no link \
             to escape through"
        );
        return Ok(());
    }
    let mut access = rooted_access(root.path())?;
    assert!(
        matches!(
            access.read(&path("/link/secret.txt")?),
            Err(VfsError::PermissionDenied { .. })
        ),
        "a read through the escaping link must be denied"
    );
    assert!(
        matches!(
            access.write(&path("/link/new.txt")?, b"x"),
            Err(VfsError::PermissionDenied { .. })
        ),
        "a write through the escaping link must be denied"
    );
    assert!(
        matches!(
            access.append(&path("/link/secret.txt")?, b"x"),
            Err(VfsError::PermissionDenied { .. })
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
fn removing_a_link_to_an_in_root_file_removes_the_link_and_keeps_the_target() -> Result<(), VfsError>
{
    let root = TempDir::new()?;
    fs::write(root.path().join("target.txt"), b"kept")
        .map_err(|err| map_io("seeding the target file", &err))?;
    if !make_file_link(&root.path().join("link"), &root.path().join("target.txt"))? {
        return Ok(());
    }
    let mut access = rooted_access(root.path())?;
    access.remove(&path("/link")?, false)?;
    assert!(!is_link(&root.path().join("link")), "the link must be gone");
    assert_eq!(access.read(&path("/target.txt")?)?, b"kept");
    Ok(())
}

#[test]
fn path_operations_act_on_a_link_to_an_outside_file_as_a_link() -> Result<(), VfsError> {
    let outside = TempDir::new()?;
    let secret = outside.path().join("secret.txt");
    fs::write(&secret, b"classified").map_err(|err| map_io("seeding the outside file", &err))?;
    let root = TempDir::new()?;
    if !make_file_link(&root.path().join("link"), &secret)? {
        return Ok(());
    }
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
            Err(VfsError::PermissionDenied { .. })
        ),
        "a read through the escaping link must be denied"
    );
    assert!(
        matches!(
            access.write(&path("/link")?, b"x"),
            Err(VfsError::PermissionDenied { .. })
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
fn removing_a_dangling_link_succeeds() -> Result<(), VfsError> {
    let root = TempDir::new()?;
    if !make_file_link(
        &root.path().join("dangling"),
        &root.path().join("missing.txt"),
    )? {
        return Ok(());
    }
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
    let outside = TempDir::new()?;
    for target_in_root in [true, false] {
        let root = TempDir::new()?;
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
    let root = TempDir::new()?;
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
    let root = TempDir::new()?;
    let target = root.path().join("real");
    fs::create_dir(&target).map_err(|err| map_io("creating the target directory", &err))?;
    fs::write(target.join("keep.txt"), b"kept")
        .map_err(|err| map_io("seeding the target file", &err))?;
    let mut access = rooted_access(root.path())?;
    for recursive in [false, true] {
        let link = root.path().join("dirlink");
        assert!(
            make_dir_link(&link, &target),
            "the directory link must be created"
        );
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
fn path_operations_act_on_a_directory_link_to_an_outside_directory_as_a_link()
-> Result<(), VfsError> {
    let outside = TempDir::new()?;
    let secret = outside.path().join("secret.txt");
    fs::write(&secret, b"classified").map_err(|err| map_io("seeding the outside file", &err))?;
    let root = TempDir::new()?;
    assert!(
        make_dir_link(&root.path().join("link"), outside.path()),
        "the directory link must be created"
    );
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
