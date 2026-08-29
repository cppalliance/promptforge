//! Verified-digest markers: a cache-side record that a blob already matched
//! its SHA-256 pin, so a profile switch does not re-hash multi-gigabyte
//! weights on every cache hit.
//!
//! # Trust tradeoff
//!
//! A marker hit trusts file size plus mtime (seconds and nanoseconds since
//! the Unix epoch) as a fingerprint of the verified content. That fingerprint
//! is spoofable by anyone who can write the cache: mtime is not a
//! cryptographic bound. The cache root is already operator-trusted (made
//! owner-private by [`super::confine::enforce_private_cache_root`], ART-006),
//! so this adds no new exposure. The pin still fully guards the download
//! path: a marker is written only after a real hash match, so a forged or
//! stale marker can never bless content that was not once verified.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::confine::{ensure_cache_directory, validate_cache_path, write_synced};
use super::digest::file_digest;
use super::{Result, source_cache_key};
use crate::error::LocalError;

/// How a blob's pin was confirmed: marker cache hit, or a fresh hash pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub(super) enum VerifyOutcome {
    /// The marker's recorded digest, size, and mtime all matched; no read of
    /// the blob itself was needed.
    MarkerHit,
    /// The marker was missing, stale, or corrupt, so the blob was hashed and
    /// the marker written or refreshed.
    Hashed,
}

/// Verifies that `path` matches the canonical lowercase hex pin `expected`,
/// consulting `marker` before falling back to a full hash of the blob.
///
/// A marker hit requires the recorded digest to equal `expected` and the
/// blob's size and mtime to match the record exactly; anything else (missing,
/// stale, truncated, or unparseable marker) is a cache miss, never an error,
/// and falls through to [`file_digest`]. On a hash match the marker is
/// written or refreshed best-effort via [`write_marker_best_effort`]: the
/// marker only skips a future re-hash, so a persistence failure is logged and
/// never fails the verification. On mismatch the stale marker is
/// deleted and the mismatch returned. See the module docs for the accepted
/// trust tradeoff.
///
/// # Errors
/// Returns [`LocalError::UnsafeCachePath`] when `marker` (or `path`, when it
/// lies under `cache_root`) escapes the cache root, [`LocalError::Io`] when
/// reading the marker or hashing or inspecting the blob fails, and
/// [`LocalError::DigestMismatch`] when the blob's actual digest does not
/// match `expected`.
pub(super) fn verify_blob(
    cache_root: &Path,
    path: &Path,
    expected: &str,
    marker: &Path,
) -> Result<VerifyOutcome> {
    validate_cache_path(cache_root, marker)?;
    // A path source lives outside the cache by design; only confine the blob
    // when it is a cache resident.
    if path.starts_with(cache_root) {
        validate_cache_path(cache_root, path)?;
    }
    if marker_matches(marker, path, expected)? {
        return Ok(VerifyOutcome::MarkerHit);
    }
    let actual = file_digest(path)?;
    if actual != expected {
        let _ignored = fs::remove_file(marker);
        return Err(LocalError::DigestMismatch {
            name: path.display().to_string(),
            expected: expected.to_owned(),
            actual,
        });
    }
    write_marker_best_effort(marker, path, expected);
    Ok(VerifyOutcome::Hashed)
}

/// The marker path for a URL-source blob: `<name>.verified` beside the blob,
/// covered by the `lock_artifact` guard the caller already holds.
pub(super) fn blob_marker_path(blob: &Path) -> PathBuf {
    let mut name = blob.as_os_str().to_owned();
    name.push(".verified");
    PathBuf::from(name)
}

/// The marker path for a path source (a file outside the cache):
/// `<cache>/markers/<source_cache_key(path)>.verified`, creating the
/// `markers` directory.
///
/// # Errors
/// Returns [`LocalError`] when the `markers` directory cannot be created or
/// the marker path fails confinement.
pub(super) fn path_source_marker(cache_root: &Path, source: &Path) -> Result<PathBuf> {
    let markers = cache_root.join("markers");
    ensure_cache_directory(cache_root, &markers)?;
    let marker = markers.join(format!(
        "{}.verified",
        source_cache_key(&source.to_string_lossy())
    ));
    validate_cache_path(cache_root, &marker)?;
    Ok(marker)
}

/// Writes or refreshes the marker for a blob whose digest just matched.
///
/// A blob with a pre-epoch mtime cannot be recorded; the marker is simply
/// left absent, which costs a re-hash on the next verification and nothing
/// more.
///
/// # Errors
/// Returns [`LocalError::Io`] when inspecting the blob or writing the marker
/// fails.
pub(super) fn write_marker(marker: &Path, path: &Path, digest: &str) -> Result<()> {
    let metadata = fs::metadata(path).map_err(|source| LocalError::Io {
        operation: "inspect verified artifact",
        path: path.to_owned(),
        source,
    })?;
    let Some((secs, nanos)) = mtime_stamp(&metadata) else {
        return Ok(());
    };
    write_synced(
        marker,
        format!("{digest}\n{}\n{secs}.{nanos}\n", metadata.len()).as_bytes(),
    )
}

/// Writes or refreshes the marker, degrading a persistence failure to a
/// warn-level log: the marker only skips a future re-hash, so losing it must
/// never fail an operation whose digest already matched.
pub(super) fn write_marker_best_effort(marker: &Path, path: &Path, digest: &str) {
    if let Err(error) = write_marker(marker, path, digest) {
        tracing::warn!(
            marker = %marker.display(),
            error = %error,
            "verified-digest marker not persisted; the blob will be re-hashed next time"
        );
    }
}

/// Whether the marker records `expected` plus the blob's current size and
/// mtime. Any parse failure or absent marker is a miss, never an error.
fn marker_matches(marker: &Path, path: &Path, expected: &str) -> Result<bool> {
    let text = match fs::read_to_string(marker) {
        Ok(text) => text,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(LocalError::Io {
                operation: "read verified marker",
                path: marker.to_owned(),
                source,
            });
        }
    };
    let mut lines = text.lines();
    let (Some(digest), Some(size), Some(mtime)) = (lines.next(), lines.next(), lines.next()) else {
        return Ok(false);
    };
    if lines.next().is_some() || digest != expected {
        return Ok(false);
    }
    let Ok(size) = size.parse::<u64>() else {
        return Ok(false);
    };
    let Some(stamp) = parse_mtime(mtime) else {
        return Ok(false);
    };
    let metadata = fs::metadata(path).map_err(|source| LocalError::Io {
        operation: "inspect cached artifact",
        path: path.to_owned(),
        source,
    })?;
    if metadata.len() != size {
        return Ok(false);
    }
    Ok(mtime_stamp(&metadata) == Some(stamp))
}

/// The `(secs, nanos)` mtime pair from `UNIX_EPOCH`, or `None` when the mtime
/// is unreadable or predates the epoch.
fn mtime_stamp(metadata: &fs::Metadata) -> Option<(u64, u32)> {
    let duration = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((duration.as_secs(), duration.subsec_nanos()))
}

fn parse_mtime(text: &str) -> Option<(u64, u32)> {
    let (secs, nanos) = text.split_once('.')?;
    Some((secs.parse().ok()?, nanos.parse().ok()?))
}
