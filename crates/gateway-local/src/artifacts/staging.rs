//! Verified publication of embedded runtime assets.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest as _, Sha256};

use super::confine::{
    ensure_cache_directory, part_path, remove_cache_entry, rename_confined, validate_cache_path,
    write_synced,
};
use super::digest::{file_digest_with_progress, hex_digest};
use super::verified::{blob_marker_path, verify_blob_with_progress, write_marker_best_effort};
use super::{ArtifactStore, Result};
use crate::error::LocalError;

static BACKUP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl ArtifactStore {
    /// Stages embedded bytes at a cache-relative path and records a verified marker.
    ///
    /// Existing bytes are reused only when the marker fingerprint or a fresh
    /// SHA-256 pass confirms the embedded digest. A replacement is fully
    /// written and verified before publication. Platforms with overwrite
    /// rename publish atomically; the Windows fallback preserves and restores
    /// the old file if publication fails.
    ///
    /// The verified marker is a cache optimization. A marker persistence
    /// failure does not invalidate bytes that were already hash-verified.
    ///
    /// # Errors
    /// Returns a [`LocalError`] when the relative path escapes the cache or
    /// staging, verification, or publication fails.
    pub(crate) fn stage_verified_asset(&self, relative: &Path, contents: &[u8]) -> Result<PathBuf> {
        let destination = self.cache_path(relative.to_owned())?;
        let _lock = self.lock_artifact(&destination)?;
        validate_cache_path(&self.cache, &destination)?;

        let mut hasher = Sha256::new();
        hasher.update(contents);
        let expected = hex_digest(hasher);
        let marker = blob_marker_path(&destination);
        if destination.is_file() {
            match verify_blob_with_progress(&self.cache, &destination, &expected, &marker, None) {
                Ok(_) => return Ok(destination),
                Err(LocalError::DigestMismatch { .. }) => {}
                Err(error) => return Err(error),
            }
        }

        let parent = destination
            .parent()
            .ok_or_else(|| LocalError::InvalidPath {
                path: destination.clone(),
            })?;
        ensure_cache_directory(&self.cache, parent)?;
        let staging = part_path(&destination);
        remove_cache_entry(&self.cache, &staging)?;
        validate_cache_path(&self.cache, &staging)?;

        let result: Result<()> = (|| {
            write_synced(&staging, contents)?;
            let actual = file_digest_with_progress(&staging, None)?;
            if actual != expected {
                return Err(LocalError::DigestMismatch {
                    name: staging.display().to_string(),
                    expected: expected.clone(),
                    actual,
                });
            }
            replace_staged_file(&self.cache, &staging, &destination)?;
            write_marker_best_effort(&marker, &destination, &expected);
            Ok(())
        })();
        if result.is_err() {
            let _ignored = fs::remove_file(&staging);
        }
        result?;
        Ok(destination)
    }
}

fn replace_staged_file(cache: &Path, staging: &Path, destination: &Path) -> Result<()> {
    match rename_confined(cache, staging, destination) {
        Ok(()) => return Ok(()),
        Err(first_error) if !destination.exists() => return Err(first_error),
        Err(_first_error) if !destination.is_file() => {
            remove_cache_entry(cache, destination)?;
            return rename_confined(cache, staging, destination);
        }
        Err(_) => {}
    }

    let backup = unique_backup_path(destination);
    validate_cache_path(cache, &backup)?;
    remove_cache_entry(cache, &backup)?;
    rename_confined(cache, destination, &backup)?;
    if let Err(replace_error) = rename_confined(cache, staging, destination) {
        if let Err(restore_error) = rename_confined(cache, &backup, destination) {
            return Err(LocalError::Io {
                operation: "restore replaced embedded asset",
                path: backup,
                source: io::Error::other(format!(
                    "publication failed ({replace_error}); restore failed ({restore_error})"
                )),
            });
        }
        return Err(replace_error);
    }
    let _ignored = fs::remove_file(backup);
    Ok(())
}

fn unique_backup_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_owned();
    name.push(format!(
        ".backup.{}.{}",
        std::process::id(),
        BACKUP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests;
