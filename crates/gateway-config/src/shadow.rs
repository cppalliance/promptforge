//! Single-file pending configuration and sibling profile-state shadows.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use toml::Value;

use self::content::restore_secrets;
use crate::config::Config;
use crate::error::ConfigError as Repr;
use crate::profile::{ProfileName, ProfileSelection, ProfileState};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

mod content;
pub use content::pending_var_references;

#[cfg(test)]
mod tests;

/// Paths staged by one pending configuration write.
///
/// # Examples
/// ```no_run
/// use gateway_config::save_config_shadow;
/// use std::path::Path;
///
/// let document = toml::from_str("config-version = 2")?;
/// let shadows = save_config_shadow(Path::new("gateway.toml"), document)?;
/// assert!(shadows.config.ends_with("gateway.toml.next"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingShadows {
    /// Shadow containing the global configuration and profile checklists.
    pub config: PathBuf,
    /// Sibling state shadow when the payload selected `active_profile`.
    pub state: Option<PathBuf>,
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
    /// Real config or state files that carry shadows.
    pub shadowed_files: Vec<PathBuf>,
    /// Changed top-level config keys plus `active_profile`, sorted.
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
/// let path = write_shadow(Path::new("gateway.toml"), "config-version = 2\n")?;
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
/// write_atomic(Path::new("gateway.toml"), "config-version = 2\n")?;
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

/// Persists the active profile without consuming a pending state shadow.
///
/// The real sibling state file is replaced through a unique temporary file,
/// so an immediate runtime switch can coexist with an unapplied Config UI
/// selection in `gateway.state.toml.next`.
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

/// Validates and stages one pending admin document.
///
/// The document contains the global version-2 config plus an optional
/// `active_profile` string. The latter is removed from `gateway.toml.next`
/// and written to `gateway.state.toml.next`, preserving the config/state
/// boundary while using the same pending key. Redacted secrets are restored
/// from the current pending config before validation.
///
/// # Errors
/// Returns [`ConfigError`](crate::ConfigError) when the document is malformed,
/// a secret cannot be restored, the config or selected profile is invalid, or
/// either shadow cannot be written.
///
/// # Examples
/// ```no_run
/// use gateway_config::save_config_shadow;
/// use std::path::Path;
///
/// let document = toml::from_str("config-version = 2")?;
/// let shadows = save_config_shadow(Path::new("gateway.toml"), document)?;
/// assert!(shadows.config.ends_with("gateway.toml.next"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn save_config_shadow(
    config_path: &Path,
    mut document: Value,
) -> Result<PendingShadows, crate::ConfigError> {
    crate::config::reject_profiles_directory(config_path).map_err(crate::ConfigError::from)?;
    let table = document.as_table_mut().ok_or_else(|| {
        crate::ConfigError::validation("pending config must be a TOML table".to_owned())
    })?;
    let active_profile = table.remove("active_profile");
    let current = read_pending_or_real(config_path)
        .map_err(crate::ConfigError::from)?
        .map(|(_, value)| value);
    restore_secrets(&mut document, current.as_ref()).map_err(crate::ConfigError::from)?;
    let rendered = toml::to_string_pretty(&document).map_err(|error| {
        crate::ConfigError::validation(format!("pending config does not render as TOML: {error}"))
    })?;
    let config =
        Config::parse_toml_at(&rendered, Some(config_path)).map_err(crate::ConfigError::from)?;

    let state = match active_profile {
        None => {
            if let Some(selected) =
                pending_selected_name(config_path, &ProfileSelection::default())?
            {
                config.select_profile(&selected)?;
            }
            None
        }
        Some(Value::String(name)) => {
            let name = ProfileName::parse(&name).map_err(|error| {
                crate::ConfigError::validation(format!(
                    "pending active_profile is invalid: {error}"
                ))
            })?;
            config.select_profile(&name)?;
            Some(ProfileState::new(&name))
        }
        Some(_) => {
            return Err(crate::ConfigError::validation(
                "pending active_profile must be a string".to_owned(),
            ));
        }
    };

    let previous_config_shadow = read_existing_shadow(config_path)?;
    let config_shadow = write_shadow(config_path, &rendered)?;
    let state_shadow = if let Some(state) = state {
        let state_path = crate::profile_state_path(config_path);
        match write_shadow(&state_path, &state.to_toml_string()?) {
            Ok(path) => Some(path),
            Err(error) => {
                restore_shadow(config_path, previous_config_shadow.as_deref())?;
                return Err(error);
            }
        }
    } else {
        None
    };
    Ok(PendingShadows {
        config: config_shadow,
        state: state_shadow,
    })
}

fn read_existing_shadow(target: &Path) -> Result<Option<String>, crate::ConfigError> {
    let path = shadow_path(target);
    match fs::read_to_string(&path) {
        Ok(contents) => Ok(Some(contents)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(crate::ConfigError::from(Repr::Read { path, source })),
    }
}

fn restore_shadow(target: &Path, contents: Option<&str>) -> Result<(), crate::ConfigError> {
    if let Some(contents) = contents {
        write_shadow(target, contents)?;
    } else {
        let path = shadow_path(target);
        if let Err(source) = fs::remove_file(&path)
            && source.kind() != std::io::ErrorKind::NotFound
        {
            return Err(crate::ConfigError::from(Repr::Write { path, source }));
        }
    }
    Ok(())
}

/// Loads the shadow-preferred config and state without any include resolution.
///
/// Command-line and environment inputs still outrank pending state.
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
    let selected = pending_selected_name(config_path, inputs)?;
    let Some(selected) = selected else {
        return Err(crate::ConfigError::validation(format!(
            "no active profile selected (defined profiles: {})",
            config.defined_profile_names()
        )));
    };
    config.select_profile(&selected)
}

fn pending_selected_name(
    config_path: &Path,
    inputs: &ProfileSelection,
) -> Result<Option<ProfileName>, crate::ConfigError> {
    if let Some(value) = inputs.command_line() {
        return ProfileName::parse(value)
            .map(Some)
            .map_err(|error| crate::ConfigError::validation(error.to_string()));
    }
    if let Some(value) = inputs.environment() {
        return ProfileName::parse(value)
            .map(Some)
            .map_err(|error| crate::ConfigError::validation(error.to_string()));
    }
    let state_path = crate::profile_state_path(config_path);
    let state_source = if shadow_path(&state_path).is_file() {
        shadow_path(&state_path)
    } else {
        state_path
    };
    let raw = match fs::read_to_string(&state_source) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(crate::ConfigError::from(Repr::Read {
                path: state_source,
                source,
            }));
        }
    };
    let state = ProfileState::from_toml_str(&raw)?;
    ProfileName::parse(state.active_profile())
        .map(Some)
        .map_err(|error| crate::ConfigError::validation(error.to_string()))
}

/// Reports pending changes across the config and sibling state shadows.
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
    let state_path = crate::profile_state_path(config_path);
    let state_shadow_path = shadow_path(&state_path);
    let mut shadowed_files = Vec::new();
    if config_shadow_path.is_file() {
        shadowed_files.push(config_path.to_owned());
    }
    if state_shadow_path.is_file() {
        shadowed_files.push(state_path.clone());
    }
    let mut changed_sections = changed_sections(&real, &pending);
    if state_shadow_path.is_file() {
        let real_state = fs::read_to_string(&state_path).ok();
        let pending_state = fs::read_to_string(&state_shadow_path).map_err(|source| {
            crate::ConfigError::from(Repr::Read {
                path: state_shadow_path,
                source,
            })
        })?;
        if real_state.as_deref() != Some(pending_state.as_str()) {
            changed_sections.push("active_profile".to_owned());
            changed_sections.sort_unstable();
        }
    }
    Ok(PendingReport {
        shadowed_files,
        changed_sections,
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
