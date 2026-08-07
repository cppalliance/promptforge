//! Pinned artifact provisioning for the explicit real-model test runner.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use thiserror::Error;

const LLAMA_RELEASE: &str = "b10082";
const INSTALL_MARKER: &str = ".promptforge-install";

/// Selects which pinned artifact set `provision` synchronizes.
///
/// One kind never downloads the other kind's artifacts: each kind names its
/// own model pin and its own server archive table, and GPU-enabled server
/// builds install under distinct platform keys so both installs coexist in
/// the cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelKind {
    /// The deterministic scenario suite: small pinned model, CPU-only server build.
    Scenario,
    /// Interactive prompt development: large pinned model, GPU-enabled server build.
    Dev,
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

const WINDOWS_X86_64_CPU: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "x86_64",
    platform: "windows-x86_64",
    archive_name: "llama-b10082-bin-win-cpu-x64.zip",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-cpu-x64.zip",
    sha256: "d606bd97164b61a3f504ded91d5c9a19f94281c6ac2e4672e09f85f41a232076",
    archive_kind: ArchiveKind::Zip,
    executable_name: "llama-server.exe",
};

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

const LINUX_X86_64_CPU: ServerAsset<'static> = ServerAsset {
    os: "linux",
    arch: "x86_64",
    platform: "linux-x86_64",
    archive_name: "llama-b10082-bin-ubuntu-x64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-x64.tar.gz",
    sha256: "01dcc9257ea1030bed5034aae667cd38c7f9cb620fd3e06c303d3813dd9e7d95",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
};

const LINUX_AARCH64_CPU: ServerAsset<'static> = ServerAsset {
    os: "linux",
    arch: "aarch64",
    platform: "linux-aarch64",
    archive_name: "llama-b10082-bin-ubuntu-arm64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-arm64.tar.gz",
    sha256: "16baaea628e228d0c546f4ddc9bef1b5182201caca75f65baa5e73ddff8d1204",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
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

const SCENARIO_SERVER_ASSETS: &[ServerAsset<'static>] = &[
    WINDOWS_X86_64_CPU,
    WINDOWS_AARCH64_CPU,
    LINUX_X86_64_CPU,
    LINUX_AARCH64_CPU,
    MACOS_X86_64,
    MACOS_AARCH64,
];

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileAsset<'a> {
    name: &'a str,
    url: &'a str,
    sha256: &'a str,
}

const SCENARIO_MODEL_ASSET: FileAsset<'static> = FileAsset {
    name: "Qwen3-0.6B-Q8_0.gguf",
    url: "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q8_0.gguf?download=true",
    sha256: "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031",
};

const DEV_MODEL_ASSET: FileAsset<'static> = FileAsset {
    name: "Qwen3.5-9B-Q4_K_M.gguf",
    url: "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf",
    sha256: "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8",
};

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("unsupported llama-server platform `{os}/{arch}`")]
    UnsupportedPlatform { os: String, arch: String },
    #[error("build HTTP client")]
    HttpClient(#[source] reqwest::Error),
    #[error("download `{url}`")]
    Download {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("read download from `{url}`")]
    DownloadRead {
        url: String,
        #[source]
        source: io::Error,
    },
    #[error("{operation} `{path}`")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("SHA-256 mismatch for `{name}`: expected {expected}, got {actual}")]
    DigestMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("unsafe or unsupported entry `{entry}` in archive `{archive}`")]
    UnsafeArchiveEntry { archive: String, entry: String },
    #[error("read archive `{archive}`")]
    Archive {
        archive: String,
        #[source]
        source: io::Error,
    },
    #[error("archive `{archive}` does not contain `{executable}`")]
    MissingExecutable { archive: String, executable: String },
    #[error("archive `{archive}` contains more than one `{executable}`")]
    DuplicateExecutable { archive: String, executable: String },
    #[error("invalid UTF-8 path inside `{path}`")]
    InvalidPath { path: PathBuf },
    #[error("cache path `{path}` escapes the cache or contains a link/reparse point")]
    UnsafeCachePath { path: PathBuf },
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub(crate) struct ProvisionedArtifacts {
    pub(crate) llama_server: PathBuf,
    pub(crate) model: PathBuf,
}

#[derive(Debug)]
struct Provisioner {
    cache: PathBuf,
    client: Client,
}

impl Provisioner {
    fn new(cache: &Path) -> Result<Self> {
        ensure_cache_directory(cache, cache)?;
        let client = Client::builder()
            .user_agent(concat!(
                "promptforge-core-tests/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(Error::HttpClient)?;
        Ok(Self {
            cache: cache.to_owned(),
            client,
        })
    }

    fn provision_file(&self, asset: FileAsset<'_>, directory: &str) -> Result<PathBuf> {
        let destination = self.cache_path(Path::new(directory).join(asset.name))?;
        self.ensure_blob(asset, &destination)?;
        Ok(destination)
    }

    fn provision_server(&self, asset: ServerAsset<'_>) -> Result<PathBuf> {
        let archive = self.cache_path(Path::new("downloads").join(asset.archive_name))?;
        let archive_asset = FileAsset {
            name: asset.archive_name,
            url: asset.url,
            sha256: asset.sha256,
        };
        self.ensure_blob(archive_asset, &archive)?;

        let install = self.cache_path(
            Path::new("llama.cpp").join(format!("{LLAMA_RELEASE}-{}", asset.platform)),
        )?;
        let _lock = self.lock_artifact(&install)?;
        validate_cache_path(&self.cache, &install)?;
        if Self::install_is_valid(&install, asset.sha256)? {
            println!("cache hit: llama-server installation {}", asset.platform);
            return find_executable(&install, asset.executable_name, asset.archive_name);
        }
        self.ensure_blob(archive_asset, &archive)?;
        validate_cache_path(&self.cache, &archive)?;

        println!(
            "installing cached llama-server archive for {}",
            asset.platform
        );
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
                .map_err(|source| Error::Io {
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
                return Err(Error::Io {
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
            if file_digest(destination)? == asset.sha256 {
                println!("cache hit: {}", asset.name);
                return Ok(());
            }
            remove_cache_entry(&self.cache, destination)?;
        } else if destination.exists() {
            remove_cache_entry(&self.cache, destination)?;
        }

        let Some(parent) = destination.parent() else {
            return Err(Error::InvalidPath {
                path: destination.to_owned(),
            });
        };
        ensure_cache_directory(&self.cache, parent)?;
        validate_cache_path(&self.cache, &staging)?;
        println!("downloading pinned artifact: {}", asset.name);
        let actual = match self.download(asset.url, &staging) {
            Ok(actual) => actual,
            Err(error) => {
                let _ignored = fs::remove_file(&staging);
                return Err(error);
            }
        };
        if actual != asset.sha256 {
            remove_cache_entry(&self.cache, &staging)?;
            return Err(Error::DigestMismatch {
                name: asset.name.to_owned(),
                expected: asset.sha256.to_owned(),
                actual,
            });
        }
        rename_confined(&self.cache, &staging, destination)
    }

    fn download(&self, url: &str, destination: &Path) -> Result<String> {
        let mut response = self
            .client
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|source| Error::Download {
                url: url.to_owned(),
                source,
            })?;
        let file = File::create(destination).map_err(|source| Error::Io {
            operation: "create partial download",
            path: destination.to_owned(),
            source,
        })?;
        let mut writer = BufWriter::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
        loop {
            let count = response
                .read(&mut buffer)
                .map_err(|source| Error::DownloadRead {
                    url: url.to_owned(),
                    source,
                })?;
            if count == 0 {
                break;
            }
            writer
                .write_all(&buffer[..count])
                .map_err(|source| Error::Io {
                    operation: "write partial download",
                    path: destination.to_owned(),
                    source,
                })?;
            hasher.update(&buffer[..count]);
        }
        writer.flush().map_err(|source| Error::Io {
            operation: "flush partial download",
            path: destination.to_owned(),
            source,
        })?;
        writer.get_ref().sync_all().map_err(|source| Error::Io {
            operation: "sync partial download",
            path: destination.to_owned(),
            source,
        })?;
        Ok(hex_digest(hasher))
    }

    fn cache_path(&self, relative: PathBuf) -> Result<PathBuf> {
        if !safe_relative_path(&relative) {
            return Err(Error::UnsafeCachePath {
                path: self.cache.join(relative),
            });
        }
        let path = self.cache.join(relative);
        validate_cache_path(&self.cache, &path)?;
        Ok(path)
    }

    fn lock_artifact(&self, artifact: &Path) -> Result<File> {
        validate_cache_path(&self.cache, artifact)?;
        let relative = artifact
            .strip_prefix(&self.cache)
            .map_err(|_| Error::UnsafeCachePath {
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
            .map_err(|source| Error::Io {
                operation: "open artifact lock",
                path: lock_path.clone(),
                source,
            })?;
        lock.lock().map_err(|source| Error::Io {
            operation: "lock artifact",
            path: lock_path,
            source,
        })?;
        validate_cache_path(&self.cache, artifact)?;
        Ok(lock)
    }
}

pub(crate) fn provision(kind: ModelKind) -> Result<ProvisionedArtifacts> {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.model-cache");
    let provisioner = Provisioner::new(&cache)?;
    let server = server_asset(kind, std::env::consts::OS, std::env::consts::ARCH)?;
    provision_assets(&provisioner, server, model_asset(kind))
}

fn provision_assets(
    provisioner: &Provisioner,
    server: ServerAsset<'_>,
    model: FileAsset<'_>,
) -> Result<ProvisionedArtifacts> {
    Ok(ProvisionedArtifacts {
        llama_server: provisioner.provision_server(server)?,
        model: provisioner.provision_file(model, "models")?,
    })
}

const fn model_asset(kind: ModelKind) -> FileAsset<'static> {
    match kind {
        ModelKind::Scenario => SCENARIO_MODEL_ASSET,
        ModelKind::Dev => DEV_MODEL_ASSET,
    }
}

const fn server_assets(kind: ModelKind) -> &'static [ServerAsset<'static>] {
    match kind {
        ModelKind::Scenario => SCENARIO_SERVER_ASSETS,
        ModelKind::Dev => DEV_SERVER_ASSETS,
    }
}

fn server_asset(kind: ModelKind, os: &str, arch: &str) -> Result<ServerAsset<'static>> {
    server_assets(kind)
        .iter()
        .copied()
        .find(|asset| asset.os == os && asset.arch == arch)
        .ok_or_else(|| Error::UnsupportedPlatform {
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
    let file = File::open(archive).map_err(|source| Error::Io {
        operation: "open archive",
        path: archive.to_owned(),
        source,
    })?;
    let mut tar = tar::Archive::new(GzDecoder::new(BufReader::new(file)));
    let entries = tar.entries().map_err(|source| Error::Archive {
        archive: archive.display().to_string(),
        source,
    })?;
    for entry in entries {
        let mut entry = entry.map_err(|source| Error::Archive {
            archive: archive.display().to_string(),
            source,
        })?;
        let entry_path = entry
            .path()
            .map_err(|source| Error::Archive {
                archive: archive.display().to_string(),
                source,
            })?
            .into_owned();
        if !safe_archive_path(&entry_path)
            || !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir())
        {
            return Err(Error::UnsafeArchiveEntry {
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
            .map_err(|source| Error::Archive {
                archive: archive.display().to_string(),
                source,
            })?;
        if !unpacked {
            return Err(Error::UnsafeArchiveEntry {
                archive: archive.display().to_string(),
                entry: entry_path.display().to_string(),
            });
        }
        validate_tree_path(destination, &output)?;
    }
    Ok(())
}

fn extract_zip(archive: &Path, destination: &Path) -> Result<()> {
    let file = File::open(archive).map_err(|source| Error::Io {
        operation: "open archive",
        path: archive.to_owned(),
        source,
    })?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|source| Error::Archive {
        archive: archive.display().to_string(),
        source: io::Error::other(source),
    })?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|source| Error::Archive {
            archive: archive.display().to_string(),
            source: io::Error::other(source),
        })?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(Error::UnsafeArchiveEntry {
                archive: archive.display().to_string(),
                entry: entry.name().to_owned(),
            });
        };
        if !safe_archive_name(entry.name()) || !safe_relative_path(&relative) {
            return Err(Error::UnsafeArchiveEntry {
                archive: archive.display().to_string(),
                entry: entry.name().to_owned(),
            });
        }
        let mode = entry.unix_mode();
        if !zip_entry_type_is_supported(mode, entry.is_dir()) {
            return Err(Error::UnsafeArchiveEntry {
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
            return Err(Error::InvalidPath { path: output });
        };
        ensure_cache_directory(destination, parent)?;
        validate_tree_path(destination, &output)?;
        let mut file = File::create(&output).map_err(|source| Error::Io {
            operation: "create extracted file",
            path: output.clone(),
            source,
        })?;
        io::copy(&mut entry, &mut file).map_err(|source| Error::Io {
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
        Error::Io {
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
        .map_err(|source| Error::Io {
            operation: "inspect executable permissions",
            path: path.to_owned(),
            source,
        })?
        .permissions()
        .mode();
    if mode & 0o111 == 0 {
        return Err(Error::UnsafeArchiveEntry {
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
        [] => Err(Error::MissingExecutable {
            archive: archive.to_owned(),
            executable: name.to_owned(),
        }),
        [path] => Ok(path.clone()),
        _ => Err(Error::DuplicateExecutable {
            archive: archive.to_owned(),
            executable: name.to_owned(),
        }),
    }
}

fn collect_named_files(root: &Path, name: &OsStr, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(root).map_err(|source| Error::Io {
        operation: "read installation directory",
        path: root.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            operation: "read installation entry",
            path: root.to_owned(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| Error::Io {
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
        .map_err(|source| Error::Io {
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
    let entries = fs::read_dir(directory).map_err(|source| Error::Io {
        operation: "read installation directory",
        path: directory.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            operation: "read installation entry",
            path: directory.to_owned(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| Error::Io {
            operation: "inspect installation entry",
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_tree_files(root, &path, files)?;
        } else if file_type.is_file() && path != root.join(INSTALL_MARKER) {
            let relative = path
                .strip_prefix(root)
                .map_err(|source| Error::Io {
                    operation: "resolve installation entry",
                    path: path.clone(),
                    source: io::Error::other(source),
                })?
                .to_str()
                .ok_or_else(|| Error::InvalidPath { path: path.clone() })?
                .replace('\\', "/");
            files.push((relative, path));
        } else if !file_type.is_file() {
            return Err(Error::UnsafeArchiveEntry {
                archive: root.display().to_string(),
                entry: path.display().to_string(),
            });
        }
    }
    Ok(())
}

fn file_digest(path: &Path) -> Result<String> {
    let file = File::open(path).map_err(|source| Error::Io {
        operation: "open cached artifact",
        path: path.to_owned(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let count = reader.read(&mut buffer).map_err(|source| Error::Io {
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
        fs::create_dir_all(root).map_err(|source| Error::Io {
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
                    return Err(Error::UnsafeCachePath { path: current });
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                if let Err(source) = fs::create_dir(&current)
                    && source.kind() != io::ErrorKind::AlreadyExists
                {
                    return Err(Error::Io {
                        operation: "create cache directory",
                        path: current.clone(),
                        source,
                    });
                }
                validate_tree_path(root, &current)?;
            }
            Err(source) => {
                return Err(Error::Io {
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
            return Err(Error::Io {
                operation: "inspect cache entry",
                path: path.to_owned(),
                source,
            });
        }
    };
    if is_link_or_reparse(&metadata) {
        return Err(Error::UnsafeCachePath {
            path: path.to_owned(),
        });
    }
    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|source| Error::Io {
        operation: "remove cache entry",
        path: path.to_owned(),
        source,
    })
}

fn rename_confined(root: &Path, source: &Path, destination: &Path) -> Result<()> {
    validate_tree_path(root, source)?;
    validate_tree_path(root, destination)?;
    fs::rename(source, destination).map_err(|error| Error::Io {
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
    let root_metadata = fs::symlink_metadata(root).map_err(|source| Error::Io {
        operation: "inspect cache root",
        path: root.to_owned(),
        source,
    })?;
    if is_link_or_reparse(&root_metadata) || !root_metadata.is_dir() {
        return Err(Error::UnsafeCachePath {
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
                    return Err(Error::UnsafeCachePath { path: current });
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(Error::Io {
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
        .map_err(|_| Error::UnsafeCachePath {
            path: path.to_owned(),
        })?;
    if !relative.as_os_str().is_empty() && !safe_relative_path(relative) {
        return Err(Error::UnsafeCachePath {
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
    let mut file = File::create(path).map_err(|source| Error::Io {
        operation: "create install marker",
        path: path.to_owned(),
        source,
    })?;
    file.write_all(contents).map_err(|source| Error::Io {
        operation: "write install marker",
        path: path.to_owned(),
        source,
    })?;
    file.sync_all().map_err(|source| Error::Io {
        operation: "sync install marker",
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests;
