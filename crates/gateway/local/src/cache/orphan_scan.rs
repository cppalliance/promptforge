//! The orphan scan: files under the cache's `models/` tree that no configured model references.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gateway_config::LocalModelConfig;
use serde::Serialize;

use super::{META_SUFFIX, read_meta};
use crate::artifacts::{expand_tilde, filename_from_url, looks_like_url, source_cache_key};
use crate::error::LocalError;

/// A file under the cache's `models/` tree that no loaded `[[local_model]]`
/// entry references.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OrphanEntry {
    /// Path relative to the cache root, `/`-separated on every platform.
    pub path: String,
    /// File length in bytes.
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 recorded by the blob's cache sidecar, when one
    /// exists. `None` for files the cache API never downloaded: blobs are
    /// multi-gigabyte, so their bytes are never re-hashed to fill the field
    /// (Amendment C).
    pub sha256: Option<String>,
}

/// Store bookkeeping suffixes that are never orphans: cache sidecars,
/// model-card sidecars, verified markers, staging files, and the staging
/// files' resume provenance markers.
const BOOKKEEPING_SUFFIXES: [&str; 5] = [META_SUFFIX, ".md", ".verified", ".part", ".part.source"];

/// The absolute path a `[[local_model]]` source occupies on disk: the
/// provisioning cache slot (`models/<key>/<filename>`) for a URL source, the
/// tilde-expanded path itself for a path source. `None` when the source
/// cannot resolve (no home directory, no URL filename segment); such a
/// source cannot name an on-disk file.
fn configured_path(root: &Path, source: &str) -> Option<PathBuf> {
    if looks_like_url(source) {
        let name = filename_from_url(source).ok()?;
        let key = source_cache_key(source);
        Some(root.join("models").join(key).join(name))
    } else {
        expand_tilde(source).ok()
    }
}

/// Lists files under `<root>/models/` that no entry of `models` references.
///
/// A model references its `source` plus the sources of its speculative and
/// multimodal-projector companions; each resolves to the same on-disk path
/// provisioning uses (a URL source to its cache slot, a path source to the
/// tilde-expanded path). Comparison falls back to canonicalized paths, so a
/// path source spelled with different case or separators still matches its
/// file. Store bookkeeping (`.meta.json` cache sidecars, `.md` model-card
/// sidecars, `.verified` markers, `.part` staging files) is never reported,
/// and symlinked entries are skipped as in [`BlobCache::list`](super::BlobCache::list). A missing `models/` directory
/// yields an empty list. Entries sort by path for a stable response.
///
/// # Errors
/// Returns [`LocalError::Io`] when the tree cannot be walked or a file
/// cannot be inspected.
pub fn orphans(
    root: &Path,
    models: &[LocalModelConfig],
    extra_sources: &[&str],
) -> Result<Vec<OrphanEntry>, LocalError> {
    let mut configured = HashSet::new();
    for model in models {
        let mut sources = vec![model.source()];
        if let Some(speculative) = model.speculative() {
            sources.push(speculative.source());
        }
        if let Some(projector) = model.multimodal_projector() {
            sources.push(projector.source());
        }
        for source in sources {
            let Some(path) = configured_path(root, source) else {
                continue;
            };
            if let Ok(canonical) = fs::canonicalize(&path) {
                configured.insert(canonical);
            }
            configured.insert(path);
        }
    }
    for source in extra_sources {
        let Some(path) = configured_path(root, source) else {
            continue;
        };
        if let Ok(canonical) = fs::canonicalize(&path) {
            configured.insert(canonical);
        }
        configured.insert(path);
    }

    let models_dir = root.join("models");
    let top = match fs::read_dir(&models_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(LocalError::Io {
                operation: "read cache models directory",
                path: models_dir,
                source,
            });
        }
    };
    let mut found = Vec::new();
    let mut directories = Vec::new();
    collect_orphans(
        root,
        &models_dir,
        top,
        &configured,
        &mut directories,
        &mut found,
    )?;
    while let Some(directory) = directories.pop() {
        let entries = fs::read_dir(&directory).map_err(|source| LocalError::Io {
            operation: "read cache models directory",
            path: directory.clone(),
            source,
        })?;
        collect_orphans(
            root,
            &directory,
            entries,
            &configured,
            &mut directories,
            &mut found,
        )?;
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(found)
}

/// Feeds one directory's entries into the orphan scan: unreferenced files
/// become entries of `found`, subdirectories queue on `directories` for a
/// later pass, and symlinks are skipped.
fn collect_orphans(
    root: &Path,
    directory: &Path,
    entries: fs::ReadDir,
    configured: &HashSet<PathBuf>,
    directories: &mut Vec<PathBuf>,
    found: &mut Vec<OrphanEntry>,
) -> Result<(), LocalError> {
    for entry in entries {
        let entry = entry.map_err(|source| LocalError::Io {
            operation: "read cache models entry",
            path: directory.to_owned(),
            source,
        })?;
        // `file_type` does not follow links, so a planted symlinked directory
        // or blob is skipped rather than read through.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            directories.push(path);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if BOOKKEEPING_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
        {
            continue;
        }
        if configured.contains(&path)
            || fs::canonicalize(&path).is_ok_and(|canonical| configured.contains(&canonical))
        {
            continue;
        }
        let size_bytes = entry
            .metadata()
            .map_err(|source| LocalError::Io {
                operation: "stat cache models entry",
                path: path.clone(),
                source,
            })?
            .len();
        let sha256 = read_meta(&path)?.map(|meta| meta.sha256);
        let relative = path.strip_prefix(root).unwrap_or(&path);
        found.push(OrphanEntry {
            path: relative.to_string_lossy().replace('\\', "/"),
            size_bytes,
            sha256,
        });
    }
    Ok(())
}
