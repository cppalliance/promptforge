//! Archive extraction (tar.gz/zip) with traversal and entry-type confinement.
//!
//! Tar symlink entries are materialized as regular-file copies of their
//! targets, never as links: the cache confinement (ART-006) refuses
//! symlinks anywhere under the root, and the macOS llama.cpp release
//! archives ship their dylibs behind versioned symlink chains. Zip
//! symlinks remain unsupported.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

use super::Result;
use super::assets::ArchiveKind;
use super::confine::{ensure_cache_directory, safe_relative_path, validate_tree_path};
use crate::local::error::LocalError;

/// Extracts `archive` into `destination`, dispatching on the archive kind.
///
/// # Errors
/// Returns [`LocalError`] on unsafe entries or extraction failures.
pub(super) fn extract_archive(archive: &Path, destination: &Path, kind: ArchiveKind) -> Result<()> {
    match kind {
        ArchiveKind::TarGz => extract_tar_gz(archive, destination),
        ArchiveKind::Zip => extract_zip(archive, destination),
    }
}

fn extract_tar_gz(archive: &Path, destination: &Path) -> Result<()> {
    let file = File::open(archive).map_err(|source| LocalError::Io {
        operation: "open archive",
        path: archive.to_owned(),
        source,
    })?;
    let mut tar = tar::Archive::new(GzDecoder::new(BufReader::new(file)));
    let entries = tar.entries().map_err(|source| LocalError::Archive {
        archive: archive.display().to_string(),
        source,
    })?;
    let mut links: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(|source| LocalError::Archive {
            archive: archive.display().to_string(),
            source,
        })?;
        let entry_path = entry
            .path()
            .map_err(|source| LocalError::Archive {
                archive: archive.display().to_string(),
                source,
            })?
            .into_owned();
        if !safe_archive_path(&entry_path) {
            return Err(LocalError::UnsafeArchiveEntry {
                archive: archive.display().to_string(),
                entry: entry_path.display().to_string(),
            });
        }
        if entry.header().entry_type().is_symlink() {
            let target = entry.link_name().map_err(|source| LocalError::Archive {
                archive: archive.display().to_string(),
                source,
            })?;
            let resolved = target
                .as_deref()
                .and_then(|target| resolve_link_target(&entry_path, target));
            let Some(resolved) = resolved else {
                return Err(LocalError::UnsafeArchiveEntry {
                    archive: archive.display().to_string(),
                    entry: entry_path.display().to_string(),
                });
            };
            links.push((entry_path, resolved));
            continue;
        }
        if !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir()) {
            return Err(LocalError::UnsafeArchiveEntry {
                archive: archive.display().to_string(),
                entry: entry_path.display().to_string(),
            });
        }
        let output = destination.join(&entry_path);
        validate_tree_path(destination, &output)?;
        if let Some(parent) = output.parent() {
            ensure_cache_directory(destination, parent)?;
        }
        let unpacked = entry
            .unpack_in(destination)
            .map_err(|source| LocalError::Archive {
                archive: archive.display().to_string(),
                source,
            })?;
        if !unpacked {
            return Err(LocalError::UnsafeArchiveEntry {
                archive: archive.display().to_string(),
                entry: entry_path.display().to_string(),
            });
        }
        validate_tree_path(destination, &output)?;
    }
    materialize_symlinks(archive, destination, &links)
}

/// Longest symlink chain the extractor follows before calling it a cycle.
const MAX_LINK_HOPS: usize = 32;

/// Lexically resolves a symlink entry's target against the entry's parent
/// directory, returning `None` unless both the target and the resolved
/// path are safe relative paths that stay inside the archive root.
fn resolve_link_target(entry_path: &Path, target: &Path) -> Option<PathBuf> {
    if !safe_archive_path(target) {
        return None;
    }
    let parent = entry_path.parent().unwrap_or(Path::new(""));
    let resolved = parent.join(target);
    safe_archive_path(&resolved).then_some(resolved)
}

/// Copies each recorded symlink entry's final target to the link's own
/// path, following chains through other recorded links. Runs after every
/// regular entry has landed, so link-before-target archive order works.
///
/// # Errors
/// Returns [`LocalError::UnsafeArchiveEntry`] for a dangling target, a
/// chain past [`MAX_LINK_HOPS`] (a cycle), or a target that is not a
/// regular file; copy failures surface as [`LocalError::Io`].
fn materialize_symlinks(
    archive: &Path,
    destination: &Path,
    links: &[(PathBuf, PathBuf)],
) -> Result<()> {
    let targets: HashMap<&Path, &Path> = links
        .iter()
        .map(|(link, target)| (link.as_path(), target.as_path()))
        .collect();
    for (link, target) in links {
        let unsafe_entry = || LocalError::UnsafeArchiveEntry {
            archive: archive.display().to_string(),
            entry: link.display().to_string(),
        };
        let mut resolved = target.as_path();
        let mut hops = 0;
        while let Some(next) = targets.get(resolved) {
            hops += 1;
            if hops > MAX_LINK_HOPS {
                return Err(unsafe_entry());
            }
            resolved = next;
        }
        let target_path = destination.join(resolved);
        validate_tree_path(destination, &target_path)?;
        let is_regular_file =
            fs::symlink_metadata(&target_path).is_ok_and(|metadata| metadata.is_file());
        if !is_regular_file {
            return Err(unsafe_entry());
        }
        let output = destination.join(link);
        validate_tree_path(destination, &output)?;
        if let Some(parent) = output.parent() {
            ensure_cache_directory(destination, parent)?;
        }
        fs::copy(&target_path, &output).map_err(|source| LocalError::Io {
            operation: "materialize archive symlink",
            path: output.clone(),
            source,
        })?;
    }
    Ok(())
}

fn extract_zip(archive: &Path, destination: &Path) -> Result<()> {
    let file = File::open(archive).map_err(|source| LocalError::Io {
        operation: "open archive",
        path: archive.to_owned(),
        source,
    })?;
    let mut zip =
        zip::ZipArchive::new(BufReader::new(file)).map_err(|source| LocalError::Archive {
            archive: archive.display().to_string(),
            source: io::Error::other(source),
        })?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|source| LocalError::Archive {
            archive: archive.display().to_string(),
            source: io::Error::other(source),
        })?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(LocalError::UnsafeArchiveEntry {
                archive: archive.display().to_string(),
                entry: entry.name().to_owned(),
            });
        };
        if !safe_archive_name(entry.name()) || !safe_relative_path(&relative) {
            return Err(LocalError::UnsafeArchiveEntry {
                archive: archive.display().to_string(),
                entry: entry.name().to_owned(),
            });
        }
        let mode = entry.unix_mode();
        if !zip_entry_type_is_supported(mode, entry.is_dir()) {
            return Err(LocalError::UnsafeArchiveEntry {
                archive: archive.display().to_string(),
                entry: entry.name().to_owned(),
            });
        }
        let output = destination.join(relative);
        validate_tree_path(destination, &output)?;
        if entry.is_dir() {
            ensure_cache_directory(destination, &output)?;
            continue;
        }
        let Some(parent) = output.parent() else {
            return Err(LocalError::InvalidPath { path: output });
        };
        ensure_cache_directory(destination, parent)?;
        validate_tree_path(destination, &output)?;
        let mut file = File::create(&output).map_err(|source| LocalError::Io {
            operation: "create extracted file",
            path: output.clone(),
            source,
        })?;
        io::copy(&mut entry, &mut file).map_err(|source| LocalError::Io {
            operation: "write extracted file",
            path: output.clone(),
            source,
        })?;
        drop(file);
        apply_archive_mode(&output, mode)?;
    }
    Ok(())
}

pub(super) fn safe_archive_path(path: &Path) -> bool {
    safe_relative_path(path) && path.to_str().is_some_and(safe_archive_name)
}

fn safe_archive_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with(['/', '\\']) || name.contains('\0') {
        return false;
    }
    let mut saw_component = false;
    for component in name.split(['/', '\\']) {
        if component.is_empty() {
            continue;
        }
        if component == "." || component == ".." || component.contains(':') {
            return false;
        }
        saw_component = true;
    }
    saw_component
}

fn zip_entry_type_is_supported(mode: Option<u32>, is_directory: bool) -> bool {
    let Some(mode) = mode else {
        return true;
    };
    let kind = mode & 0o170_000;
    if is_directory {
        kind == 0 || kind == 0o040_000
    } else {
        kind == 0 || kind == 0o100_000
    }
}

#[cfg(unix)]
fn apply_archive_mode(path: &Path, mode: Option<u32>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let Some(mode) = mode else {
        return Ok(());
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777)).map_err(|source| {
        LocalError::Io {
            operation: "set extracted file permissions",
            path: path.to_owned(),
            source,
        }
    })
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the fallible Unix implementation at the call site"
)]
fn apply_archive_mode(_path: &Path, _mode: Option<u32>) -> Result<()> {
    Ok(())
}

/// Verifies the staged `path` carries an executable bit (tar.gz installs).
///
/// # Errors
/// Returns [`LocalError`] when the file lacks an executable bit or cannot be read.
#[cfg(unix)]
pub(super) fn require_executable(path: &Path, archive: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = fs::metadata(path)
        .map_err(|source| LocalError::Io {
            operation: "inspect executable permissions",
            path: path.to_owned(),
            source,
        })?
        .permissions()
        .mode();
    if mode & 0o111 == 0 {
        return Err(LocalError::UnsafeArchiveEntry {
            archive: archive.to_owned(),
            entry: path.display().to_string(),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the fallible Unix implementation at the call site"
)]
pub(super) fn require_executable(_path: &Path, _archive: &str) -> Result<()> {
    Ok(())
}

/// Locates the single file named `name` under `root` (recursively).
///
/// # Errors
/// Returns [`LocalError`] when no match or more than one match is found, or the
/// tree cannot be walked.
pub(super) fn find_executable(root: &Path, name: &str, archive: &str) -> Result<PathBuf> {
    let mut matches = Vec::new();
    collect_named_files(root, OsStr::new(name), &mut matches)?;
    matches.sort();
    match matches.as_slice() {
        [] => Err(LocalError::MissingExecutable {
            archive: archive.to_owned(),
            executable: name.to_owned(),
        }),
        [path] => Ok(path.clone()),
        _ => Err(LocalError::DuplicateExecutable {
            archive: archive.to_owned(),
            executable: name.to_owned(),
        }),
    }
}

fn collect_named_files(root: &Path, name: &OsStr, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(root).map_err(|source| LocalError::Io {
        operation: "read installation directory",
        path: root.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| LocalError::Io {
            operation: "read installation entry",
            path: root.to_owned(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| LocalError::Io {
            operation: "inspect installation entry",
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_named_files(&path, name, files)?;
        } else if file_type.is_file() && entry.file_name() == name {
            files.push(path);
        }
    }
    Ok(())
}
