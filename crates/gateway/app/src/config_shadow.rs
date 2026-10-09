//! Shadow-file bookkeeping: which real config files have a pending
//! `.next` shadow, and how those paths are rendered for the wire.
//! Provides a struct for reading the pending environment from the shadow files.
//!
//! Three readers share this. `GET /admin/config-dirty` reports the census
//! as pending state, `POST /admin/config-apply` takes it under the apply
//! lock to decide what to promote, and the Apply command renders the same
//! file names into its outcome. The shadow mechanics themselves sit in
//! `gateway-config`; this module only assembles the census and puts its
//! paths in comparable and displayable form.

use std::collections::BTreeMap;
use std::env::VarError;
use std::path::{Path, PathBuf};

use gateway_config::{pending_report, shadow_path};

use crate::error::{GatewayError, pending_read_error};

/// Every shadow on disk for one gateway and the sections they change.
pub(crate) struct ShadowCensus {
    /// Real files whose shadows exist, in canonical form, without
    /// duplicates.
    pub(crate) files: Vec<PathBuf>,
    /// Top-level sections whose merged value the shadows change, sorted
    /// and deduplicated.
    pub(crate) sections: Vec<String>,
}

/// Collects the config shadow and one env shadow.
pub(crate) fn shadow_census(config_path: &Path) -> Result<ShadowCensus, GatewayError> {
    let profile = pending_report(config_path).map_err(|error| pending_read_error(&error))?;
    let mut files: Vec<PathBuf> = Vec::new();
    for file in &profile.shadowed_files {
        push_unique(&mut files, file);
    }
    let sections = profile.changed_sections;
    let env = config_path.with_extension("env");
    if shadow_path(&env).is_file() {
        push_unique(&mut files, &env);
    }
    Ok(ShadowCensus { files, sections })
}

/// The directory config files render relative to.
pub(crate) fn config_root(config_path: &Path) -> Option<&Path> {
    config_path.parent()
}

/// Appends `file` unless its canonical form is already listed. The same
/// file reaches here under different spellings (the profile chain writes
/// `profiles/../gateway.toml`, the boot path is `gateway.toml`), so the
/// list holds canonical forms.
fn push_unique(shadowed: &mut Vec<PathBuf>, file: &Path) {
    let canonical = canonical_form(file);
    if !shadowed.contains(&canonical) {
        shadowed.push(canonical);
    }
}

/// A comparable form of `path`: canonicalized when it exists, otherwise
/// its canonicalized parent plus its own name (a real `.env` may not
/// exist while its shadow does), otherwise the path as given.
pub(crate) fn canonical_form(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(parent) = parent.canonicalize()
    {
        return parent.join(name);
    }
    path.to_path_buf()
}

/// Renders one shadowed real file for the wire: relative to `root` when
/// it sits beneath it, the full path otherwise, always with forward
/// slashes for a stable shape across platforms.
pub(crate) fn relative_name(file: &Path, root: Option<&Path>) -> String {
    let file = canonical_form(file);
    let relative = root
        .map(canonical_form)
        .and_then(|root| file.strip_prefix(&root).ok().map(Path::to_path_buf))
        .unwrap_or(file);
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// Struct for accessing the environment variables the next boot will run with.
pub(crate) struct PendingEnv {
    data: BTreeMap<String, String>,
}

impl PendingEnv {
    /// Constructs `Self` from env files.
    /// If a shadow env file is staged, it reads that, otherwise the real file.
    pub(crate) fn new(config_path: &Path) -> Result<Self, GatewayError> {
        let path = {
            let env_path = config_path.with_extension("env");
            let shadow_path = shadow_path(&env_path);

            if shadow_path.is_file() {
                shadow_path
            } else {
                env_path
            }
        };

        Ok(Self {
            data: read_env_file(&path)?,
        })
    }

    /// Resolves a `${VAR}` from the pending environment.
    /// If a value is not present in the pending environment, it checks the current
    /// running environment.
    ///
    /// TODO: This function is not able to distinguish between variables set in an
    /// env file and overrides set in the shell. If the pending environment defines a
    /// variable `VAR=abc` but the shell environment overrides it with `VAR=def`,
    /// this will return `abc` but when the gateway restarts the value will be `def`
    pub(crate) fn resolve_var(&self, name: &str) -> Result<String, VarError> {
        match self.data.get(name) {
            Some(value) => Ok(value.clone()),
            None => std::env::var(name),
        }
    }
}

/// Parses a dotenv file and returns a map without changing the process environment; a
/// missing file is an empty map.
pub(crate) fn read_env_file(path: &Path) -> Result<BTreeMap<String, String>, GatewayError> {
    let mut vars = BTreeMap::new();

    if !path.is_file() {
        return Ok(vars);
    }

    let entries =
        dotenvy::from_path_iter(path).map_err(|error| GatewayError::EnvFile(Box::new(error)))?;

    for entry in entries {
        let (key, value) = entry.map_err(|error| GatewayError::EnvFile(Box::new(error)))?;
        vars.insert(key, value);
    }

    Ok(vars)
}

#[cfg(test)]
#[path = "config_shadow-tests.rs"]
mod tests;
