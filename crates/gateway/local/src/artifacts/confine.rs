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
//! Windows the equivalent restriction is a DACL granted only to the current
//! process token's SID.

#[path = "confine-private-root.rs"]
mod private_root;

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use super::Result;
use crate::error::LocalError;
pub(crate) use private_root::enforce_private_cache_root;

/// Parses one `whoami /user /fo csv /nh` record into its canonical SID.
#[cfg(any(windows, test))]
pub(super) fn parse_whoami_user_sid(
    root: &Path,
    command_succeeded: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<String> {
    let not_private = |reason: String| LocalError::CacheNotPrivate {
        path: root.to_owned(),
        reason,
    };
    if !command_succeeded {
        let detail = String::from_utf8_lossy(stderr);
        let detail = detail.trim();
        return Err(not_private(if detail.is_empty() {
            "whoami identity query failed".to_owned()
        } else {
            format!("whoami identity query failed: {detail}")
        }));
    }

    let record = stdout
        .strip_suffix(b"\r\n")
        .or_else(|| stdout.strip_suffix(b"\n"))
        .unwrap_or(stdout);
    if record.is_empty() {
        return Err(not_private("whoami identity output is empty".to_owned()));
    }
    if record.contains(&b'\r') || record.contains(&b'\n') {
        return Err(not_private(
            "whoami identity output contains multiple records".to_owned(),
        ));
    }

    let Some(inner) = record
        .strip_prefix(b"\"")
        .and_then(|value| value.strip_suffix(b"\""))
    else {
        return Err(not_private(
            "whoami identity output is not quoted CSV".to_owned(),
        ));
    };
    let mut separators = inner
        .windows(3)
        .enumerate()
        .filter(|(_, window)| *window == b"\",\"");
    let Some((separator, _)) = separators.next() else {
        return Err(not_private(
            "whoami identity output does not contain two fields".to_owned(),
        ));
    };
    if separators.next().is_some() {
        return Err(not_private(
            "whoami identity output contains extra fields".to_owned(),
        ));
    }

    let account = &inner[..separator];
    let sid_bytes = &inner[separator + 3..];
    if !account.iter().any(|byte| !byte.is_ascii_whitespace()) || account.contains(&b'"') {
        return Err(not_private(
            "whoami identity output has an invalid account".to_owned(),
        ));
    }
    let sid = std::str::from_utf8(sid_bytes)
        .map_err(|_| not_private("whoami identity output has a non-UTF-8 SID".to_owned()))?;
    if !is_canonical_windows_sid(sid) {
        return Err(not_private(
            "whoami identity output has a non-canonical SID".to_owned(),
        ));
    }
    Ok(sid.to_owned())
}

#[cfg(any(windows, test))]
fn is_canonical_windows_sid(sid: &str) -> bool {
    fn canonical_decimal(value: &str) -> bool {
        !value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && (value == "0" || !value.starts_with('0'))
    }

    let mut components = sid.split('-');
    if components.next() != Some("S") || components.next() != Some("1") {
        return false;
    }
    let Some(authority) = components.next() else {
        return false;
    };
    if !canonical_decimal(authority)
        || authority
            .parse::<u64>()
            .map_or(true, |value| value > 0xFFFF_FFFF_FFFF)
    {
        return false;
    }

    let mut subauthority_count = 0;
    for subauthority in components {
        subauthority_count += 1;
        if subauthority_count > 15
            || !canonical_decimal(subauthority)
            || subauthority.parse::<u32>().is_err()
        {
            return false;
        }
    }
    subauthority_count != 0
}

/// Renders a validated SID as an `icacls /grant:r` access specification.
#[cfg(any(windows, test))]
#[must_use]
pub(super) fn windows_sid_grant(sid: &str) -> String {
    format!("*{sid}:(OI)(CI)F")
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

/// The provenance marker for a `.part` staging file: the source URL the
/// partial is being downloaded from. A partial without a marker naming the
/// same source is never resumed - appending bytes of unknown provenance
/// would poison the digest.
pub(crate) fn source_marker_path(part: &Path) -> PathBuf {
    let mut name = part.as_os_str().to_owned();
    name.push(".source");
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
