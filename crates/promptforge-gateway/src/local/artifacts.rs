//! Pinned `llama-server` binaries and GGUF cache for gateway-owned local inference.
//!
//! Downloads land under the operator cache (`~/.promptforge` by default). The
//! `llama-server` build is the same b10082 pin used by `promptforge-core-tests`,
//! preferring GPU-enabled archives (Vulkan on Windows/Linux, Metal on macOS).

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};

use crate::local::error::LocalError;

const LLAMA_RELEASE: &str = "b10082";
const INSTALL_MARKER: &str = ".promptforge-install";
/// Non-TTY log cadence: every 64 MiB or 5% of Content-Length, whichever fires first.
const LOG_PROGRESS_BYTES: u64 = 64 * 1024 * 1024;
/// Connect timeout for artifact downloads (bounds a stalled connect).
const DOWNLOAD_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Hard ceiling on a single artifact, guarding the cache volume against a
/// malicious or mistaken endpoint. Generous enough for large GGUF weights.
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024 * 1024;

/// Whether `url` is an HTTPS Hugging Face host eligible for the hub bearer token.
///
/// The token is attached only to these hosts so an operator's `HF_TOKEN` is
/// never disclosed to an arbitrary (or plaintext-HTTP) endpoint named in a
/// `[[local_model]].source`.
fn is_huggingface_https(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(parsed) => parsed.scheme() == "https" && parsed.host_str().is_some_and(is_hf_host),
        Err(_) => false,
    }
}

fn is_hf_host(host: &str) -> bool {
    host == "huggingface.co" || host.ends_with(".huggingface.co")
}

type Result<T> = std::result::Result<T, LocalError>;

/// Cache root plus HTTP client for provisioning local inference artifacts.
#[derive(Debug)]
pub(crate) struct ArtifactStore {
    cache: PathBuf,
    client: Client,
}

impl ArtifactStore {
    /// Creates a store rooted at `cache`, creating the directory if needed.
    ///
    /// # Errors
    /// Returns [`LocalError::Io`] or [`LocalError::HttpClient`] on setup failure.
    pub(crate) fn new(cache: impl Into<PathBuf>) -> Result<Self> {
        let cache = cache.into();
        ensure_cache_directory(&cache, &cache)?;
        let client = Client::builder()
            .user_agent(concat!("promptforge-gateway/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
            .build()
            .map_err(LocalError::HttpClient)?;
        Ok(Self { cache, client })
    }

    /// Ensures the pinned GPU-capable `llama-server` for this host is installed.
    ///
    /// # Errors
    /// Returns a [`LocalError`] when the platform is unsupported or provisioning fails.
    pub(crate) fn provision_llama_server(&self) -> Result<PathBuf> {
        let asset = server_asset(std::env::consts::OS, std::env::consts::ARCH)?;
        self.provision_server(asset)
    }

    /// Ensures a GGUF (or other blob) from `source` is available locally.
    ///
    /// `source` is either an `http(s)://` URL or a filesystem path (`~` expanded).
    /// When `sha256` is `Some`, the digest is verified after download and on cache hit.
    ///
    /// # Errors
    /// Returns a [`LocalError`] on download, verification, or path failures.
    pub(crate) fn ensure_model(&self, source: &str, sha256: Option<&str>) -> Result<PathBuf> {
        if looks_like_url(source) {
            let name = filename_from_url(source)?;
            let destination = self.cache_path(Path::new("models").join(&name))?;
            let asset = FileAsset {
                name: &name,
                url: source,
                sha256,
            };
            self.ensure_blob(asset, &destination)?;
            return Ok(destination);
        }
        let path = expand_tilde(source);
        if !path.is_file() {
            return Err(LocalError::InvalidSource {
                value: source.to_owned(),
                reason: "path is not an existing file".to_owned(),
            });
        }
        if let Some(expected) = sha256 {
            let actual = file_digest(&path)?;
            if actual != expected {
                return Err(LocalError::DigestMismatch {
                    name: path.display().to_string(),
                    expected: expected.to_owned(),
                    actual,
                });
            }
        }
        Ok(path)
    }

    fn provision_server(&self, asset: ServerAsset<'_>) -> Result<PathBuf> {
        let archive = self.cache_path(Path::new("downloads").join(asset.archive_name))?;
        let archive_asset = FileAsset {
            name: asset.archive_name,
            url: asset.url,
            sha256: Some(asset.sha256),
        };
        self.ensure_blob(archive_asset, &archive)?;

        let install = self.cache_path(
            Path::new("llama.cpp").join(format!("{LLAMA_RELEASE}-{}", asset.platform)),
        )?;
        let _lock = self.lock_artifact(&install)?;
        validate_cache_path(&self.cache, &install)?;
        if Self::install_is_valid(&install, asset.sha256)? {
            return find_executable(&install, asset.executable_name, asset.archive_name);
        }
        self.ensure_blob(archive_asset, &archive)?;
        validate_cache_path(&self.cache, &archive)?;

        remove_cache_entry(&self.cache, &install)?;
        let staging = part_path(&install);
        remove_cache_entry(&self.cache, &staging)?;
        ensure_cache_directory(&self.cache, &staging)?;

        if let Err(error) = extract_archive(&archive, &staging, asset.archive_kind) {
            let _ignored = fs::remove_dir_all(&staging);
            return Err(error);
        }

        let staged_executable =
            find_executable(&staging, asset.executable_name, asset.archive_name)?;
        if asset.archive_kind == ArchiveKind::TarGz {
            require_executable(&staged_executable, asset.archive_name)?;
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
        write_synced(
            &marker,
            format!("{}\n{tree_sha256}\n", asset.sha256).as_bytes(),
        )?;
        rename_confined(&self.cache, &staging, &install)?;
        Ok(install.join(relative_executable))
    }

    fn install_is_valid(install: &Path, archive_sha256: &str) -> Result<bool> {
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
        let mut lines = marker_text.lines();
        let Some(recorded_archive) = lines.next() else {
            return Ok(false);
        };
        let Some(recorded_tree) = lines.next() else {
            return Ok(false);
        };
        if lines.next().is_some() || recorded_archive != archive_sha256 {
            return Ok(false);
        }
        Ok(tree_digest(install)? == recorded_tree)
    }

    fn ensure_blob(&self, asset: FileAsset<'_>, destination: &Path) -> Result<()> {
        let _lock = self.lock_artifact(destination)?;
        validate_cache_path(&self.cache, destination)?;
        let staging = part_path(destination);
        remove_cache_entry(&self.cache, &staging)?;

        if destination.is_file() {
            match asset.sha256 {
                Some(expected) => {
                    if file_digest(destination)? == expected {
                        return Ok(());
                    }
                    remove_cache_entry(&self.cache, destination)?;
                }
                None => return Ok(()),
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
        let actual = match self.download(asset.url, &staging) {
            Ok(actual) => actual,
            Err(error) => {
                let _ignored = fs::remove_file(&staging);
                return Err(error);
            }
        };
        if let Some(expected) = asset.sha256
            && actual != expected
        {
            remove_cache_entry(&self.cache, &staging)?;
            return Err(LocalError::DigestMismatch {
                name: asset.name.to_owned(),
                expected: expected.to_owned(),
                actual,
            });
        }
        rename_confined(&self.cache, &staging, destination)
    }

    fn download(&self, url: &str, destination: &Path) -> Result<String> {
        let label = download_label(url);
        let progress = progress_for_download(&label, io::stderr().is_terminal());
        match self.download_with_progress(url, destination, progress.as_ref()) {
            Ok(digest) => {
                progress.finish();
                Ok(digest)
            }
            Err(error) => {
                progress.abandon();
                Err(error)
            }
        }
    }

    fn download_with_progress(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn DownloadProgress,
    ) -> Result<String> {
        let mut request = self.client.get(url);
        if is_huggingface_https(url)
            && let Some(token) = hub_bearer_token(env_var)
        {
            request = request.bearer_auth(token);
        }
        let mut response = request
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|source| LocalError::Download {
                url: url.to_owned(),
                source,
            })?;
        let total = response.content_length();
        if let Some(total) = total
            && total > MAX_ARTIFACT_BYTES
        {
            return Err(LocalError::Server(format!(
                "artifact at {url} declares {total} bytes, exceeding the {MAX_ARTIFACT_BYTES}-byte limit"
            )));
        }
        progress.set_len(total);
        let file = File::create(destination).map_err(|source| LocalError::Io {
            operation: "create partial download",
            path: destination.to_owned(),
            source,
        })?;
        let mut writer = BufWriter::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
        let mut downloaded: u64 = 0;
        loop {
            let count = response
                .read(&mut buffer)
                .map_err(|source| LocalError::DownloadRead {
                    url: url.to_owned(),
                    source,
                })?;
            if count == 0 {
                break;
            }
            downloaded = downloaded.saturating_add(count as u64);
            if downloaded > MAX_ARTIFACT_BYTES {
                return Err(LocalError::Server(format!(
                    "artifact at {url} exceeded the {MAX_ARTIFACT_BYTES}-byte limit mid-stream"
                )));
            }
            writer
                .write_all(&buffer[..count])
                .map_err(|source| LocalError::Io {
                    operation: "write partial download",
                    path: destination.to_owned(),
                    source,
                })?;
            hasher.update(&buffer[..count]);
            progress.inc(count as u64);
        }
        writer.flush().map_err(|source| LocalError::Io {
            operation: "flush partial download",
            path: destination.to_owned(),
            source,
        })?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|source| LocalError::Io {
                operation: "sync partial download",
                path: destination.to_owned(),
                source,
            })?;
        Ok(hex_digest(hasher))
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
        validate_cache_path(&self.cache, artifact)?;
        let relative =
            artifact
                .strip_prefix(&self.cache)
                .map_err(|_| LocalError::UnsafeCachePath {
                    path: artifact.to_owned(),
                })?;
        let mut hasher = Sha256::new();
        hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        let lock_directory = self.cache.join(".locks");
        ensure_cache_directory(&self.cache, &lock_directory)?;
        let lock_path = lock_directory.join(format!("{}.lock", hex_digest(hasher)));
        validate_cache_path(&self.cache, &lock_path)?;
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
        validate_cache_path(&self.cache, artifact)?;
        Ok(lock)
    }
}

fn looks_like_url(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

fn filename_from_url(url: &str) -> Result<String> {
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

fn expand_tilde(source: &str) -> PathBuf {
    if let Some(rest) = source.strip_prefix("~/") {
        return default_home().join(rest);
    }
    if let Some(rest) = source.strip_prefix("~\\") {
        return default_home().join(rest);
    }
    if source == "~" {
        return default_home();
    }
    PathBuf::from(source)
}

/// Returns the operator home directory used for `~` expansion and defaults.
#[must_use]
pub(crate) fn default_home() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map_or_else(|| PathBuf::from("."), PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
    }
}

/// Default root for gateway local artifacts (`~/.promptforge`).
#[must_use]
pub(crate) fn default_promptforge_root() -> PathBuf {
    default_home().join(".promptforge")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ServerAsset<'a> {
    os: &'a str,
    arch: &'a str,
    platform: &'a str,
    archive_name: &'a str,
    url: &'a str,
    sha256: &'a str,
    archive_kind: ArchiveKind,
    executable_name: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileAsset<'a> {
    name: &'a str,
    url: &'a str,
    sha256: Option<&'a str>,
}

const WINDOWS_AARCH64_CPU: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "aarch64",
    platform: "windows-aarch64",
    archive_name: "llama-b10082-bin-win-cpu-arm64.zip",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-cpu-arm64.zip",
    sha256: "50dab63396f579cc0ceb4a4fc4b985414d55aaebd4722f363ad03696648711a4",
    archive_kind: ArchiveKind::Zip,
    executable_name: "llama-server.exe",
};

// The macOS release tars are already Metal-enabled, so both kinds share them.
const MACOS_X86_64: ServerAsset<'static> = ServerAsset {
    os: "macos",
    arch: "x86_64",
    platform: "macos-x86_64",
    archive_name: "llama-b10082-bin-macos-x64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-macos-x64.tar.gz",
    sha256: "5a28fad0f05bf283c1adb92224c1bf3c25ee06acd0f4065b170016c14b490473",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
};

const MACOS_AARCH64: ServerAsset<'static> = ServerAsset {
    os: "macos",
    arch: "aarch64",
    platform: "macos-aarch64",
    archive_name: "llama-b10082-bin-macos-arm64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-macos-arm64.tar.gz",
    sha256: "d644e16eefef3402e4fa86c0fcdce3b00a6786db68c3f216875ce87b45d29173",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
};

const WINDOWS_X86_64_VULKAN: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "x86_64",
    platform: "windows-x86_64-vulkan",
    archive_name: "llama-b10082-bin-win-vulkan-x64.zip",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-vulkan-x64.zip",
    sha256: "0a4b2e41cfb950da9a749baf8978e0626690fbead3b0ca96860785484cda5bde",
    archive_kind: ArchiveKind::Zip,
    executable_name: "llama-server.exe",
};

const LINUX_X86_64_VULKAN: ServerAsset<'static> = ServerAsset {
    os: "linux",
    arch: "x86_64",
    platform: "linux-x86_64-vulkan",
    archive_name: "llama-b10082-bin-ubuntu-vulkan-x64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-vulkan-x64.tar.gz",
    sha256: "9003ea32e3d5d8a01da3e4b5d3124e0d21c63d51e112c40f5dcdef91ffaca7cc",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
};

const LINUX_AARCH64_VULKAN: ServerAsset<'static> = ServerAsset {
    os: "linux",
    arch: "aarch64",
    platform: "linux-aarch64-vulkan",
    archive_name: "llama-b10082-bin-ubuntu-vulkan-arm64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-vulkan-arm64.tar.gz",
    sha256: "2805902c3074f615a0105a5325ee29799500c8e29c90ccb986b59e1141df551e",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
};

// No Vulkan build exists for Windows arm64 in release b10082, so the dev
// table falls back to the CPU archive there.
const DEV_SERVER_ASSETS: &[ServerAsset<'static>] = &[
    WINDOWS_X86_64_VULKAN,
    WINDOWS_AARCH64_CPU,
    LINUX_X86_64_VULKAN,
    LINUX_AARCH64_VULKAN,
    MACOS_X86_64,
    MACOS_AARCH64,
];

fn server_asset(os: &str, arch: &str) -> Result<ServerAsset<'static>> {
    DEV_SERVER_ASSETS
        .iter()
        .copied()
        .find(|asset| asset.os == os && asset.arch == arch)
        .ok_or_else(|| LocalError::UnsupportedPlatform {
            os: os.to_owned(),
            arch: arch.to_owned(),
        })
}

fn extract_archive(archive: &Path, destination: &Path, kind: ArchiveKind) -> Result<()> {
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
        if !safe_archive_path(&entry_path)
            || !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir())
        {
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

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn safe_archive_path(path: &Path) -> bool {
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

#[cfg(unix)]
fn require_executable(path: &Path, archive: &str) -> Result<()> {
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
fn require_executable(_path: &Path, _archive: &str) -> Result<()> {
    Ok(())
}

fn find_executable(root: &Path, name: &str, archive: &str) -> Result<PathBuf> {
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

fn tree_digest(root: &Path) -> Result<String> {
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

fn file_digest(path: &Path) -> Result<String> {
    let file = File::open(path).map_err(|source| LocalError::Io {
        operation: "open cached artifact",
        path: path.to_owned(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
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
    }
    Ok(hex_digest(hasher))
}

/// Reads a process environment variable as UTF-8 text.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Reads the HF bearer token from the process environment.
pub(crate) fn hub_bearer_token_from_env() -> Option<String> {
    hub_bearer_token(env_var)
}

/// Hugging Face hub bearer token for gated downloads.
///
/// Prefers `HF_TOKEN`, then `HUGGING_FACE_HUB_TOKEN`. Empty or whitespace-only
/// values are ignored. The token is never logged.
fn hub_bearer_token(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    for key in ["HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"] {
        if let Some(value) = lookup(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

/// Progress updates for a single HTTP blob download.
trait DownloadProgress: Send {
    fn set_len(&self, total: Option<u64>);
    fn inc(&self, n: u64);
    fn finish(&self);
    fn abandon(&self);
}

/// Basename (or short fallback) shown on the progress bar / log lines.
fn download_label(url: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|part| {
            let name = part.split('?').next().unwrap_or(part);
            (!name.is_empty()).then(|| name.to_owned())
        })
        .unwrap_or_else(|| "download".to_owned())
}

/// Chooses a TTY progress bar or non-TTY tracing progress for `label`.
fn progress_for_download(label: &str, is_tty: bool) -> Box<dyn DownloadProgress> {
    if is_tty {
        Box::new(IndicatifProgress::new(label))
    } else {
        Box::new(TracingProgress::new(label))
    }
}

/// Interactive stderr progress bar via indicatif.
struct IndicatifProgress {
    bar: ProgressBar,
}

impl IndicatifProgress {
    fn new(label: &str) -> Self {
        let bar = ProgressBar::new_spinner();
        bar.set_message(label.to_owned());
        if let Some(style) = bar_style() {
            bar.set_style(style);
        }
        Self { bar }
    }
}

fn bar_style() -> Option<ProgressStyle> {
    ProgressStyle::with_template(
        "{spinner:.green} {msg} [{bar:40.cyan/blue}] {percent:>3}% {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
    )
    .ok()
    .map(|style| style.progress_chars("=>-"))
}

fn spinner_style() -> Option<ProgressStyle> {
    ProgressStyle::with_template("{spinner:.green} {msg} {bytes} ({bytes_per_sec})").ok()
}

impl DownloadProgress for IndicatifProgress {
    fn set_len(&self, total: Option<u64>) {
        match total {
            Some(len) if len > 0 => {
                self.bar.set_length(len);
                if let Some(style) = bar_style() {
                    self.bar.set_style(style);
                }
            }
            _ => {
                if let Some(style) = spinner_style() {
                    self.bar.set_style(style);
                }
            }
        }
    }

    fn inc(&self, n: u64) {
        self.bar.inc(n);
    }

    fn finish(&self) {
        self.bar.finish_and_clear();
    }

    fn abandon(&self) {
        self.bar.abandon();
    }
}

/// Non-TTY progress: periodic `tracing::info!` lines.
struct TracingProgress {
    label: String,
    total: AtomicU64,
    downloaded: AtomicU64,
    last_logged: AtomicU64,
}

impl TracingProgress {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            total: AtomicU64::new(0),
            downloaded: AtomicU64::new(0),
            last_logged: AtomicU64::new(0),
        }
    }

    fn maybe_log(&self, force: bool) {
        let downloaded = self.downloaded.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        let last = self.last_logged.load(Ordering::Relaxed);
        let step = if total > 0 {
            (total / 20).max(LOG_PROGRESS_BYTES)
        } else {
            LOG_PROGRESS_BYTES
        };
        if !force && downloaded.saturating_sub(last) < step && downloaded != 0 {
            return;
        }
        self.last_logged.store(downloaded, Ordering::Relaxed);
        if let Some(percent) = downloaded.saturating_mul(100).checked_div(total) {
            tracing::info!(
                file = %self.label,
                downloaded,
                total,
                percent,
                "download progress"
            );
        } else {
            tracing::info!(
                file = %self.label,
                downloaded,
                "download progress"
            );
        }
    }
}

impl DownloadProgress for TracingProgress {
    fn set_len(&self, total: Option<u64>) {
        if let Some(len) = total {
            self.total.store(len, Ordering::Relaxed);
        }
        tracing::info!(
            file = %self.label,
            total = total.unwrap_or(0),
            "download started"
        );
    }

    fn inc(&self, n: u64) {
        self.downloaded.fetch_add(n, Ordering::Relaxed);
        self.maybe_log(false);
    }

    fn finish(&self) {
        self.maybe_log(true);
        tracing::info!(
            file = %self.label,
            downloaded = self.downloaded.load(Ordering::Relaxed),
            "download finished"
        );
    }

    fn abandon(&self) {
        tracing::warn!(
            file = %self.label,
            downloaded = self.downloaded.load(Ordering::Relaxed),
            "download abandoned"
        );
    }
}

fn hex_digest(hasher: Sha256) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = hasher.finalize();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

fn ensure_cache_directory(root: &Path, directory: &Path) -> Result<()> {
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

fn remove_cache_entry(root: &Path, path: &Path) -> Result<()> {
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

fn rename_confined(root: &Path, source: &Path, destination: &Path) -> Result<()> {
    validate_tree_path(root, source)?;
    validate_tree_path(root, destination)?;
    fs::rename(source, destination).map_err(|error| LocalError::Io {
        operation: "atomically install artifact",
        path: destination.to_owned(),
        source: error,
    })
}

fn validate_cache_path(root: &Path, path: &Path) -> Result<()> {
    validate_tree_path(root, path)
}

fn validate_tree_path(root: &Path, path: &Path) -> Result<()> {
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

fn write_synced(path: &Path, contents: &[u8]) -> Result<()> {
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

#[cfg(test)]
mod tests;
