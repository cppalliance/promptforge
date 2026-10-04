//! Blob fetch and verification into the artifact cache.

use std::path::Path;

use gateway_progress::Activity;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
use super::DownloadProgress;
use super::assets::FileAsset;
use super::digest::file_digest_with_progress;
use super::download;
use super::verified::{blob_marker_path, verify_blob_with_progress, write_marker_best_effort};
use super::{
    ArtifactStore, Result, ensure_cache_directory, parse_expected_digest, part_path,
    remove_cache_entry, rename_confined, validate_cache_path,
};
use crate::error::LocalError;

impl ArtifactStore {
    /// `ensure_blob` variant that writes the download and verify stages into
    /// `activity`'s text: a cache hit reports at most the verify hash pass,
    /// and the pin check after a download costs no second pass because the
    /// digest is computed inline during the transfer.
    pub(super) fn ensure_blob_with_progress(
        &self,
        asset: FileAsset<'_>,
        destination: &Path,
        activity: Option<&Activity>,
        token: Option<&CancellationToken>,
    ) -> Result<()> {
        let _lock = self.lock_artifact(destination)?;
        validate_cache_path(&self.cache, destination)?;
        // No pre-download cleanup: a staged `.part` with a provenance
        // marker naming this source resumes where it stopped; any other
        // partial is truncated by the fresh transfer.
        let staging = part_path(destination);

        // Validate/canonicalize the pin once, at the boundary, so both the
        // cache-hit and post-download comparisons are case-insensitive and a
        // malformed pin fails fast rather than always mismatching.
        let expected_digest = asset.sha256.map(parse_expected_digest).transpose()?;

        if destination.is_file() {
            let Some(expected) = expected_digest.as_deref() else {
                if !crate::cache::blob_meta_matches(destination, asset.url)? {
                    let actual = file_digest_with_progress(destination, activity)?;
                    crate::cache::write_blob_meta(&self.cache, destination, asset.url, &actual)?;
                }
                return Ok(());
            };
            let marker = blob_marker_path(destination);
            match verify_blob_with_progress(&self.cache, destination, expected, &marker, activity) {
                Ok(_) => {
                    // A verified cache hit has no download to run.
                    crate::cache::write_blob_meta(&self.cache, destination, asset.url, expected)?;
                    return Ok(());
                }
                // A pin mismatch on a cached blob is repaired by
                // re-downloading; every other failure propagates.
                Err(LocalError::DigestMismatch { .. }) => {
                    tracing::warn!(
                        name = asset.name,
                        "cached artifact no longer matches its pin; downloading it again"
                    );
                    remove_cache_entry(&self.cache, destination)?;
                }
                Err(error) => return Err(error),
            }
        } else if destination.exists() {
            remove_cache_entry(&self.cache, destination)?;
        }

        let Some(parent) = destination.parent() else {
            return Err(LocalError::InvalidPath {
                path: destination.to_owned(),
            });
        };
        ensure_cache_directory(&self.cache, parent)?;
        validate_cache_path(&self.cache, &staging)?;
        // A failed transfer keeps the staged partial for resume.
        let actual = download::download(
            &self.client,
            asset.url,
            &staging,
            asset.name,
            activity,
            token,
        )?;
        // The pin is checked against the digest computed inline during the
        // download, so no separate verify pass runs here.
        if let Some(expected) = expected_digest.as_deref()
            && actual != expected
        {
            return Err(LocalError::DigestMismatch {
                name: asset.name.to_owned(),
                expected: expected.to_owned(),
                actual,
            });
        }
        rename_confined(&self.cache, &staging, destination)?;
        if let Some(expected) = expected_digest.as_deref() {
            let marker = blob_marker_path(destination);
            // Confinement stays a hard error; only the marker write degrades.
            validate_cache_path(&self.cache, &marker)?;
            write_marker_best_effort(&marker, destination, expected);
        }
        crate::cache::write_blob_meta(
            &self.cache,
            destination,
            asset.url,
            expected_digest.as_deref().unwrap_or(&actual),
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn download_with_progress(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn DownloadProgress,
    ) -> Result<String> {
        download::download_with_progress(&self.client, url, destination, progress, None)
    }
}
