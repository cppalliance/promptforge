//! Owner-only, no-follow, atomic filesystem primitives for the dump tree.
//!
//! Dumped store files and turn traces can hold raw prompts, tool arguments and
//! results, and model output, so this module is the one place that writes them:
//!
//! - Directories and files are created owner-only from the outset: `0o700` /
//!   `0o600` on Unix, and on Windows inheritance is stripped and full control
//!   is granted to the current user alone via `icacls`.
//! - Every write and every directory creation refuses a symlink or Windows
//!   reparse point at the target and at every existing ancestor, so a planted
//!   link (leaf or ancestor) cannot redirect a write outside the dump tree.
//! - Each file is written to a sibling temporary and atomically renamed over
//!   its destination, so an interrupted write cannot truncate a prior file.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

/// Reports whether `meta` describes a symlink or a Windows reparse point.
fn is_reparse(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Returns an error when `path` or any of its existing ancestors is a symlink
/// or reparse point.
///
/// Checking every ancestor (not just the leaf) prevents an escape through a
/// planted link anywhere in the path, including when the full path already
/// exists as a directory. A component that does not exist yet is fine; any
/// stat error other than "not found" is propagated.
pub(crate) fn reject_reparse_ancestors(path: &Path) -> io::Result<()> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if is_reparse(&meta) => {
                return Err(io::Error::other(format!(
                    "refusing to traverse symlink or reparse point {}",
                    ancestor.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Creates `dir` and every missing parent with owner-only access.
///
/// Refuses to traverse an existing symlink or reparse point at any component,
/// so the created tree is anchored entirely under real directories.
pub(crate) fn create_dir_all_secure(dir: &Path) -> io::Result<()> {
    // Verify every existing ancestor first, even when `dir` already exists as a
    // directory: an escape can hide in an ancestor of an already-present path.
    reject_reparse_ancestors(dir)?;
    create_dir_all_inner(dir)
}

/// Recursive creation helper; ancestors were already vetted by the caller.
fn create_dir_all_inner(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    match fs::symlink_metadata(dir) {
        Ok(meta) if is_reparse(&meta) => {
            return Err(io::Error::other(format!(
                "refusing to traverse symlink or reparse point {}",
                dir.display()
            )));
        }
        Ok(meta) if meta.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(io::Error::other(format!(
                "{} exists and is not a directory",
                dir.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if let Some(parent) = dir.parent()
        && !parent.as_os_str().is_empty()
    {
        create_dir_all_inner(parent)?;
    }
    match create_dir_restricted(dir) {
        Ok(()) => Ok(()),
        // A concurrent creator won the race; accept a real directory but still
        // reject a link that appeared in the meantime.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            match fs::symlink_metadata(dir) {
                Ok(meta) if !is_reparse(&meta) && meta.is_dir() => Ok(()),
                Ok(_) => Err(io::Error::other(format!(
                    "{} exists and is not a plain directory",
                    dir.display()
                ))),
                Err(other) => Err(other),
            }
        }
        Err(error) => Err(error),
    }
}

/// Atomically writes `contents` to `path` with owner-only access.
///
/// Refuses a symlink or reparse point at `path` and at every existing ancestor,
/// writes to a sibling temporary with restricted permissions, flushes it,
/// restricts it to the owner, then renames it over the destination. A failure
/// removes the temporary so no partial file lingers.
pub(crate) fn write_atomic_secure(path: &Path, contents: &[u8]) -> io::Result<()> {
    reject_reparse_ancestors(path)?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => Path::new(".").to_path_buf(),
    };
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} names no file", path.display())))?
        .to_string_lossy()
        .into_owned();
    let temp = parent.join(format!(".{file_name}.tmp{:016x}", fastrand::u64(..)));

    let write_result = (|| -> io::Result<()> {
        let mut file = create_restricted(&temp)?;
        file.write_all(contents)?;
        file.flush()
    })();
    if let Err(error) = write_result {
        let _ignored = fs::remove_file(&temp);
        return Err(error);
    }
    // Restrict the finished temp to the owner before it takes the destination's
    // name; an explicit DACL travels with the file across the rename. On Unix
    // the mode was already set at creation, so this is Windows-only.
    #[cfg(windows)]
    if let Err(error) = restrict_to_owner(&temp, false) {
        let _ignored = fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ignored = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

/// Creates a directory owner-only.
fn create_dir_restricted(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        fs::DirBuilder::new().mode(0o700).create(dir)
    }
    #[cfg(windows)]
    {
        fs::create_dir(dir)?;
        restrict_to_owner(dir, true)
    }
    #[cfg(not(any(unix, windows)))]
    {
        fs::create_dir(dir)
    }
}

/// Creates (truncating) a file owner-only. On Unix the mode is set at creation;
/// on Windows the ACL is applied separately once the file is closed
/// ([`restrict_file_to_owner`]).
fn create_restricted(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

/// Removes inherited access and grants full control to the current user alone,
/// via `icacls`. Windows only.
#[cfg(windows)]
fn restrict_to_owner(path: &Path, is_dir: bool) -> io::Result<()> {
    let user = std::env::var("USERNAME").map_err(|_| {
        io::Error::other("USERNAME is unset; cannot restrict the dump ACL to the owner")
    })?;
    let grant = if is_dir {
        format!("{user}:(OI)(CI)F")
    } else {
        format!("{user}:F")
    };
    let output = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(&grant)
        .arg("/Q")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "icacls could not restrict {} to owner-only: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{create_dir_all_secure, reject_reparse_ancestors, write_atomic_secure};

    #[test]
    fn atomic_write_creates_then_replaces_contents() {
        let directory = tempfile::tempdir().expect("create fixture directory");
        let target = directory.path().join("nested").join("evidence.md");
        create_dir_all_secure(target.parent().expect("has parent")).expect("create dirs");

        write_atomic_secure(&target, b"first").expect("first write");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read first"),
            "first"
        );

        write_atomic_secure(&target, b"second").expect("second write");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read second"),
            "second"
        );

        // No temporary siblings must linger after a successful write.
        let leftovers: Vec<_> = std::fs::read_dir(target.parent().expect("has parent"))
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temp files may remain: {leftovers:?}"
        );
    }

    #[test]
    fn reject_reparse_ancestors_accepts_a_plain_file_or_missing_path() {
        let directory = tempfile::tempdir().expect("create fixture directory");
        let missing = directory.path().join("absent").join("deeper");
        reject_reparse_ancestors(&missing).expect("a missing path with real ancestors is fine");

        let plain = directory.path().join("plain.txt");
        std::fs::write(&plain, "x").expect("write plain file");
        reject_reparse_ancestors(&plain).expect("a plain file is fine");
    }

    #[cfg(unix)]
    #[test]
    fn write_refuses_to_follow_a_symlinked_destination() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("create fixture directory");
        let outside = directory.path().join("outside.txt");
        std::fs::write(&outside, "original").expect("write outside target");
        let link = directory.path().join("link.txt");
        symlink(&outside, &link).expect("create symlink");

        write_atomic_secure(&link, b"attacker")
            .expect_err("writing through a symlink must be refused");
        assert_eq!(
            std::fs::read_to_string(&outside).expect("read outside"),
            "original",
            "the symlink target must be untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn created_files_and_dirs_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("create fixture directory");
        let dir = directory.path().join("secure");
        create_dir_all_secure(&dir).expect("create secure dir");
        let dir_mode = std::fs::metadata(&dir)
            .expect("stat dir")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "dump directories must be owner-only");

        let file = dir.join("secret.json");
        write_atomic_secure(&file, b"{}").expect("write secure file");
        let file_mode = std::fs::metadata(&file)
            .expect("stat file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "dumped files must be owner-only");
    }

    #[cfg(unix)]
    #[test]
    fn create_dir_all_refuses_a_symlinked_component() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("create fixture directory");
        let real_elsewhere = directory.path().join("elsewhere");
        std::fs::create_dir(&real_elsewhere).expect("create real dir");
        let link = directory.path().join("link");
        symlink(&real_elsewhere, &link).expect("create dir symlink");

        create_dir_all_secure(&link.join("child"))
            .expect_err("traversing a symlinked component must be refused");
        assert!(
            !real_elsewhere.join("child").exists(),
            "nothing may be created through the link"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_dir_all_refuses_a_symlinked_ancestor_even_when_the_path_exists() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("create fixture directory");
        let base = directory.path().join("base");
        let child = base.join("child");
        std::fs::create_dir_all(&child).expect("create the real tree");
        let link = directory.path().join("link");
        symlink(&base, &link).expect("symlink to base");

        // `link/child` fully resolves to an existing directory through the
        // symlinked ancestor `link`; it must still be refused.
        create_dir_all_secure(&link.join("child")).expect_err(
            "a symlinked ancestor must be refused even when the full path already exists",
        );
        // The same escape must be refused for a write under that ancestor.
        write_atomic_secure(&link.join("child").join("x.txt"), b"nope")
            .expect_err("writing under a symlinked ancestor must be refused");
    }

    #[cfg(windows)]
    #[test]
    fn created_files_are_owner_only_on_windows() {
        let directory = tempfile::tempdir().expect("create fixture directory");
        let dir = directory.path().join("secure");
        create_dir_all_secure(&dir).expect("create secure dir");
        let file = dir.join("secret.json");
        write_atomic_secure(&file, b"{}").expect("write secure file");

        let user = std::env::var("USERNAME").expect("USERNAME is set on Windows");
        for target in [&dir, &file] {
            let output = std::process::Command::new("icacls")
                .arg(target)
                .output()
                .expect("run icacls");
            let acl = String::from_utf8_lossy(&output.stdout);
            assert!(
                acl.contains(&user),
                "the owner must retain access to {}: {acl}",
                target.display()
            );
            // `/inheritance:r` removed every inherited ACE, so no `(I)` flag
            // (and therefore no inherited Users/Everyone grant) may remain.
            assert!(
                !acl.contains("(I)"),
                "inherited access must be stripped from {}: {acl}",
                target.display()
            );
        }
    }
}
