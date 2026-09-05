//! Pinned native runtimes and GGUF cache for gateway-owned local inference.
//!
//! Downloads land under the operator cache (`~/.promptforge` by default). The
//! `llama-server` build is pinned to b10082,
//! preferring GPU-enabled archives (Vulkan on Windows/Linux, Metal on macOS).
//! Speech-to-text uses a separately pinned whisper.cpp shared-library bundle.
//!
//! The module is split into cohesive units: `assets` (release table),
//! `digest` (hashing + pin validation), `archive` (extraction),
//! `confine` (cache-root path safety), `progress` (download reporting),
//! `download` (HTTP transfer + scoped HF auth), and `verified`
//! (verified-digest markers). This file owns `ArtifactStore`, the
//! orchestration that ties them together.

mod archive;
mod assets;
mod confine;
mod digest;
mod download;
mod progress;
mod staging;
mod verified;

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use gateway_config::LlamaBackend;
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use shared_progress::ProgressHandle;
use tokio_util::sync::CancellationToken;

use crate::error::LocalError;

#[cfg(test)]
use archive::extract_archive;
use archive::extract_archive_with_progress;
use archive::find_executable;
use archive::require_executable;
use assets::ArchiveKind;
use assets::FileAsset;
use assets::{
    LLAMA_RELEASE, ServerAsset, WHISPER_RELEASE, WhisperAsset, server_asset, whisper_asset,
};
use confine::validate_tree_path;
use digest::{file_digest_with_progress, tree_digest};
use verified::{
    blob_marker_path, path_source_marker, verify_blob_with_progress, write_marker_best_effort,
};

// Re-exports consumed elsewhere in the crate (`runtime.rs`, `cache.rs`,
// `testsupport.rs`). Test-only helpers are imported directly from their
// submodules by `tests.rs`.
pub(crate) use confine::{
    enforce_private_cache_root, ensure_cache_directory, part_path, remove_cache_entry,
    rename_confined, safe_relative_path, validate_cache_path, write_synced,
};
// Test builds only: the resume tests in this module and cache.rs build the
// marker path; the download path itself imports it from confine directly.
#[cfg(test)]
pub(crate) use confine::source_marker_path;
pub(crate) use digest::hex_digest;
pub use digest::parse_expected_digest;
pub(crate) use download::{download_with_progress, hub_bearer_token_from_env};
pub use progress::{DownloadProgress, TreeProgress};

const INSTALL_MARKER: &str = ".promptforge-install";
/// Connect timeout for artifact downloads (bounds a stalled connect).
const DOWNLOAD_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Whole-request timeout for an artifact download (ART-003).
///
/// The pinned blocking reqwest client (0.12) exposes no per-read timeout,
/// so the read loop enforces the idle bound itself (see `download.rs`); this
/// generous ceiling stays as the final backstop: large enough for
/// multi-gigabyte GGUF weights on a slow link, but finite so a peer that
/// accepts the connection and then sends nothing can never pin the
/// provisioning thread forever - and a reader thread parked past the idle
/// bound reaps when the ceiling drops its body.
const DOWNLOAD_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

type Result<T> = std::result::Result<T, LocalError>;

/// A provisioned `llama-server`: the executable plus the directories its
/// child's `PATH` must be prefixed with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProvisionedServer {
    /// Absolute path of the `llama-server` executable.
    pub(crate) executable: PathBuf,
    /// Child `PATH` prefix. Empty: every managed install ships its runtime
    /// DLLs beside the executable.
    pub(crate) path_prefix: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
struct InstallAsset<'a> {
    family: &'a str,
    release: &'a str,
    platform: &'a str,
    archives: &'a [assets::ArchiveRef<'a>],
    required_name: &'a str,
    allow_cached_fallback: bool,
}

fn whisper_install_asset<'a>(
    asset: WhisperAsset<'a>,
    archives: &'a [assets::ArchiveRef<'a>],
) -> InstallAsset<'a> {
    InstallAsset {
        family: "whisper.cpp",
        release: WHISPER_RELEASE,
        platform: asset.platform,
        archives,
        required_name: asset.library_name,
        allow_cached_fallback: false,
    }
}

/// How the `llama-server` executable is chosen, from the `[local]` config
/// section.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ServerSelection<'a> {
    /// `llama_server_path`: an explicit executable path that wins over the
    /// `PROMPTFORGE_LLAMA_SERVER` environment variable and the managed
    /// download.
    pub(crate) server_path: Option<&'a str>,
    /// `llama_backend`: which build to download on Windows x86-64.
    pub(crate) backend: LlamaBackend,
}

/// Validates an operator-supplied `llama-server` path (the config key or
/// the environment variable) and returns it as the provisioned server. A
/// set-but-missing path is an operator error and fails loud rather than
/// falling through to the download.
fn external_server(value: &str, source: &str) -> Result<ProvisionedServer> {
    let path = expand_tilde(value)?;
    if !path.is_file() {
        return Err(LocalError::InvalidSource {
            value: path.display().to_string(),
            reason: format!("{source} does not name an existing file"),
        });
    }
    Ok(ProvisionedServer {
        executable: path,
        path_prefix: Vec::new(),
    })
}

/// Queries the host's NVIDIA compute capabilities through `nvidia-smi`.
/// Returns `None` when the driver or the tool is absent or fails; the
/// caller falls back to the Vulkan build.
fn nvidia_compute_caps() -> Option<Vec<(u64, u64)>> {
    let mut command = std::process::Command::new("nvidia-smi");
    command.args(["--query-gpu=compute_cap", "--format=csv,noheader"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(crate::CREATE_NO_WINDOW);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let caps: Vec<(u64, u64)> = stdout
        .lines()
        .filter_map(|line| {
            let (major, minor) = line.trim().split_once('.')?;
            Some((major.trim().parse().ok()?, minor.trim().parse().ok()?))
        })
        .collect();
    if caps.is_empty() { None } else { Some(caps) }
}

/// Cache root plus HTTP client for provisioning local inference artifacts.
#[derive(Debug)]
pub struct ArtifactStore {
    cache: PathBuf,
    client: Client,
}

impl ArtifactStore {
    /// Creates a store rooted at `cache`, creating the directory if needed.
    ///
    /// # Errors
    /// Returns [`LocalError::Io`] or [`LocalError::HttpClient`] on setup failure.
    pub fn new(cache: impl Into<PathBuf>) -> Result<Self> {
        let cache = cache.into();
        ensure_cache_directory(&cache, &cache)?;
        // Enforce the private-cache precondition the confinement design relies on
        // (owner-only root) before trusting the tree (ART-006).
        enforce_private_cache_root(&cache)?;
        Ok(Self {
            cache,
            client: download_client()?,
        })
    }

    /// Resolves the `llama-server` executable for this host: the configured
    /// `llama_server_path` first, then the `PROMPTFORGE_LLAMA_SERVER`
    /// environment variable, then the managed download of the pinned build
    /// for the selected backend, reporting the download, verify, and extract
    /// stages into child leaves of `progress`, when given.
    ///
    /// # Errors
    /// Returns a [`LocalError`] when an explicit path is invalid, the
    /// platform is unsupported, or provisioning fails.
    pub(crate) fn provision_llama_server_with_progress(
        &self,
        selection: &ServerSelection<'_>,
        progress: Option<&ProgressHandle>,
    ) -> Result<ProvisionedServer> {
        self.provision_llama_server_with_cancellation(selection, progress, None)
    }

    /// [`Self::provision_llama_server_with_progress`] variant that stops at
    /// download chunk boundaries and phase boundaries when `token` fires.
    pub(crate) fn provision_llama_server_with_cancellation(
        &self,
        selection: &ServerSelection<'_>,
        progress: Option<&ProgressHandle>,
        token: Option<&CancellationToken>,
    ) -> Result<ProvisionedServer> {
        if let Some(path) = selection.server_path {
            return external_server(path, "[local] llama_server_path");
        }
        if let Some(value) = std::env::var_os("PROMPTFORGE_LLAMA_SERVER") {
            return external_server(
                &value.to_string_lossy(),
                "the PROMPTFORGE_LLAMA_SERVER environment variable",
            );
        }
        // The GPU probe matters only for the Windows x86-64 `auto` pick;
        // every other platform and every explicit backend already knows its
        // row.
        let gpus = if std::env::consts::OS == "windows"
            && std::env::consts::ARCH == "x86_64"
            && selection.backend == LlamaBackend::Auto
        {
            nvidia_compute_caps()
        } else {
            None
        };
        let asset = server_asset(
            std::env::consts::OS,
            std::env::consts::ARCH,
            selection.backend,
            gpus.as_deref(),
        )?;
        let executable = self.provision_server(asset, progress, token)?;
        Ok(ProvisionedServer {
            executable,
            path_prefix: Vec::new(),
        })
    }

    /// Provisions the pinned whisper.cpp runtime for this host and returns
    /// the shared library path.
    ///
    /// The archive is downloaded, digest-verified, and extracted under the
    /// artifact cache. Its sibling ggml and GPU runtime libraries stay beside
    /// the returned file for the platform loader.
    ///
    /// # Errors
    /// Returns a [`LocalError`] when the platform is unsupported or download,
    /// verification, extraction, or cache publication fails.
    pub fn provision_whisper_library(&self, progress: Option<&ProgressHandle>) -> Result<PathBuf> {
        let asset = whisper_asset(std::env::consts::OS, std::env::consts::ARCH)?;
        let archives = [asset.archive];
        self.provision_install(whisper_install_asset(asset, &archives), progress, None)
    }

    /// Ensures a GGUF (or other blob) from `source` is available locally.
    ///
    /// `source` is either an `http(s)://` URL or a filesystem path (`~` expanded).
    /// When `sha256` is `Some`, the digest is verified after download and on cache hit.
    ///
    /// # Errors
    /// Returns a [`LocalError`] on download, verification, or path failures.
    pub fn ensure_model(&self, source: &str, sha256: Option<&str>) -> Result<PathBuf> {
        self.ensure_model_with_progress(source, sha256, None)
    }

    /// [`Self::ensure_model`] variant that reports the download and verify stages
    /// into child leaves of `progress`, when given. A path source completes
    /// the download leaf immediately. An unpinned URL hashes once when an
    /// older cache hit lacks listing metadata, then reuses that metadata.
    /// Both stages exist in the subtree whether or not they have work. An
    /// error exit fails any leaf that has not already finished.
    ///
    /// # Errors
    /// Returns a [`LocalError`] on download, verification, or path failures.
    pub fn ensure_model_with_progress(
        &self,
        source: &str,
        sha256: Option<&str>,
        progress: Option<&ProgressHandle>,
    ) -> Result<PathBuf> {
        self.ensure_model_with_cancellation(source, sha256, progress, None)
    }

    /// [`Self::ensure_model_with_progress`] variant that stops at download
    /// chunk boundaries when `token` fires, returning
    /// [`LocalError::Cancelled`]; the staged partial stays in place for a
    /// later resume.
    ///
    /// # Errors
    /// Returns a [`LocalError`] on download, verification, or path failures.
    pub fn ensure_model_with_cancellation(
        &self,
        source: &str,
        sha256: Option<&str>,
        progress: Option<&ProgressHandle>,
        token: Option<&CancellationToken>,
    ) -> Result<PathBuf> {
        let download = progress.map(|handle| handle.child("download", 4.0));
        let verify = progress.map(|handle| handle.child("verify", 1.0));
        let result =
            self.ensure_model_reporting(source, sha256, download.as_ref(), verify.as_ref(), token);
        // Terminal state is sticky, so leaves already finished inside (a
        // verified cache hit, a digest mismatch) are not failed twice.
        if result.is_err() {
            if let Some(handle) = &download {
                handle.fail();
            }
            if let Some(handle) = &verify {
                handle.fail();
            }
        }
        result
    }

    fn ensure_model_reporting(
        &self,
        source: &str,
        sha256: Option<&str>,
        download: Option<&ProgressHandle>,
        verify: Option<&ProgressHandle>,
        token: Option<&CancellationToken>,
    ) -> Result<PathBuf> {
        if looks_like_url(source) {
            let name = filename_from_url(source)?;
            // Key the cache slot by normalized source identity (ART-004) so two
            // distinct URLs that share a filename cannot collide on one path.
            let key = source_cache_key(source);
            let destination = self.cache_path(Path::new("models").join(&key).join(&name))?;
            let asset = FileAsset {
                name: &name,
                url: source,
                sha256,
            };
            self.ensure_blob_with_progress(asset, &destination, download, verify, token)?;
            return Ok(destination);
        }
        // A path source is already local: the download stage has no work.
        if let Some(handle) = download {
            handle.complete();
        }
        let path = expand_tilde(source)?;
        if !path.is_file() {
            return Err(LocalError::InvalidSource {
                value: source.to_owned(),
                reason: "path is not an existing file".to_owned(),
            });
        }
        match sha256 {
            Some(pin) => {
                let expected = parse_expected_digest(pin)?;
                let marker = path_source_marker(&self.cache, &path)?;
                let _outcome =
                    verify_blob_with_progress(&self.cache, &path, &expected, &marker, verify)?;
            }
            None => {
                if let Some(handle) = verify {
                    handle.complete();
                }
            }
        }
        Ok(path)
    }

    fn provision_server(
        &self,
        asset: ServerAsset<'_>,
        progress: Option<&ProgressHandle>,
        token: Option<&CancellationToken>,
    ) -> Result<PathBuf> {
        self.provision_install(
            InstallAsset {
                family: "llama.cpp",
                release: LLAMA_RELEASE,
                platform: asset.platform,
                archives: asset.archives,
                required_name: asset.executable_name,
                allow_cached_fallback: true,
            },
            progress,
            token,
        )
    }

    fn provision_install(
        &self,
        asset: InstallAsset<'_>,
        progress: Option<&ProgressHandle>,
        token: Option<&CancellationToken>,
    ) -> Result<PathBuf> {
        let download = progress.map(|handle| handle.child("download", 4.0));
        let verify = progress.map(|handle| handle.child("verify", 1.0));
        let extract = progress.map(|handle| handle.child("extract", 1.0));

        // Download and verify every archive the asset needs. When a download
        // fails and an older install is already in the cache, use the cached
        // one with a warning instead of failing to start.
        let mut downloaded = Vec::new();
        for archive_ref in asset.archives {
            // Phase boundary: a cancelled command stops before the next
            // archive rather than midway through the set.
            if token.is_some_and(CancellationToken::is_cancelled) {
                return Err(LocalError::Cancelled);
            }
            let archive = self.cache_path(Path::new("downloads").join(archive_ref.archive_name))?;
            let file_asset = FileAsset {
                name: archive_ref.archive_name,
                url: archive_ref.url,
                sha256: Some(archive_ref.sha256),
            };
            if let Err(error) = self.ensure_blob_with_progress(
                file_asset,
                &archive,
                download.as_ref(),
                verify.as_ref(),
                token,
            ) {
                if asset.allow_cached_fallback
                    && let Some(cached) =
                        self.cached_install_fallback(asset.family, asset.required_name)?
                {
                    tracing::warn!(
                        path = %cached.display(),
                        family = asset.family,
                        "runtime download failed ({error}); using the cached install"
                    );
                    return Ok(cached);
                }
                return Err(error);
            }
            downloaded.push(archive);
        }

        let install = self.cache_path(
            Path::new(asset.family).join(format!("{}-{}", asset.release, asset.platform)),
        )?;
        let _lock = self.lock_artifact(&install)?;
        validate_cache_path(&self.cache, &install)?;
        if Self::install_pins_are_valid(&install, asset.archives)? {
            // A valid install skips extraction entirely.
            if let Some(handle) = &extract {
                handle.complete();
            }
            return find_executable(&install, asset.required_name, asset.platform);
        }

        remove_cache_entry(&self.cache, &install)?;
        let staging = part_path(&install);
        remove_cache_entry(&self.cache, &staging)?;
        ensure_cache_directory(&self.cache, &staging)?;

        // Phase boundary: extraction starts only for an uncancelled command.
        if token.is_some_and(CancellationToken::is_cancelled) {
            return Err(LocalError::Cancelled);
        }
        // Every archive extracts into the same install folder (the generic
        // CUDA asset pairs the server zip with its runtime zip).
        for (archive, archive_ref) in downloaded.iter().zip(asset.archives.iter()) {
            validate_cache_path(&self.cache, archive)?;
            if let Err(error) = extract_archive_with_progress(
                archive,
                &staging,
                archive_ref.archive_kind,
                extract.as_ref(),
            ) {
                let _ignored = fs::remove_dir_all(&staging);
                return Err(error);
            }
        }

        let staged_executable = find_executable(&staging, asset.required_name, asset.platform)?;
        if asset
            .archives
            .iter()
            .any(|archive_ref| archive_ref.archive_kind == ArchiveKind::TarGz)
        {
            require_executable(&staged_executable, asset.platform)?;
        }
        let relative_executable =
            staged_executable
                .strip_prefix(&staging)
                .map_err(|source| LocalError::Io {
                    operation: "resolve staged executable",
                    path: staged_executable.clone(),
                    source: io::Error::other(source),
                })?;
        let tree_sha256 = tree_digest(&staging)?;
        let marker = staging.join(INSTALL_MARKER);
        validate_cache_path(&self.cache, &marker)?;
        // The marker records each archive's pin in table order, then the
        // tree digest.
        let mut marker_text = String::new();
        for archive_ref in asset.archives {
            marker_text.push_str(archive_ref.sha256);
            marker_text.push('\n');
        }
        marker_text.push_str(&tree_sha256);
        marker_text.push('\n');
        write_synced(&marker, marker_text.as_bytes())?;
        rename_confined(&self.cache, &staging, &install)?;
        Ok(install.join(relative_executable))
    }

    /// Finds a usable older runtime install in `family`: any install whose
    /// marker still verifies against its tree. Used when a version bump lands
    /// while the network is unavailable.
    fn cached_install_fallback(
        &self,
        family: &str,
        required_name: &str,
    ) -> Result<Option<PathBuf>> {
        let installs_dir = self.cache_path(Path::new(family).to_path_buf())?;
        let entries = match fs::read_dir(&installs_dir) {
            Ok(entries) => entries,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(LocalError::Io {
                    operation: "list runtime installs",
                    path: installs_dir,
                    source,
                });
            }
        };
        for entry in entries {
            let install = entry
                .map_err(|source| LocalError::Io {
                    operation: "read runtime install entry",
                    path: installs_dir.clone(),
                    source,
                })?
                .path();
            if !install.is_dir() || !Self::install_is_self_valid(&install)? {
                continue;
            }
            if let Ok(executable) = find_executable(&install, required_name, "cached install") {
                return Ok(Some(executable));
            }
        }
        Ok(None)
    }

    /// Marker self-validity for the fallback scan: the recorded tree digest
    /// (the marker's last line) still matches the install tree. The archive
    /// pins above it are provenance for a build that is no longer the pin.
    fn install_is_self_valid(install: &Path) -> Result<bool> {
        if !install.is_dir() {
            return Ok(false);
        }
        let marker = install.join(INSTALL_MARKER);
        validate_tree_path(install, &marker)?;
        let marker_text = match fs::read_to_string(&marker) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(LocalError::Io {
                    operation: "read install marker",
                    path: marker,
                    source,
                });
            }
        };
        let lines: Vec<&str> = marker_text.lines().collect();
        let Some(recorded_tree) = lines.last() else {
            return Ok(false);
        };
        if lines.len() < 2 {
            return Ok(false);
        }
        Ok(tree_digest(install)? == *recorded_tree)
    }

    #[cfg(test)]
    fn install_is_valid(install: &Path, asset: &ServerAsset<'_>) -> Result<bool> {
        Self::install_pins_are_valid(install, asset.archives)
    }

    fn install_pins_are_valid(install: &Path, archives: &[assets::ArchiveRef<'_>]) -> Result<bool> {
        if !install.is_dir() {
            return Ok(false);
        }
        let marker = install.join(INSTALL_MARKER);
        validate_tree_path(install, &marker)?;
        let marker_text = match fs::read_to_string(&marker) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(LocalError::Io {
                    operation: "read install marker",
                    path: marker,
                    source,
                });
            }
        };
        let lines: Vec<&str> = marker_text.lines().collect();
        if lines.len() != archives.len() + 1 {
            return Ok(false);
        }
        for (recorded, archive_ref) in lines.iter().zip(archives) {
            if *recorded != archive_ref.sha256 {
                return Ok(false);
            }
        }
        Ok(tree_digest(install)? == lines[archives.len()])
    }

    /// `ensure_blob` variant that reports the download and verify stages
    /// into the given leaves. Both leaves reach their terminal event on every
    /// exit path: a cache hit completes the download leaf without work, and
    /// the pin check after a download completes the verify leaf whether the
    /// digest matches or not.
    fn ensure_blob_with_progress(
        &self,
        asset: FileAsset<'_>,
        destination: &Path,
        download: Option<&ProgressHandle>,
        verify: Option<&ProgressHandle>,
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

        // Set once the verify leaf has emitted its terminal event, so the pin
        // recheck after a mismatch-repair download never emits it twice.
        let mut verify_finished = false;
        if destination.is_file() {
            let Some(expected) = expected_digest.as_deref() else {
                if let Some(handle) = download {
                    handle.complete();
                }
                if !crate::cache::blob_meta_matches(destination, asset.url)? {
                    let actual = file_digest_with_progress(destination, verify)?;
                    crate::cache::write_blob_meta(&self.cache, destination, asset.url, &actual)?;
                }
                if let Some(handle) = verify {
                    handle.complete();
                }
                return Ok(());
            };
            let marker = blob_marker_path(destination);
            match verify_blob_with_progress(&self.cache, destination, expected, &marker, verify) {
                Ok(_) => {
                    // A verified cache hit leaves the download stage
                    // with no work.
                    if let Some(handle) = download {
                        handle.complete();
                    }
                    crate::cache::write_blob_meta(&self.cache, destination, asset.url, expected)?;
                    return Ok(());
                }
                // A pin mismatch on a cached blob is repaired by
                // re-downloading; every other failure propagates.
                Err(LocalError::DigestMismatch { .. }) => {
                    verify_finished = true;
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
        let actual = match download::download(&self.client, asset.url, &staging, download, token) {
            Ok(actual) => actual,
            Err(error) => {
                if !verify_finished && let Some(handle) = verify {
                    handle.complete();
                }
                return Err(error);
            }
        };
        // The pin is checked against the digest computed inline during the
        // download, so the verify stage's work ends here on every outcome.
        if !verify_finished && let Some(handle) = verify {
            handle.complete();
        }
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
    fn download_with_progress(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn progress::DownloadProgress,
    ) -> Result<String> {
        download::download_with_progress(&self.client, url, destination, progress, None)
    }

    fn cache_path(&self, relative: PathBuf) -> Result<PathBuf> {
        if !safe_relative_path(&relative) {
            return Err(LocalError::UnsafeCachePath {
                path: self.cache.join(relative),
            });
        }
        let path = self.cache.join(relative);
        validate_cache_path(&self.cache, &path)?;
        Ok(path)
    }

    fn lock_artifact(&self, artifact: &Path) -> Result<File> {
        lock_artifact(&self.cache, artifact)
    }
}

/// Resolves an already-present model artifact without downloading or writing.
///
/// URL sources use the same source-keyed cache slot as [`ArtifactStore`].
/// Filesystem sources expand `~` with the same rules as provisioning.
///
/// # Errors
/// Returns [`LocalError`] when the source URL, home directory, or an existing
/// cache path is invalid.
pub fn existing_model_path(cache_root: &Path, source: &str) -> Result<Option<PathBuf>> {
    let path = if looks_like_url(source) {
        let name = filename_from_url(source)?;
        let relative = Path::new("models")
            .join(source_cache_key(source))
            .join(name);
        if !safe_relative_path(&relative) {
            return Err(LocalError::UnsafeCachePath {
                path: cache_root.join(relative),
            });
        }
        cache_root.join(relative)
    } else {
        expand_tilde(source)?
    };
    if !path.is_file() {
        return Ok(None);
    }
    if path.starts_with(cache_root) {
        validate_cache_path(cache_root, &path)?;
    }
    Ok(Some(path))
}

/// The blocking HTTP client shared by artifact provisioning and the blob
/// cache: gateway user agent, bounded connect, and a generous whole-request
/// ceiling (ART-003) behind the read loop's own idle bound.
///
/// # Errors
/// Returns [`LocalError::HttpClient`] when the client cannot be built.
pub(crate) fn download_client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("gateway/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_REQUEST_TIMEOUT)
        .build()
        .map_err(LocalError::HttpClient)
}

/// Takes the advisory OS lock serializing publishers of `artifact` under
/// `cache`, keyed by the artifact's cache-relative path.
///
/// The returned handle owns the lock; dropping it releases. Both the artifact
/// and the lock file are confinement-checked before use (ART-006/007).
///
/// # Errors
/// Returns [`LocalError`] when a path is unsafe or the lock cannot be taken.
pub(crate) fn lock_artifact(cache: &Path, artifact: &Path) -> Result<File> {
    validate_cache_path(cache, artifact)?;
    let relative = artifact
        .strip_prefix(cache)
        .map_err(|_| LocalError::UnsafeCachePath {
            path: artifact.to_owned(),
        })?;
    let mut hasher = Sha256::new();
    hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
    let lock_directory = cache.join(".locks");
    ensure_cache_directory(cache, &lock_directory)?;
    let lock_path = lock_directory.join(format!("{}.lock", hex_digest(hasher)));
    validate_cache_path(cache, &lock_path)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| LocalError::Io {
            operation: "open artifact lock",
            path: lock_path.clone(),
            source,
        })?;
    lock.lock().map_err(|source| LocalError::Io {
        operation: "lock artifact",
        path: lock_path,
        source,
    })?;
    validate_cache_path(cache, artifact)?;
    Ok(lock)
}

/// Whether a `[[local_model]]` source names a download rather than a path.
pub(crate) fn looks_like_url(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

/// A stable, filesystem-safe cache-slot key derived from the full source URL.
///
/// Two different URLs that share a filename map to different slots (ART-004),
/// while the same URL always maps to the same slot so a cache hit is stable.
pub(crate) fn source_cache_key(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hex_digest(hasher).chars().take(16).collect()
}

/// The URL's final path segment, validated as a safe relative filename.
///
/// # Errors
/// Returns [`LocalError::InvalidSource`] when the URL has no filename segment
/// or the segment is not a safe relative path.
pub fn filename_from_url(url: &str) -> Result<String> {
    let without_query = url.split('?').next().unwrap_or(url);
    let name = without_query
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| LocalError::InvalidSource {
            value: url.to_owned(),
            reason: "URL has no filename segment".to_owned(),
        })?;
    if !safe_relative_path(Path::new(name)) {
        return Err(LocalError::InvalidSource {
            value: url.to_owned(),
            reason: "URL filename is not a safe relative path".to_owned(),
        });
    }
    Ok(name.to_owned())
}

/// Expands a leading `~` in a path source against the operator home.
///
/// # Errors
/// Returns [`LocalError::MissingHome`] when the source needs a home directory
/// and none is available.
pub(crate) fn expand_tilde(source: &str) -> Result<PathBuf> {
    if source == "~" || source.starts_with("~/") || source.starts_with("~\\") {
        return Ok(expand_tilde_against(source, &default_home_checked()?));
    }
    Ok(PathBuf::from(source))
}

/// The pure core of [`expand_tilde`]: a leading `~`, `~/`, or `~\` resolves
/// against `home`; every other spelling passes through untouched.
pub(crate) fn expand_tilde_against(source: &str, home: &Path) -> PathBuf {
    if let Some(rest) = source.strip_prefix("~/") {
        return home.join(rest);
    }
    if let Some(rest) = source.strip_prefix("~\\") {
        return home.join(rest);
    }
    if source == "~" {
        return home.to_path_buf();
    }
    PathBuf::from(source)
}

/// Resolves the operator home for artifact provisioning, or a typed error.
///
/// Returns [`LocalError::MissingHome`] rather than silently using the working
/// directory when the home variable is unset or empty (ART-009).
pub(crate) fn default_home_checked() -> Result<PathBuf> {
    #[cfg(windows)]
    let (var, value) = ("USERPROFILE", std::env::var_os("USERPROFILE"));
    #[cfg(not(windows))]
    let (var, value) = ("HOME", std::env::var_os("HOME"));
    home_or_missing(var, value)
}

/// Pure resolver: an empty or absent home value is a [`LocalError::MissingHome`].
fn home_or_missing(var: &'static str, value: Option<std::ffi::OsString>) -> Result<PathBuf> {
    match value {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        _ => Err(LocalError::MissingHome { var }),
    }
}

/// Default artifact root (`~/.promptforge`), erroring when home is unset (ART-009).
pub(crate) fn default_promptforge_root_checked() -> Result<PathBuf> {
    Ok(default_home_checked()?.join(".promptforge"))
}

#[cfg(test)]
mod tests;
