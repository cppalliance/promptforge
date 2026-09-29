//! Pinned runtime installs: `llama-server` selection, the whisper.cpp runtime, archive
//! extraction, and install markers.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gateway_config::{LlamaBackend, WhisperBackend};
use gateway_progress::Activity;
use tokio_util::sync::CancellationToken;

use super::archive::{extract_archive_with_progress, find_executable, require_executable};
use super::assets::{
    self, ArchiveKind, FileAsset, LLAMA_RELEASE, ServerAsset, server_asset,
    whisper_asset_with_probe,
};
use super::confine::validate_tree_path;
use super::digest::tree_digest;
use super::{
    ArtifactStore, INSTALL_MARKER, InstallAsset, ProvisionedServer, Result, ServerSelection,
    ensure_cache_directory, expand_tilde, part_path, remove_cache_entry, rename_confined,
    validate_cache_path, whisper_install_asset, write_synced,
};
use crate::error::LocalError;

#[path = "install-machine.rs"]
mod machine;

use machine::{host_x86_extensions, nvidia_probe};

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

impl ArtifactStore {
    /// Resolves the `llama-server` executable for this machine: the configured
    /// `llama_server_path` first, then the `PROMPTFORGE_LLAMA_SERVER`
    /// environment variable, then the managed download of the pinned build
    /// for the selected backend, writing the download, verify, and extract
    /// stages into `activity`'s text, when given.
    ///
    /// # Errors
    /// Returns a [`LocalError`] when an explicit path is invalid, the
    /// platform is unsupported, or provisioning fails.
    pub(crate) fn provision_llama_server_with_progress(
        &self,
        selection: &ServerSelection<'_>,
        activity: Option<&Activity>,
    ) -> Result<ProvisionedServer> {
        self.provision_llama_server_with_cancellation(selection, activity, None)
    }

    /// [`Self::provision_llama_server_with_progress`] variant that stops at
    /// download chunk boundaries and phase boundaries when `token` fires.
    pub(crate) fn provision_llama_server_with_cancellation(
        &self,
        selection: &ServerSelection<'_>,
        activity: Option<&Activity>,
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
        let probe = if std::env::consts::OS == "windows"
            && std::env::consts::ARCH == "x86_64"
            && selection.backend == LlamaBackend::Auto
        {
            nvidia_probe()
        } else {
            None
        };
        let asset = server_asset(
            std::env::consts::OS,
            std::env::consts::ARCH,
            selection.backend,
            probe.as_ref().map(|probe| probe.compute_caps.as_slice()),
        )?;
        let executable = self.provision_server(asset, activity, token)?;
        Ok(ProvisionedServer {
            executable,
            path_prefix: Vec::new(),
        })
    }

    /// Provisions the pinned whisper.cpp runtime for this machine and returns
    /// the shared library path.
    ///
    /// `backend` (the `[stt] whisper_backend` setting) chooses between the
    /// CPU and CUDA builds on Windows x86-64 and Linux x86-64, where `auto`
    /// probes the host's NVIDIA GPUs and driver version; every other
    /// platform has one build. On x86-64 the host CPU must report every
    /// extension the builds execute, under every setting.
    /// The archive is downloaded, digest-verified, and extracted under the
    /// artifact cache. Its sibling ggml and GPU runtime libraries stay beside
    /// the returned file for the platform loader.
    ///
    /// # Errors
    /// Returns [`LocalError::UnsupportedCpu`] when an x86-64 CPU lacks an
    /// extension the selected build executes, and another [`LocalError`]
    /// when the platform is unsupported or download, verification,
    /// extraction, or cache publication fails.
    pub fn provision_whisper_library(
        &self,
        backend: WhisperBackend,
        activity: Option<&Activity>,
    ) -> Result<PathBuf> {
        let asset = whisper_asset_with_probe(
            std::env::consts::OS,
            std::env::consts::ARCH,
            backend,
            nvidia_probe,
            &host_x86_extensions(),
        )?;
        let archives = [asset.archive];
        self.provision_install(whisper_install_asset(asset, &archives), activity, None)
    }

    fn provision_server(
        &self,
        asset: ServerAsset<'_>,
        activity: Option<&Activity>,
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
            activity,
            token,
        )
    }

    fn provision_install(
        &self,
        asset: InstallAsset<'_>,
        activity: Option<&Activity>,
        token: Option<&CancellationToken>,
    ) -> Result<PathBuf> {
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
            if let Err(error) =
                self.ensure_blob_with_progress(file_asset, &archive, activity, token)
            {
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
            if let Err(error) =
                extract_archive_with_progress(archive, &staging, archive_ref.archive_kind, activity)
            {
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
    pub(super) fn install_is_valid(install: &Path, asset: &ServerAsset<'_>) -> Result<bool> {
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
}
