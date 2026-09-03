//! SHA-256 digest parsing, file hashing, and deterministic tree digests.

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use promptforge_progress::ProgressHandle;

use super::{INSTALL_MARKER, Result};
use crate::error::LocalError;

/// Validates and canonicalizes a configured SHA-256 pin at the trust boundary.
///
/// A pin must be exactly 64 hexadecimal characters. The returned value is
/// lowercased so comparison against the lowercase hex produced by `hex_digest`
/// never fails on case alone (a real footgun with an uppercase config value).
///
/// # Errors
/// Returns [`LocalError::InvalidDigest`] when the pin is not 64 hex characters.
pub fn parse_expected_digest(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.len() != 64 {
        return Err(LocalError::InvalidDigest {
            value: raw.to_owned(),
            reason: "expected exactly 64 hexadecimal characters",
        });
    }
    if !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(LocalError::InvalidDigest {
            value: raw.to_owned(),
            reason: "expected only hexadecimal characters",
        });
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// Lowercase hex encoding of a finalized SHA-256 hasher.
pub(crate) fn hex_digest(hasher: Sha256) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = hasher.finalize();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

/// Streams `path` through SHA-256, returning the lowercase hex digest.
///
/// # Errors
/// Returns [`LocalError::Io`] when the file cannot be opened or read.
pub(super) fn file_digest(path: &Path) -> Result<String> {
    file_digest_with_progress(path, None)
}

/// [`file_digest`] variant that reports bytes read into `progress`, when given.
///
/// # Errors
/// Returns [`LocalError::Io`] when the file cannot be opened, inspected, or read.
pub(super) fn file_digest_with_progress(
    path: &Path,
    progress: Option<&ProgressHandle>,
) -> Result<String> {
    let file = File::open(path).map_err(|source| LocalError::Io {
        operation: "open cached artifact",
        path: path.to_owned(),
        source,
    })?;
    let total = progress
        .map(|_| {
            file.metadata()
                .map(|metadata| metadata.len())
                .map_err(|source| LocalError::Io {
                    operation: "inspect cached artifact",
                    path: path.to_owned(),
                    source,
                })
        })
        .transpose()?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    let mut read: u64 = 0;
    loop {
        let count = reader.read(&mut buffer).map_err(|source| LocalError::Io {
            operation: "hash cached artifact",
            path: path.to_owned(),
            source,
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        read = read.saturating_add(count as u64);
        if let (Some(handle), Some(total)) = (progress, total) {
            handle.set_units(read, total);
        }
    }
    Ok(hex_digest(hasher))
}

/// A deterministic digest over the installed file tree under `root`.
///
/// The digest is stable across runs: entries are sorted by relative path and
/// each contributes its path, mode, and content digest, so an install marker
/// can later detect any drift in the extracted tree.
///
/// # Errors
/// Returns [`LocalError`] when the tree cannot be walked or a file cannot be read.
pub(super) fn tree_digest(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_tree_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut tree_hasher = Sha256::new();
    for (relative, path) in files {
        tree_hasher.update((relative.len() as u64).to_le_bytes());
        tree_hasher.update(relative.as_bytes());
        tree_hasher.update(file_mode(&path)?.to_le_bytes());
        tree_hasher.update(file_digest(&path)?.as_bytes());
    }
    Ok(hex_digest(tree_hasher))
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o7777)
        .map_err(|source| LocalError::Io {
            operation: "inspect cached artifact permissions",
            path: path.to_owned(),
            source,
        })
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the fallible Unix implementation at the call site"
)]
fn file_mode(_path: &Path) -> Result<u32> {
    Ok(0)
}

fn collect_tree_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|source| LocalError::Io {
        operation: "read installation directory",
        path: directory.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| LocalError::Io {
            operation: "read installation entry",
            path: directory.to_owned(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| LocalError::Io {
            operation: "inspect installation entry",
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_tree_files(root, &path, files)?;
        } else if file_type.is_file() && path != root.join(INSTALL_MARKER) {
            let relative = path
                .strip_prefix(root)
                .map_err(|source| LocalError::Io {
                    operation: "resolve installation entry",
                    path: path.clone(),
                    source: io::Error::other(source),
                })?
                .to_str()
                .ok_or_else(|| LocalError::InvalidPath { path: path.clone() })?
                .replace('\\', "/");
            files.push((relative, path));
        } else if !file_type.is_file() {
            return Err(LocalError::UnsafeArchiveEntry {
                archive: root.display().to_string(),
                entry: path.display().to_string(),
            });
        }
    }
    Ok(())
}
