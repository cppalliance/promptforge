//! Profile identifiers, startup selection inputs, and sibling state files.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

mod name;

pub use name::{ProfileName, ProfileNameError};

/// Startup profile choices supplied at the configuration boundary.
///
/// Command-line input outranks environment input. When both are absent,
/// [`Config::load`](crate::Config::load) reads the sibling state file.
///
/// # Examples
/// ```
/// use gateway_config::ProfileSelection;
///
/// let selection = ProfileSelection::new(Some("work"), Some("travel"));
/// assert_eq!(selection.command_line(), Some("work"));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProfileSelection {
    command_line: Option<String>,
    environment: Option<String>,
}

impl ProfileSelection {
    /// Builds startup selection inputs in precedence order.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::ProfileSelection;
    ///
    /// let selection = ProfileSelection::new(Some("work"), None);
    /// assert_eq!(selection.command_line(), Some("work"));
    /// ```
    #[must_use]
    pub fn new(command_line: Option<&str>, environment: Option<&str>) -> ProfileSelection {
        ProfileSelection {
            command_line: command_line.map(str::to_owned),
            environment: environment.map(str::to_owned),
        }
    }

    /// Returns the command-line profile value, when supplied.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::ProfileSelection;
    ///
    /// assert_eq!(
    ///     ProfileSelection::new(Some("work"), None).command_line(),
    ///     Some("work")
    /// );
    /// ```
    #[must_use]
    pub fn command_line(&self) -> Option<&str> {
        self.command_line.as_deref()
    }

    /// Returns the `PROMPTFORGE_PROFILE` value, when supplied.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::ProfileSelection;
    ///
    /// assert_eq!(
    ///     ProfileSelection::new(None, Some("travel")).environment(),
    ///     Some("travel")
    /// );
    /// ```
    #[must_use]
    pub fn environment(&self) -> Option<&str> {
        self.environment.as_deref()
    }
}

/// Persisted startup state stored beside `gateway.toml`.
///
/// # Examples
/// ```
/// use gateway_config::{ProfileName, ProfileState};
///
/// let name = ProfileName::parse("work")?;
/// assert_eq!(ProfileState::new(&name).active_profile(), "work");
/// # Ok::<(), gateway_config::ProfileNameError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ProfileState {
    active_profile: String,
}

impl ProfileState {
    /// Builds state for a validated profile name.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::{ProfileName, ProfileState};
    ///
    /// let state = ProfileState::new(&ProfileName::parse("work")?);
    /// assert_eq!(state.active_profile(), "work");
    /// # Ok::<(), gateway_config::ProfileNameError>(())
    /// ```
    #[must_use]
    pub fn new(active_profile: &ProfileName) -> ProfileState {
        ProfileState {
            active_profile: active_profile.as_str().to_owned(),
        }
    }

    /// Parses the canonical sibling-state TOML shape.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) when TOML is malformed,
    /// contains an unknown key, or `active_profile` is not a legal profile
    /// identifier.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::ProfileState;
    ///
    /// let state = ProfileState::from_toml_str("active_profile = \"work\"\n")?;
    /// assert_eq!(state.active_profile(), "work");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    pub fn from_toml_str(raw: &str) -> Result<ProfileState, crate::ConfigError> {
        parse_state(raw, None).map_err(crate::ConfigError::from)
    }

    /// Returns the persisted active profile.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::ProfileState;
    ///
    /// let state = ProfileState::from_toml_str("active_profile = \"work\"\n")?;
    /// assert_eq!(state.active_profile(), "work");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn active_profile(&self) -> &str {
        &self.active_profile
    }

    /// Renders exactly one canonical `active_profile` key.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) if the state cannot be
    /// represented as TOML.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::ProfileState;
    ///
    /// let state = ProfileState::from_toml_str("active_profile = \"work\"\n")?;
    /// assert_eq!(state.to_toml_string()?, "active_profile = \"work\"\n");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    pub fn to_toml_string(&self) -> Result<String, crate::ConfigError> {
        toml::to_string(self)
            .map_err(|error| {
                ConfigError::Validation(format!("profile state does not render as TOML: {error}"))
            })
            .map_err(crate::ConfigError::from)
    }
}

/// Returns the canonical sibling state path for a configuration file.
///
/// `gateway.toml` maps to `gateway.state.toml`.
///
/// # Examples
/// ```
/// use gateway_config::profile_state_path;
/// use std::path::Path;
///
/// assert_eq!(
///     profile_state_path(Path::new("gateway.toml")),
///     Path::new("gateway.state.toml")
/// );
/// ```
#[must_use]
pub fn profile_state_path(config_path: &Path) -> PathBuf {
    let stem = config_path.file_stem().map_or_else(
        || std::ffi::OsString::from("gateway"),
        std::ffi::OsStr::to_os_string,
    );
    let mut name = stem;
    name.push(".state.toml");
    config_path.with_file_name(name)
}

pub(crate) fn resolve_selection(
    config_path: &Path,
    inputs: &ProfileSelection,
) -> Result<Option<ProfileName>, ConfigError> {
    if let Some(value) = inputs.command_line() {
        return parse_selected_name(value, "command-line --profile").map(Some);
    }
    if let Some(value) = inputs.environment() {
        return parse_selected_name(value, "PROMPTFORGE_PROFILE").map(Some);
    }
    let state_path = profile_state_path(config_path);
    let Some(state) = load_state(&state_path)? else {
        return Ok(None);
    };
    parse_selected_name(state.active_profile(), &state_path.display().to_string()).map(Some)
}

fn parse_selected_name(value: &str, source: &str) -> Result<ProfileName, ConfigError> {
    ProfileName::parse(value).map_err(|error| {
        ConfigError::Validation(format!(
            "{source} selects invalid profile {value:?}: {error}"
        ))
    })
}

fn load_state(path: &Path) -> Result<Option<ProfileState>, ConfigError> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    parse_state(&raw, Some(path.to_owned())).map(Some)
}

fn parse_state(raw: &str, path: Option<PathBuf>) -> Result<ProfileState, ConfigError> {
    let state: ProfileState = toml::from_str(raw).map_err(|source| ConfigError::Parse {
        path,
        source: Box::new(source),
    })?;
    ProfileName::parse(&state.active_profile).map_err(|error| {
        ConfigError::Validation(format!(
            "state active_profile {:?} is invalid: {error}",
            state.active_profile
        ))
    })?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_path_replaces_the_config_extension() {
        assert_eq!(
            profile_state_path(Path::new("config/gateway.toml")),
            PathBuf::from("config/gateway.state.toml")
        );
    }

    #[test]
    fn state_shape_is_the_canonical_pending_key() {
        let name = ProfileName::parse("work").expect("valid profile");
        let state = ProfileState::new(&name);
        assert_eq!(
            state.to_toml_string().expect("state renders"),
            "active_profile = \"work\"\n"
        );
        assert_eq!(
            ProfileState::from_toml_str("active_profile = \"work\"\n")
                .expect("state parses")
                .active_profile(),
            "work"
        );
    }
}
