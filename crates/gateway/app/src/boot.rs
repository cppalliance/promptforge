//! Boot-time configuration: discovery and first-run provisioning.
//!
//! An explicit config path (the CLI `--config` flag or
//! `PROMPTFORGE_GATEWAY_CONFIG`, resolved by the binary) always wins.
//! Without one, the discovery search looks beside the executable, then in
//! the working directory, then in the user profile's `.promptforge`
//! directory. When no location holds a `gateway.toml`, first-run
//! generation writes the sidecar default - loopback on an OS-assigned
//! port, a fresh random bearer key, the recommended STT pair unless the
//! installer declined it - into the profile location, and the boot
//! proceeds from it.

use std::path::{Path, PathBuf};

use crate::ProfileName;

/// Canonical file name searched for at each candidate location.
const CONFIG_FILE_NAME: &str = "gateway.toml";

/// The profile the generated default contains and selects.
const DEFAULT_PROFILE: &str = "default";

/// The release artifact the cloud provider model sheet downloads from.
pub(crate) const DEFAULT_SHEET_URL: &str = "https://github.com/cppalliance/promptforge-cloud-providers/releases/download/models/cloud-provider-models.json";

/// The environment override for the sheet URL, matching the repo's
/// `PROMPTFORGE_*` convention; there is no config-schema knob.
pub(crate) const SHEET_URL_ENV: &str = "PROMPTFORGE_MODELS_SHEET_URL";

/// The sheet cache file name inside the profile directory.
pub(crate) const CACHE_FILE_NAME: &str = "cloud-provider-models.json";

/// The installer's STT choice for first-run generation.
///
/// The NSIS components page records the choice as the `InstallSTT` DWORD
/// under `HKCU\Software\PromptForge\PromptForge`; a bare
/// `promptforge-gateway` run outside the installer finds no value and ships
/// STT, as does every non-Windows machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallerStt {
    /// The generated config includes the recommended STT pair.
    Included,
    /// The installer was told to skip STT; the generated config omits the
    /// pair and the profile selects nothing.
    Omitted,
}

impl InstallerStt {
    /// Reads the installer's recorded choice. Absent means included.
    #[must_use]
    pub(crate) fn read() -> InstallerStt {
        installer_stt()
    }

    /// Maps the installer's `InstallSTT` DWORD: zero omits STT; absent or
    /// nonzero ships it. Compiled on every platform - gating it to Windows
    /// would leave `Omitted` with no construction site elsewhere, and
    /// `dead_code` fires under the Linux CI clippy run.
    #[must_use]
    fn from_dword(value: Option<u32>) -> InstallerStt {
        match value {
            Some(0) => InstallerStt::Omitted,
            _ => InstallerStt::Included,
        }
    }
}

/// The Windows read of the installer's choice.
#[cfg(windows)]
fn installer_stt() -> InstallerStt {
    InstallerStt::from_dword(registry::install_stt_dword())
}

/// Non-Windows machines have no installer registry; the absent value ships STT.
#[cfg(not(windows))]
fn installer_stt() -> InstallerStt {
    InstallerStt::from_dword(None)
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "the registry shims (RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, RegDeleteValueW) are raw Win32 with no safe wrapper"
)]
pub(crate) mod registry;

/// A boot-time discovery or first-run generation failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BootError {
    /// A process location could not be determined.
    #[error("locate {what}")]
    Locate {
        /// What could not be located.
        what: &'static str,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The executable path has no parent directory.
    #[error("the executable has no parent directory")]
    NoExeDir,
    /// The user profile directory could not be determined; it holds the
    /// fallback search location and receives the generated default.
    #[error("locate the user profile directory")]
    NoHome,
    /// The config path has no parent directory to create.
    #[error("the config path {} has no parent directory", path.display())]
    NoParent {
        /// The path without a parent.
        path: PathBuf,
    },
    /// A directory could not be created.
    #[error("create {}", path.display())]
    CreateDir {
        /// The directory that could not be created.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The config file could not be written.
    #[error("write {}", path.display())]
    Write {
        /// The path that could not be written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The sibling state file selecting the default profile could not be
    /// written.
    #[error("persist the default profile selection")]
    ProfileState(#[source] gateway_config::ConfigError),
}

/// The three base directories discovery searches, gathered from the
/// process. Tests inject fixed locations through [`resolve_in`].
#[derive(Debug)]
struct Locations {
    exe_dir: PathBuf,
    cwd: PathBuf,
    home: PathBuf,
}

impl Locations {
    /// Reads the process's executable, working, and profile directories.
    fn gather() -> Result<Locations, BootError> {
        let exe = std::env::current_exe().map_err(|source| BootError::Locate {
            what: "the executable",
            source,
        })?;
        let exe_dir = exe
            .parent()
            .map(Path::to_path_buf)
            .ok_or(BootError::NoExeDir)?;
        let cwd = std::env::current_dir().map_err(|source| BootError::Locate {
            what: "the current directory",
            source,
        })?;
        let home = std::env::home_dir().ok_or(BootError::NoHome)?;
        Ok(Locations { exe_dir, cwd, home })
    }
}

/// Resolves the boot config path: an explicit path wins; otherwise the
/// discovery search; otherwise first-run generation into the profile
/// location.
///
/// # Errors
/// Returns [`BootError`] when a process location cannot be determined or
/// the generated default cannot be written.
pub(crate) fn resolve_boot_config(explicit: Option<PathBuf>) -> Result<PathBuf, BootError> {
    resolve_in(explicit, Locations::gather, InstallerStt::read())
}

/// The testable resolution chain: `gather` runs only when `explicit` is
/// `None`, so an explicit-path boot never depends on location lookups - a
/// bare server may have no resolvable profile directory at all.
fn resolve_in(
    explicit: Option<PathBuf>,
    gather: impl FnOnce() -> Result<Locations, BootError>,
    stt: InstallerStt,
) -> Result<PathBuf, BootError> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let locations = gather()?;
    if let Some(path) = first_existing(&candidates_from(
        &locations.exe_dir,
        &locations.cwd,
        &locations.home,
    )) {
        return Ok(path);
    }
    let path = profile_config_path(&locations.home);
    generate_default(&path, stt)?;
    Ok(path)
}

/// The config path a diagnostics report names: the explicit path when
/// given, else the first discovery candidate that exists, else the
/// profile location first-run generation would write. Reads only - it
/// never generates. `None` when no location can be determined at all.
pub(crate) fn discover_for_report(explicit: Option<PathBuf>) -> Option<PathBuf> {
    discover_in(explicit, Locations::gather)
}

/// The testable discovery chain: like [`resolve_in`], `gather` runs only
/// when `explicit` is `None`, so an explicit-path report never depends on
/// location lookups. Unlike `resolve_in` this never generates: the
/// profile location is named, not written.
fn discover_in(
    explicit: Option<PathBuf>,
    gather: impl FnOnce() -> Result<Locations, BootError>,
) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit;
    }
    let locations = gather().ok()?;
    Some(
        first_existing(&candidates_from(
            &locations.exe_dir,
            &locations.cwd,
            &locations.home,
        ))
        .unwrap_or_else(|| profile_config_path(&locations.home)),
    )
}

/// The profile candidate: `<home>/.promptforge/gateway.toml`. This is the
/// one definition of the profile configuration's location, so first-run
/// generation writes where discovery reads.
fn profile_config_path(home: &Path) -> PathBuf {
    home.join(".promptforge").join(CONFIG_FILE_NAME)
}

/// Builds the candidate list in search order from the three base
/// directories.
fn candidates_from(exe_dir: &Path, cwd: &Path, home: &Path) -> Vec<PathBuf> {
    vec![
        exe_dir.join(CONFIG_FILE_NAME),
        cwd.join(CONFIG_FILE_NAME),
        home.join(".promptforge").join(CONFIG_FILE_NAME),
    ]
}

/// Returns the first candidate path that exists, if any.
fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.is_file()).cloned()
}

/// Writes the default configuration to `path` with a fresh random bearer
/// key, plus the sibling state file selecting the `default` profile so the
/// generated config boots with no `--profile` flag on every run. The write
/// is create-new: an existing file is never overwritten, so two racing
/// first runs both boot from the winner's file.
///
/// Returns the config path.
///
/// # Errors
/// Returns [`BootError`] when the parent directory, the config file, or
/// the state file cannot be created or written.
fn generate_default(path: &Path, stt: InstallerStt) -> Result<PathBuf, BootError> {
    let dir = path.parent().ok_or_else(|| BootError::NoParent {
        path: path.to_path_buf(),
    })?;
    std::fs::create_dir_all(dir).map_err(|source| BootError::CreateDir {
        path: dir.to_path_buf(),
        source,
    })?;
    if write_new_config(path, &default_boot_config(&generate_api_key(), stt))? {
        tracing::info!(
            "no gateway.toml found; wrote default config to {}",
            path.display()
        );
    } else {
        tracing::info!(
            "a racing first run wrote {} first; using the existing file",
            path.display()
        );
    }
    let Ok(profile) = ProfileName::parse(DEFAULT_PROFILE) else {
        unreachable!("DEFAULT_PROFILE is a valid name");
    };
    gateway_config::persist_profile_state(path, &profile).map_err(BootError::ProfileState)?;
    Ok(path.to_path_buf())
}

/// Writes `contents` to `path` only when no file exists there: the open is
/// create-new, so a racing first run loses to the winner's file instead of
/// truncating it, and a symlink planted at `path` is never followed into a
/// victim file. Returns `true` when this call created the file.
///
/// # Errors
/// Returns [`BootError::Write`] when the file cannot be created or written
/// for a reason other than it already existing.
fn write_new_config(path: &Path, contents: &str) -> Result<bool, BootError> {
    use std::io::Write as _;
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(source) => {
            return Err(BootError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    file.write_all(contents.as_bytes())
        .map_err(|source| BootError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(true)
}

/// The recommended STT pair: digest-pinned whisper.cpp models, one interim
/// and one final.
const STT_MODELS_TOML: &str = r#"
[[stt_model]]
name = "whisper-base-en"
role = "interim"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
sha256 = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"
vram_gb = 1.0

[[stt_model]]
name = "whisper-small-en"
role = "final"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin"
sha256 = "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d"
vram_gb = 2.0
"#;

/// The boot configuration written on first run, with a freshly generated
/// bearer key baked in.
///
/// The gateway binds loopback on an OS-assigned port; the gateway
/// discovery file written after the bind records the real port. There is
/// no `[workshop]` section: the shell serves the workshop UI itself.
fn default_boot_config(api_key: &str, stt: InstallerStt) -> String {
    let (stt_models, profile_models) = match stt {
        InstallerStt::Included => (
            STT_MODELS_TOML,
            "[\"whisper-base-en\", \"whisper-small-en\"]",
        ),
        InstallerStt::Omitted => ("", "[]"),
    };
    format!(
        r#"config-version = 0

# PromptForge gateway configuration
# Generated on first run. Edit as needed.
# See: crates/gateway/app/README.md
# Diagnostics: promptforge-gateway diagnostics

[server]
bind = "127.0.0.1:0"
api_key = "{api_key}"
# Loopback callers need no key. On a shared machine any local account can
# use the gateway - set trust_loopback = false to require the key from all.
trust_loopback = true
{stt_models}
[[profile]]
name = "default"
models = {profile_models}
"#
    )
}

/// A fresh random bearer key for the generated `[server]` section, using
/// the OS-seeded cryptographic RNG (`rand::rng`, a ChaCha-based CSPRNG)
/// rather than a fast non-cryptographic generator, since the key guards
/// the gateway's listener.
fn generate_api_key() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    format!("{:016x}{:016x}", rng.random::<u64>(), rng.random::<u64>())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod provisioning_tests;

#[cfg(all(test, feature = "stt"))]
mod boot_speech_tests;
