//! Cache-root path confinement: no traversal, no symlink/reparse escape.
//!
//! # Threat model and contract (ART-006)
//!
//! The names these helpers confine come from *config* and *download URLs* - a
//! model filename, an archive entry, a source-derived cache slot. The guard
//! rejects any such name that escapes the cache root: a `..` component, an
//! absolute path, or any component that resolves through a symlink/reparse
//! point (each interior component is `symlink_metadata`-checked as it is walked,
//! so an attacker-planted link cannot redirect a write outside the root).
//!
//! These are check-then-operate sequences, so a purely handle-relative
//! (`openat(O_NOFOLLOW)`) implementation would close a residual filesystem race.
//! That race is scoped away by an **enforced** ownership precondition rather
//! than only a documented one: [`enforce_private_cache_root`] makes the cache
//! root owner-private (Unix mode `0700`) and refuses to proceed if group/world
//! access cannot be removed ([`LocalError::CacheNotPrivate`]). With no untrusted
//! party able to write inside the root, a local actor able to race directory
//! creation there already holds the operator's privileges, so the confinement's
//! job is to stop malicious *names*, not to defend a shared-tenant cache. On
//! Windows the equivalent restriction is the per-user profile ACL that the
//! default `%USERPROFILE%\.promptforge` inherits.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use super::Result;
use crate::error::LocalError;

/// Enforces the private-cache ownership precondition on the cache `root`.
///
/// This is a real, verified restriction on every platform (ART-006), never a
/// silent no-op:
/// - Unix: `chmod 0700`, then verify no group/world mode bits remain.
/// - Windows: strip inherited ACEs and grant the current account full control
///   (`icacls /inheritance:r /grant:r`), then verify no broad principal
///   (Everyone / Authenticated Users / Users) still appears in the DACL.
///
/// Returns [`LocalError::CacheNotPrivate`] when the root cannot be made private.
#[cfg(unix)]
pub(crate) fn enforce_private_cache_root(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(|source| {
        LocalError::Io {
            operation: "restrict cache root to owner-only",
            path: root.to_owned(),
            source,
        }
    })?;
    let mode = fs::symlink_metadata(root)
        .map_err(|source| LocalError::Io {
            operation: "inspect cache root permissions",
            path: root.to_owned(),
            source,
        })?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(LocalError::CacheNotPrivate {
            path: root.to_owned(),
            reason: format!("filesystem mode {mode:o} still allows group/world access"),
        });
    }
    Ok(())
}

/// Broad, multi-user principals that must never retain access to the cache.
///
/// Matched by the well-known English `icacls` names plus a couple of common
/// localizations; the primary guarantee is the `/inheritance:r` strip, which is
/// SID-based and locale-independent, so this listing is a defense-in-depth
/// verification rather than the sole enforcement.
/// Each entry is matched with its ACE `:` suffix (icacls renders an access
/// entry as `principal:(perms)`), so a cache path that merely *contains* one of
/// these words (for example `C:\Users\...`) is not a false positive.
#[cfg(windows)]
const BROAD_WINDOWS_PRINCIPALS: [&str; 5] = [
    "Everyone:",
    "Authenticated Users:",
    "\\Users:",
    "Todos:", // es-* localization of "Everyone"
    "Jeder:", // de-* localization of "Everyone"
];

#[cfg(windows)]
pub(crate) fn enforce_private_cache_root(root: &Path) -> Result<()> {
    let account = current_windows_account(root)?;
    set_owner_only_windows_dacl(root, &account)?;
    verify_private_windows_dacl(root)
}

/// The `DOMAIN\user` (or bare `user`) icacls principal for the current process.
#[cfg(windows)]
fn current_windows_account(root: &Path) -> Result<String> {
    let Some(user) = std::env::var("USERNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(LocalError::CacheNotPrivate {
            path: root.to_owned(),
            reason: "USERNAME is not set, cannot restrict the cache DACL".to_owned(),
        });
    };
    Ok(
        match std::env::var("USERDOMAIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(domain) => format!("{domain}\\{user}"),
            None => user,
        },
    )
}

/// Removes inherited ACEs and grants the current account sole full control.
#[cfg(windows)]
fn set_owner_only_windows_dacl(root: &Path, account: &str) -> Result<()> {
    let output = std::process::Command::new("icacls")
        .arg(root)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{account}:(OI)(CI)F"))
        .output()
        .map_err(|source| LocalError::Io {
            operation: "run icacls to restrict cache DACL",
            path: root.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(LocalError::CacheNotPrivate {
            path: root.to_owned(),
            reason: format!(
                "icacls restriction failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(())
}

/// Verifies no broad multi-user principal retains access after the restriction.
#[cfg(windows)]
fn verify_private_windows_dacl(root: &Path) -> Result<()> {
    let output = std::process::Command::new("icacls")
        .arg(root)
        .output()
        .map_err(|source| LocalError::Io {
            operation: "run icacls to verify cache DACL",
            path: root.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(LocalError::CacheNotPrivate {
            path: root.to_owned(),
            reason: "could not read back the cache DACL to verify it".to_owned(),
        });
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    for principal in BROAD_WINDOWS_PRINCIPALS {
        if listing.contains(principal) {
            return Err(LocalError::CacheNotPrivate {
                path: root.to_owned(),
                reason: format!(
                    "a broad principal ({}) still has DACL access",
                    principal.trim_end_matches(':')
                ),
            });
        }
    }
    Ok(())
}

/// Whether `path` is a non-empty relative path of only normal components.
pub(crate) fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// The sibling `<path>.part` staging name for an atomic publish.
pub(crate) fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// Creates `directory` under `root`, refusing any symlink/reparse component.
///
/// # Errors
/// Returns [`LocalError`] when the path escapes `root` or a component is unsafe.
pub(crate) fn ensure_cache_directory(root: &Path, directory: &Path) -> Result<()> {
    if directory == root {
        fs::create_dir_all(root).map_err(|source| LocalError::Io {
            operation: "create cache directory",
            path: root.to_owned(),
            source,
        })?;
        return validate_tree_path(root, root);
    }
    validate_tree_path(root, directory)?;
    let relative = confined_relative(root, directory)?;
    let mut current = root.to_owned();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                    return Err(LocalError::UnsafeCachePath { path: current });
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                if let Err(source) = fs::create_dir(&current)
                    && source.kind() != io::ErrorKind::AlreadyExists
                {
                    return Err(LocalError::Io {
                        operation: "create cache directory",
                        path: current.clone(),
                        source,
                    });
                }
                validate_tree_path(root, &current)?;
            }
            Err(source) => {
                return Err(LocalError::Io {
                    operation: "inspect cache directory",
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

/// Removes a file or directory under `root`, refusing symlink/reparse targets.
///
/// # Errors
/// Returns [`LocalError`] when the path is unsafe or removal fails.
pub(crate) fn remove_cache_entry(root: &Path, path: &Path) -> Result<()> {
    validate_tree_path(root, path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(LocalError::Io {
                operation: "inspect cache entry",
                path: path.to_owned(),
                source,
            });
        }
    };
    if is_link_or_reparse(&metadata) {
        return Err(LocalError::UnsafeCachePath {
            path: path.to_owned(),
        });
    }
    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|source| LocalError::Io {
        operation: "remove cache entry",
        path: path.to_owned(),
        source,
    })
}

/// Atomically renames `source` to `destination`, both confined to `root`.
///
/// # Errors
/// Returns [`LocalError`] when either path is unsafe or the rename fails.
pub(crate) fn rename_confined(root: &Path, source: &Path, destination: &Path) -> Result<()> {
    validate_tree_path(root, source)?;
    validate_tree_path(root, destination)?;
    fs::rename(source, destination).map_err(|error| LocalError::Io {
        operation: "atomically install artifact",
        path: destination.to_owned(),
        source: error,
    })
}

/// Rejects a target that escapes `root` via traversal or a symlink component.
///
/// # Errors
/// Returns [`LocalError::UnsafeCachePath`] when `path` is not confined.
pub(crate) fn validate_cache_path(root: &Path, path: &Path) -> Result<()> {
    validate_tree_path(root, path)
}

/// Walks `path` component by component under `root`, refusing any symlink or
/// reparse point and any non-directory interior component.
///
/// # Errors
/// Returns [`LocalError`] when the path escapes `root` or a component is unsafe.
pub(super) fn validate_tree_path(root: &Path, path: &Path) -> Result<()> {
    let relative = confined_relative(root, path)?;
    let root_metadata = fs::symlink_metadata(root).map_err(|source| LocalError::Io {
        operation: "inspect cache root",
        path: root.to_owned(),
        source,
    })?;
    if is_link_or_reparse(&root_metadata) || !root_metadata.is_dir() {
        return Err(LocalError::UnsafeCachePath {
            path: root.to_owned(),
        });
    }
    let mut current = root.to_owned();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if is_link_or_reparse(&metadata)
                    || (index + 1 != components.len() && !metadata.is_dir())
                {
                    return Err(LocalError::UnsafeCachePath { path: current });
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(LocalError::Io {
                    operation: "inspect cache path",
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn confined_relative<'a>(root: &Path, path: &'a Path) -> Result<&'a Path> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| LocalError::UnsafeCachePath {
            path: path.to_owned(),
        })?;
    if !relative.as_os_str().is_empty() && !safe_relative_path(relative) {
        return Err(LocalError::UnsafeCachePath {
            path: path.to_owned(),
        });
    }
    Ok(relative)
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// Writes `contents` to `path` and fsyncs before returning.
///
/// # Errors
/// Returns [`LocalError::Io`] when creating, writing, or syncing fails.
pub(crate) fn write_synced(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = File::create(path).map_err(|source| LocalError::Io {
        operation: "create install marker",
        path: path.to_owned(),
        source,
    })?;
    file.write_all(contents).map_err(|source| LocalError::Io {
        operation: "write install marker",
        path: path.to_owned(),
        source,
    })?;
    file.sync_all().map_err(|source| LocalError::Io {
        operation: "sync install marker",
        path: path.to_owned(),
        source,
    })
}
