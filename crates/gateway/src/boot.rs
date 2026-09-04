//! Boot-time configuration: discovery and first-run provisioning.
//!
//! An explicit config path (the CLI positional or `PROMPTFORGE_GATEWAY_CONFIG`,
//! resolved by the binary) always wins. Without one, the discovery search
//! looks beside the executable, then in the working directory, then in the
//! user profile's `.promptforge` directory. When no location holds a
//! `gateway.toml`, first-run generation writes the sidecar-shaped default -
//! loopback on an OS-assigned port, a fresh random bearer key, the
//! recommended STT pair unless the installer declined it - into the profile
//! location, and the boot proceeds from it.

use std::path::{Path, PathBuf};

use crate::ProfileName;

/// Canonical file name searched for at each candidate location.
const CONFIG_FILE_NAME: &str = "gateway.toml";

/// The profile the generated default carries and selects.
pub(crate) const DEFAULT_PROFILE: &str = "default";

/// The installer's STT choice for first-run generation.
///
/// The NSIS components page records the choice as the `InstallSTT` DWORD
/// under `HKCU\Software\PromptForge\PromptForge`; a bare
/// `promptforge-gateway` run outside the installer finds no value and ships
/// STT, as does every non-Windows host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallerStt {
    /// The generated config carries the recommended STT pair.
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
    /// nonzero ships it. Compiled on every host - gating it to Windows
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

/// Non-Windows hosts have no installer registry; the absent value ships STT.
#[cfg(not(windows))]
fn installer_stt() -> InstallerStt {
    InstallerStt::from_dword(None)
}

/// The installer's STT choice and the tray's Launch at Login entry, both
/// recorded in the registry. Raw Win32 FFI has no safe wrapper in the tree.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "the registry shims (RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, RegDeleteValueW) are raw Win32 with no safe wrapper"
)]
pub(crate) mod registry {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_SZ, RegCloseKey,
        RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };

    /// The autostart entry the tray's Launch at Login check item manages:
    /// `HKCU\...\CurrentVersion\Run\PromptForgeGateway`.
    const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    /// The Run-key value name holding the gateway's login command line.
    const RUN_VALUE: &str = "PromptForgeGateway";

    /// Opens one of this module's subkeys, returning the live handle. The
    /// caller closes it with [`close_key`] on every path.
    ///
    /// # Errors
    /// Returns the registry status code when the key cannot be opened.
    fn open_key(subkey: &str, access: u32) -> Result<HKEY, u32> {
        let subkey: Vec<u16> = subkey.encode_utf16().chain(Some(0)).collect();
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `subkey` is a valid null-terminated UTF-16 string and
        // `key` is valid for one HKEY write; on success `key` holds a live
        // handle that `close_key` releases exactly once.
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, access, &raw mut key) };
        if status != ERROR_SUCCESS {
            return Err(status);
        }
        Ok(key)
    }

    /// Closes a handle from [`open_key`].
    ///
    /// # Safety
    /// `key` must be a live handle from a successful [`open_key`] that has
    /// not been closed yet; it is closed exactly once here.
    fn close_key(key: HKEY) {
        // SAFETY: per the contract above, `key` is live and closed once.
        unsafe {
            RegCloseKey(key);
        }
    }

    /// The registry error code as an `io::Error`, for the write shims whose
    /// callers report failures.
    fn status_error(status: u32) -> std::io::Error {
        std::io::Error::from_raw_os_error(status.cast_signed())
    }

    /// Reads `HKCU\Software\PromptForge\PromptForge\InstallSTT` as a DWORD,
    /// or `None` when the key or value is absent, carries another type, or
    /// cannot be read.
    pub(super) fn install_stt_dword() -> Option<u32> {
        let key = open_key("Software\\PromptForge\\PromptForge", KEY_READ).ok()?;
        let value: Vec<u16> = "InstallSTT".encode_utf16().chain(Some(0)).collect();
        let mut data = 0u32;
        let mut kind = 0u32;
        // A REG_DWORD is 4 bytes; the query fails rather than truncates when
        // the buffer is too small.
        let mut len = 4u32;
        // SAFETY: `key` is the live handle opened above; `value` is a valid
        // null-terminated UTF-16 string; `data` is valid for `len` bytes of
        // writes and `kind` and `len` are valid for one u32 write each.
        let status = unsafe {
            RegQueryValueExW(
                key,
                value.as_ptr(),
                std::ptr::null(),
                &raw mut kind,
                std::ptr::from_mut(&mut data).cast::<u8>(),
                &raw mut len,
            )
        };
        close_key(key);
        if status != ERROR_SUCCESS || kind != REG_DWORD || len != 4 {
            return None;
        }
        Some(data)
    }

    /// Reads the Launch at Login command line from the Run key, or `None`
    /// when the value is absent, is not a string, or cannot be read. The
    /// tray's check item derives its state from this read alone - never
    /// from local config - because the user can revoke the entry
    /// externally.
    pub(crate) fn read_run_value() -> Option<String> {
        let key = open_key(RUN_SUBKEY, KEY_READ).ok()?;
        let value: Vec<u16> = RUN_VALUE.encode_utf16().chain(Some(0)).collect();
        let mut kind = 0u32;
        let mut len = 0u32;
        // SAFETY: `key` is live; `value` is a valid null-terminated UTF-16
        // string; the null buffer queries the required size into `len`.
        let status = unsafe {
            RegQueryValueExW(
                key,
                value.as_ptr(),
                std::ptr::null(),
                &raw mut kind,
                std::ptr::null_mut(),
                &raw mut len,
            )
        };
        if status != ERROR_SUCCESS || kind != REG_SZ || len == 0 {
            close_key(key);
            return None;
        }
        let mut buffer = vec![0u16; (len as usize).div_ceil(2) + 1];
        // SAFETY: `key` is live; `buffer` is valid for `len` bytes of
        // writes plus a spare terminator word, and `kind`/`len` are valid
        // for one u32 write each.
        let status = unsafe {
            RegQueryValueExW(
                key,
                value.as_ptr(),
                std::ptr::null(),
                &raw mut kind,
                buffer.as_mut_ptr().cast::<u8>(),
                &raw mut len,
            )
        };
        close_key(key);
        if status != ERROR_SUCCESS {
            return None;
        }
        let words = (len as usize) / 2;
        String::from_utf16(&buffer[..words])
            .ok()
            .map(|text| text.trim_end_matches('\0').to_owned())
    }

    /// Writes the Launch at Login command line to the Run key, creating the
    /// value when absent.
    ///
    /// # Errors
    /// Returns the registry error when the key cannot be opened or the
    /// value cannot be written.
    pub(crate) fn write_run_value(command: &str) -> std::io::Result<()> {
        let key = open_key(RUN_SUBKEY, KEY_SET_VALUE).map_err(status_error)?;
        let value: Vec<u16> = RUN_VALUE.encode_utf16().chain(Some(0)).collect();
        let wide: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
        let len = u32::try_from(wide.len() * 2).unwrap_or(u32::MAX);
        // SAFETY: `key` is live; `value` and `wide` are valid
        // null-terminated UTF-16 strings; `wide` is readable for `len`
        // bytes.
        let status = unsafe {
            RegSetValueExW(
                key,
                value.as_ptr(),
                0,
                REG_SZ,
                wide.as_ptr().cast::<u8>(),
                len,
            )
        };
        close_key(key);
        if status != ERROR_SUCCESS {
            return Err(status_error(status));
        }
        Ok(())
    }

    /// Deletes the Launch at Login value from the Run key. An absent value
    /// is a success: delete is idempotent.
    ///
    /// # Errors
    /// Returns the registry error when the key cannot be opened or the
    /// value cannot be deleted.
    pub(crate) fn delete_run_value() -> std::io::Result<()> {
        let key = open_key(RUN_SUBKEY, KEY_SET_VALUE).map_err(status_error)?;
        let value: Vec<u16> = RUN_VALUE.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `key` is live and `value` is a valid null-terminated
        // UTF-16 string.
        let status = unsafe { RegDeleteValueW(key, value.as_ptr()) };
        close_key(key);
        if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
            return Err(status_error(status));
        }
        Ok(())
    }
}

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

/// The profile candidate: `<home>/.promptforge/gateway.toml`. This is the
/// one place that knows where the profile configuration lives, so
/// first-run generation writes where discovery reads.
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
pub(crate) fn generate_default(path: &Path, stt: InstallerStt) -> Result<PathBuf, BootError> {
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
/// The gateway binds loopback on an OS-assigned port; the connection file
/// written after the bind carries the real port. There is no `[workshop]`
/// section: the shell hosts the workshop UI itself.
fn default_boot_config(api_key: &str, stt: InstallerStt) -> String {
    let (stt_models, profile_models) = match stt {
        InstallerStt::Included => (
            STT_MODELS_TOML,
            "[\"whisper-base-en\", \"whisper-small-en\"]",
        ),
        InstallerStt::Omitted => ("", "[]"),
    };
    format!(
        r#"config-version = 2

# PromptForge gateway configuration
# Generated on first run. Edit as needed.
# See: crates/gateway/README.md

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
mod tests {
    use super::*;

    /// Hex characters [`generate_api_key`] produces (two u64s, 128 bits).
    const API_KEY_LENGTH: usize = 32;

    /// Fixed locations rooted at a tempdir, for [`resolve_in`].
    fn locations(temp: &tempfile::TempDir) -> Locations {
        Locations {
            exe_dir: temp.path().join("exe"),
            cwd: temp.path().join("cwd"),
            home: temp.path().join("home"),
        }
    }

    #[test]
    fn candidates_are_ordered_exe_then_cwd_then_profile() {
        let candidates = candidates_from(
            Path::new("exe-dir"),
            Path::new("cwd-dir"),
            Path::new("home-dir"),
        );
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("exe-dir/gateway.toml"),
                PathBuf::from("cwd-dir/gateway.toml"),
                PathBuf::from("home-dir/.promptforge/gateway.toml"),
            ]
        );
    }

    #[test]
    fn the_first_existing_candidate_wins() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let dirs = locations(&temp);
        for dir in [&dirs.exe_dir, &dirs.cwd] {
            std::fs::create_dir_all(dir).expect("create fixture dir");
        }
        let promptforge = dirs.home.join(".promptforge");
        std::fs::create_dir_all(&promptforge).expect("create profile dir");
        let in_cwd = dirs.cwd.join(CONFIG_FILE_NAME);
        let in_home = promptforge.join(CONFIG_FILE_NAME);
        std::fs::write(&in_cwd, "").expect("write fixture");
        std::fs::write(&in_home, "").expect("write fixture");

        let candidates = candidates_from(&dirs.exe_dir, &dirs.cwd, &dirs.home);
        assert_eq!(
            first_existing(&candidates).as_deref(),
            Some(in_cwd.as_path()),
            "the current directory beats the profile"
        );

        let in_exe = dirs.exe_dir.join(CONFIG_FILE_NAME);
        std::fs::write(&in_exe, "").expect("write fixture");
        assert_eq!(
            first_existing(&candidates).as_deref(),
            Some(in_exe.as_path()),
            "beside the executable beats everything"
        );
    }

    #[test]
    fn no_config_returns_none() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let dirs = locations(&temp);
        let candidates = candidates_from(&dirs.exe_dir, &dirs.cwd, &dirs.home);
        assert_eq!(first_existing(&candidates), None);
    }

    #[test]
    fn an_explicit_path_wins_without_touching_the_disk() {
        let resolved = resolve_in(
            Some(PathBuf::from("explicit/gateway.toml")),
            || panic!("an explicit path skips the location lookup"),
            InstallerStt::Included,
        )
        .expect("the explicit path resolves");
        assert_eq!(resolved, PathBuf::from("explicit/gateway.toml"));
    }

    #[test]
    fn discovery_finds_an_existing_config_without_generating() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let dirs = locations(&temp);
        std::fs::create_dir_all(&dirs.cwd).expect("create cwd");
        let in_cwd = dirs.cwd.join(CONFIG_FILE_NAME);
        std::fs::write(&in_cwd, "").expect("write fixture");

        let resolved = resolve_in(None, || Ok(locations(&temp)), InstallerStt::Included)
            .expect("discovery resolves");

        assert_eq!(resolved, in_cwd);
        assert!(
            !dirs.home.join(".promptforge").exists(),
            "a discovered config means no first-run generation"
        );
    }

    #[test]
    fn first_run_generates_a_bootable_config_into_the_profile() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let dirs = locations(&temp);

        let resolved = resolve_in(None, || Ok(locations(&temp)), InstallerStt::Included)
            .expect("first run generates");

        assert_eq!(resolved, profile_config_path(&dirs.home));
        // The boot path itself: the generated file loads with no CLI or
        // environment profile, because generation wrote the sibling state
        // file selecting `default`.
        let config = gateway_config::Config::load(
            &resolved,
            &gateway_config::ProfileSelection::new(None, None),
        )
        .expect("the generated config boots with no profile flags");
        assert_eq!(
            config
                .active_profile()
                .map(gateway_config::ProfileConfig::name),
            Some(DEFAULT_PROFILE),
            "the state file selects the generated profile on every boot"
        );
    }

    #[test]
    fn the_generated_config_binds_loopback_on_an_os_assigned_port() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let path = generate_default(&temp.path().join(CONFIG_FILE_NAME), InstallerStt::Included)
            .expect("generates");
        let config = gateway_config::Config::from_toml_str(
            &std::fs::read_to_string(&path).expect("generated config reads"),
        )
        .expect("the generated config parses");

        assert_eq!(
            config.server().bind().to_string(),
            "127.0.0.1:0",
            "the sidecar bind is loopback with an OS-assigned port"
        );
        assert!(
            config.workshop().is_none(),
            "the generated config carries no [workshop] section"
        );
        let key = config.server().api_key().expose();
        assert_eq!(key.len(), API_KEY_LENGTH);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn the_generated_config_carries_the_recommended_stt_pair_by_default() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let path = generate_default(&temp.path().join(CONFIG_FILE_NAME), InstallerStt::Included)
            .expect("generates");
        let raw = std::fs::read_to_string(&path).expect("read back");
        let config = gateway_config::Config::from_toml_str(&raw).expect("the config parses");

        let stt = config.catalog_stt_models();
        assert_eq!(stt.len(), 2);
        assert_eq!(stt[0].name(), "whisper-base-en");
        assert_eq!(stt[1].name(), "whisper-small-en");
        assert!(raw.contains("sha256 = "), "the pair stays digest-pinned");
        assert!(
            raw.contains("models = [\"whisper-base-en\", \"whisper-small-en\"]"),
            "the default profile selects the pair"
        );
    }

    #[test]
    fn the_generated_config_omits_stt_when_the_installer_declined_it() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let path = generate_default(&temp.path().join(CONFIG_FILE_NAME), InstallerStt::Omitted)
            .expect("generates");
        let raw = std::fs::read_to_string(&path).expect("read back");
        let config = gateway_config::Config::from_toml_str(&raw).expect("the config parses");

        assert!(
            config.catalog_stt_models().is_empty(),
            "no [[stt_model]] entries when the installer declined STT"
        );
        assert!(
            !raw.contains("stt_model"),
            "the file carries no STT text at all: {raw}"
        );
        assert!(raw.contains("models = []"), "the profile selects nothing");
        assert_eq!(config.server().bind().to_string(), "127.0.0.1:0");
        assert_eq!(config.server().api_key().expose().len(), API_KEY_LENGTH);
    }

    #[test]
    fn generation_never_overwrites_an_existing_config() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let path = temp.path().join(CONFIG_FILE_NAME);
        std::fs::write(&path, "sentinel").expect("write fixture");

        let written = generate_default(&path, InstallerStt::Included).expect("generation succeeds");

        assert_eq!(written, path);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "sentinel",
            "an existing config is never overwritten, key included"
        );
    }

    #[test]
    fn the_generated_config_without_stt_assembles_a_gateway() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let path = generate_default(&temp.path().join(CONFIG_FILE_NAME), InstallerStt::Omitted)
            .expect("generates");
        let config =
            gateway_config::Config::load(&path, &gateway_config::ProfileSelection::new(None, None))
                .expect("the generated config loads");
        crate::Gateway::from_config(&config, crate::ProfilesContext::default())
            .expect("the generated config assembles a gateway");
    }

    #[test]
    fn generated_api_keys_are_random_hex() {
        let first = generate_api_key();
        let second = generate_api_key();
        assert_eq!(first.len(), API_KEY_LENGTH);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second, "two first runs must not share a bearer key");
    }

    #[test]
    fn the_installer_dword_maps_only_zero_to_omitted() {
        assert_eq!(InstallerStt::from_dword(None), InstallerStt::Included);
        assert_eq!(InstallerStt::from_dword(Some(0)), InstallerStt::Omitted);
        assert_eq!(InstallerStt::from_dword(Some(1)), InstallerStt::Included);
    }
}
