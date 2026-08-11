//! Cache-root path confinement: no traversal, no symlink/reparse escape.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use super::Result;
use crate::local::error::LocalError;

/// Whether `path` is a non-empty relative path of only normal components.
pub(super) fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// The sibling `<path>.part` staging name for an atomic publish.
pub(super) fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// Creates `directory` under `root`, refusing any symlink/reparse component.
///
/// # Errors
/// Returns [`LocalError`] when the path escapes `root` or a component is unsafe.
pub(super) fn ensure_cache_directory(root: &Path, directory: &Path) -> Result<()> {
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
pub(super) fn remove_cache_entry(root: &Path, path: &Path) -> Result<()> {
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
pub(super) fn rename_confined(root: &Path, source: &Path, destination: &Path) -> Result<()> {
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
pub(super) fn validate_cache_path(root: &Path, path: &Path) -> Result<()> {
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
pub(super) fn write_synced(path: &Path, contents: &[u8]) -> Result<()> {
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
