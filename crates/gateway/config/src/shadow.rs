//! Single-file pending configuration shadow and the real profile-state file.
//!
//! `gateway.toml.next` is the only shadow: a save stages the whole config
//! document there, and apply promotes it. Profile selection is never
//! staged; `POST /admin/switch-profile` writes or deletes the real
//! `gateway.state.toml` directly.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use toml::Value;

use self::content::restore_secrets;
use crate::config::Config;
use crate::error::ConfigError as Repr;
use crate::profile::{ProfileName, ProfileSelection, ProfileState};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[path = "shadow-content.rs"]
mod content;
pub use content::pending_var_references;

#[cfg(test)]
#[path = "shadow-tests.rs"]
mod tests;

/// Paths staged by one pending configuration write.
///
/// # Examples
/// ```no_run
/// use gateway_config::save_config_shadow;
/// use std::path::Path;
///
/// let document = toml::from_str("config-version = 0")?;
/// let shadows = save_config_shadow(Path::new("gateway.toml"), document)?;
/// assert!(shadows.config.ends_with("gateway.toml.next"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingShadows {
    /// Shadow containing the global configuration and profile checklists.
    pub config: PathBuf,
}

/// Summary of pending single-file changes.
///
/// # Examples
/// ```no_run
/// use gateway_config::pending_report;
/// use std::path::Path;
///
/// let report = pending_report(Path::new("gateway.toml"))?;
/// assert!(report.changed_sections.windows(2).all(|pair| pair[0] <= pair[1]));
/// # Ok::<(), gateway_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingReport {
    /// Real config files that have shadows: the config path when
    /// `gateway.toml.next` exists, otherwise empty.
    pub shadowed_files: Vec<PathBuf>,
    /// Changed top-level config keys, sorted.
    pub changed_sections: Vec<String>,
}

/// Returns a managed file's shadow path.
///
/// `gateway.toml` maps to `gateway.toml.next`.
///
/// # Examples
/// ```
/// use gateway_config::shadow_path;
/// use std::path::Path;
///
/// assert_eq!(
///     shadow_path(Path::new("gateway.toml")),
///     Path::new("gateway.toml.next")
/// );
/// ```
#[must_use]
pub fn shadow_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map_or_else(
        || std::ffi::OsString::from("shadow"),
        std::ffi::OsStr::to_os_string,
    );
    name.push(".next");
    path.with_file_name(name)
}

/// Writes a complete sibling shadow through a temporary file and rename.
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) when writing or renaming fails.
///
/// # Examples
/// ```no_run
/// use gateway_config::write_shadow;
/// use std::path::Path;
///
/// let path = write_shadow(Path::new("gateway.toml"), "config-version = 0\n")?;
/// assert!(path.ends_with("gateway.toml.next"));
/// # Ok::<(), gateway_config::ConfigError>(())
/// ```
pub fn write_shadow(target: &Path, contents: &str) -> Result<PathBuf, crate::ConfigError> {
    write_shadow_repr(target, contents).map_err(crate::ConfigError::from)
}

fn write_shadow_repr(target: &Path, contents: &str) -> Result<PathBuf, Repr> {
    let shadow = shadow_path(target);
    write_atomic_repr(&shadow, contents)?;
    Ok(shadow)
}

/// Replaces `target` with `contents` through a temporary file and rename.
///
/// This is the primitive behind [`write_shadow`] and
/// [`persist_profile_state`], exposed so a caller holding a file's intended
/// contents can commit them without staging a shadow first.
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) when writing or renaming fails.
///
/// # Examples
/// ```no_run
/// use gateway_config::write_atomic;
/// use std::path::Path;
///
/// write_atomic(Path::new("gateway.toml"), "config-version = 0\n")?;
/// # Ok::<(), gateway_config::ConfigError>(())
/// ```
pub fn write_atomic(target: &Path, contents: &str) -> Result<(), crate::ConfigError> {
    write_atomic_repr(target, contents).map_err(crate::ConfigError::from)
}

fn write_atomic_repr(target: &Path, contents: &str) -> Result<(), Repr> {
    let temp = unique_sidecar(target, "tmp");
    if let Err(source) = fs::write(&temp, contents) {
        let _ = fs::remove_file(&temp);
        return Err(Repr::Write { path: temp, source });
    }
    if let Err(error) = replace_file(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

fn unique_sidecar(path: &Path, kind: &str) -> PathBuf {
    let mut name = path.file_name().map_or_else(
        || std::ffi::OsString::from("shadow"),
        std::ffi::OsStr::to_os_string,
    );
    name.push(format!(
        ".{kind}-{}-{}",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

fn replace_file(source_path: &Path, destination: &Path) -> Result<(), Repr> {
    let first_error = match fs::rename(source_path, destination) {
        Ok(()) => return Ok(()),
        Err(source) => source,
    };
    if !destination.exists() {
        return Err(Repr::Write {
            path: destination.to_owned(),
            source: first_error,
        });
    }
    if !destination.is_file() {
        return Err(Repr::Write {
            path: destination.to_owned(),
            source: first_error,
        });
    }

    let backup = unique_sidecar(destination, "backup");
    fs::rename(destination, &backup).map_err(|source| Repr::Write {
        path: destination.to_owned(),
        source,
    })?;
    if let Err(source) = fs::rename(source_path, destination) {
        if let Err(restore_source) = fs::rename(&backup, destination) {
            return Err(Repr::Write {
                path: backup,
                source: std::io::Error::new(
                    restore_source.kind(),
                    format!(
                        "replacement failed ({source}); original remains at backup path: \
                         {restore_source}"
                    ),
                ),
            });
        }
        return Err(Repr::Write {
            path: destination.to_owned(),
            source,
        });
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

/// Promotes one shadow to its real file.
///
/// Replacement is atomic on platforms that let rename overwrite a file. The
/// fallback first preserves the old file under a private backup name and
/// restores it if the second rename fails.
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) when the shadow is absent or
/// the rename fails.
///
/// # Examples
/// ```no_run
/// use gateway_config::promote_shadow;
/// use std::path::Path;
///
/// promote_shadow(Path::new("gateway.toml"))?;
/// # Ok::<(), gateway_config::ConfigError>(())
/// ```
pub fn promote_shadow(target: &Path) -> Result<(), crate::ConfigError> {
    let shadow = shadow_path(target);
    if !shadow.is_file() {
        return Err(crate::ConfigError::from(Repr::Write {
            path: shadow,
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no shadow to promote"),
        }));
    }
    replace_file(&shadow, target).map_err(crate::ConfigError::from)?;
    Ok(())
}

/// Persists the active profile to the real sibling state file.
///
/// The file is replaced through a unique temporary file and rename, so a
/// reader never observes a partial write.
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) when rendering, writing, or
/// replacing the sibling state file fails.
///
/// # Examples
/// ```no_run
/// use gateway_config::{ProfileName, persist_profile_state};
/// use std::path::Path;
///
/// let profile = ProfileName::parse("work")?;
/// persist_profile_state(Path::new("gateway.toml"), &profile)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn persist_profile_state(
    config_path: &Path,
    profile: &ProfileName,
) -> Result<(), crate::ConfigError> {
    let rendered = ProfileState::new(profile).to_toml_string()?;
    write_atomic_repr(&crate::profile_state_path(config_path), &rendered)
        .map_err(crate::ConfigError::from)
}

/// Deletes the sibling state file, the persisted form of "no profile".
///
/// The inverse of [`persist_profile_state`]. An already absent state file is
/// success: the persisted selection is "none" either way.
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) when the state file exists
/// and cannot be removed.
///
/// # Examples
/// ```no_run
/// use gateway_config::clear_profile_state;
/// use std::path::Path;
///
/// clear_profile_state(Path::new("gateway.toml"))?;
/// # Ok::<(), gateway_config::ConfigError>(())
/// ```
pub fn clear_profile_state(config_path: &Path) -> Result<(), crate::ConfigError> {
    let path = crate::profile_state_path(config_path);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        // `Repr::Write` displays as a shadow write; the wrapped source names
        // the operation that failed so the operator reads a removal error.
        Err(source) => Err(crate::ConfigError::from(Repr::Write {
            source: std::io::Error::new(
                source.kind(),
                format!("remove state file {}: {source}", path.display()),
            ),
            path,
        })),
    }
}

/// Validates and stages one pending admin document.
///
/// The document is the global config alone. Profile selection is not a
/// configuration key: a document with `active_profile` is refused, so
/// the Config UI cannot stage a selection that `POST /admin/switch-profile`
/// owns. Redacted secrets are restored from the current pending config
/// before validation. The persisted selection is not checked against the
/// document: a state file naming a profile the document drops degrades to
/// "no profile" at the next load, the same way [`Config::load`] treats it.
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) when the document is malformed
/// or contains `active_profile`, a secret cannot be restored, the config is
/// invalid, or the shadow cannot be written.
///
/// # Examples
/// ```no_run
/// use gateway_config::save_config_shadow;
/// use std::path::Path;
///
/// let document = toml::from_str("config-version = 0")?;
/// let shadows = save_config_shadow(Path::new("gateway.toml"), document)?;
/// assert!(shadows.config.ends_with("gateway.toml.next"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn save_config_shadow(
    config_path: &Path,
    mut document: Value,
) -> Result<PendingShadows, crate::ConfigError> {
    crate::config::reject_profiles_directory(config_path).map_err(crate::ConfigError::from)?;
    let table = document.as_table().ok_or_else(|| {
        crate::ConfigError::validation("pending config must be a TOML table".to_owned())
    })?;
    if table.contains_key("active_profile") {
        return Err(crate::ConfigError::validation(
            "active_profile is not a configuration key; select a profile with \
             POST /admin/switch-profile"
                .to_owned(),
        ));
    }
    let current = read_pending_or_real(config_path)
        .map_err(crate::ConfigError::from)?
        .map(|(_, value)| value);
    restore_secrets(&mut document, current.as_ref()).map_err(crate::ConfigError::from)?;
    let rendered = toml::to_string_pretty(&document).map_err(|error| {
        crate::ConfigError::validation(format!("pending config does not render as TOML: {error}"))
    })?;
    Config::parse_toml_at(&rendered, Some(config_path)).map_err(crate::ConfigError::from)?;
    let config = write_shadow(config_path, &rendered)?;
    Ok(PendingShadows { config })
}

/// Loads the shadow-preferred config, resolving the profile the same way
/// [`Config::load`] does against the real state file.
///
/// Command-line and environment inputs outrank the state file and must name
/// a defined profile; a state file naming a profile the pending document
/// drops degrades to no profile, reported through
/// [`Config::stale_state_selection`].
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) under the same conditions as
/// [`Config::load`], or when a pending shadow cannot be read.
///
/// # Examples
/// ```no_run
/// use gateway_config::{ProfileSelection, load_pending_config};
/// use std::path::Path;
///
/// let config = load_pending_config(
///     Path::new("gateway.toml"),
///     &ProfileSelection::new(Some("work"), None),
/// )?;
/// assert_eq!(config.active_profile().map(|profile| profile.name()), Some("work"));
/// # Ok::<(), gateway_config::ConfigError>(())
/// ```
pub fn load_pending_config(
    config_path: &Path,
    inputs: &ProfileSelection,
) -> Result<Config, crate::ConfigError> {
    crate::config::reject_profiles_directory(config_path).map_err(crate::ConfigError::from)?;
    let (source_path, raw) = read_pending_or_real(config_path)
        .map_err(crate::ConfigError::from)?
        .ok_or_else(|| {
            crate::ConfigError::from(Repr::Read {
                path: config_path.to_owned(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "config not found"),
            })
        })?;
    let config = Config::parse_toml_at(
        &toml::to_string(&raw).map_err(|error| {
            crate::ConfigError::validation(format!("pending config does not render: {error}"))
        })?,
        Some(&source_path),
    )
    .map_err(crate::ConfigError::from)?;
    // The state file sits beside the real config, not the shadow.
    config
        .select_at_load(config_path, inputs)
        .map_err(crate::ConfigError::from)
}

/// Reports pending changes in the config shadow.
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) when a real file or shadow
/// cannot be read or parsed.
///
/// # Examples
/// ```no_run
/// use gateway_config::pending_report;
/// use std::path::Path;
///
/// let report = pending_report(Path::new("gateway.toml"))?;
/// assert!(report.changed_sections.windows(2).all(|pair| pair[0] <= pair[1]));
/// # Ok::<(), gateway_config::ConfigError>(())
/// ```
pub fn pending_report(config_path: &Path) -> Result<PendingReport, crate::ConfigError> {
    let real = read_toml(config_path).map_err(crate::ConfigError::from)?;
    let config_shadow_path = shadow_path(config_path);
    let pending = if config_shadow_path.is_file() {
        read_toml(&config_shadow_path).map_err(crate::ConfigError::from)?
    } else {
        real.clone()
    };
    let shadowed_files = if config_shadow_path.is_file() {
        vec![config_path.to_owned()]
    } else {
        Vec::new()
    };
    Ok(PendingReport {
        shadowed_files,
        changed_sections: changed_sections(&real, &pending),
    })
}

fn read_pending_or_real(path: &Path) -> Result<Option<(PathBuf, Value)>, Repr> {
    let shadow = shadow_path(path);
    if shadow.is_file() {
        return read_toml(&shadow).map(|value| Some((shadow, value)));
    }
    if path.is_file() {
        return read_toml(path).map(|value| Some((path.to_owned(), value)));
    }
    Ok(None)
}

fn read_toml(path: &Path) -> Result<Value, Repr> {
    let raw = fs::read_to_string(path).map_err(|source| Repr::Read {
        path: path.to_owned(),
        source,
    })?;
    toml::from_str(&raw).map_err(|source| Repr::Parse {
        path: Some(path.to_owned()),
        source: Box::new(source),
    })
}

fn changed_sections(real: &Value, pending: &Value) -> Vec<String> {
    let empty = toml::map::Map::new();
    let real = real.as_table().unwrap_or(&empty);
    let pending = pending.as_table().unwrap_or(&empty);
    let mut keys: Vec<String> = real
        .keys()
        .chain(pending.keys())
        .filter(|key| real.get(key.as_str()) != pending.get(key.as_str()))
        .cloned()
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}
