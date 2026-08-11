//! Named profiles: recursive `include` resolution and the profiles directory.
//!
//! A profile is a TOML file under `~/.promptforge/profiles/` (or a path passed
//! to `serve`). Includes resolve depth-first relative to the including file;
//! later definitions replace earlier ones with the same `id` or `name`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use toml::Value;

use crate::config::Config;
use crate::error::ConfigError;
use crate::local::artifacts::default_promptforge_root;

/// Maximum `include` nesting depth (guards against runaway trees).
pub(crate) const MAX_INCLUDE_DEPTH: usize = 16;

// Include boundary policy (PROFILE-009).
//
// Profile files are operator-authored, trusted inputs. `include` paths resolve
// relative to the including file (absolute paths are permitted) and may reach a
// shared parent (for example `../common.toml`); this is deliberate so operators
// can factor shared configuration. The two guarded, attacker-relevant surfaces
// are enforced elsewhere: runtime profile *selection* is confined to a single
// path component by [`ProfileName`], and include recursion is bounded by
// [`MAX_INCLUDE_DEPTH`] and cycle detection. There is intentionally no
// additional filesystem confinement on include targets themselves.

mod merge;
mod name;

use merge::merge_docs;
pub use name::{ProfileName, ProfileNameError};

#[cfg(test)]
mod tests;

/// Default directory for named profiles: `~/.promptforge/profiles`.
///
/// # Examples
/// ```
/// let dir = promptforge_gateway::default_profiles_dir();
/// assert!(dir.ends_with("profiles"));
/// ```
#[must_use]
pub fn default_profiles_dir() -> PathBuf {
    default_promptforge_root().join("profiles")
}

/// Lists profile names (`*.toml` stems) in `dir`, sorted.
///
/// Missing directories yield an empty list. Non-directory paths yield
/// [`ConfigError::Validation`].
///
/// # Errors
/// Returns [`ConfigError::Read`] when the directory cannot be read, or
/// [`ConfigError::Validation`] when `dir` exists but is not a directory.
pub(crate) fn list_profiles(dir: &Path) -> Result<Vec<String>, ConfigError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    if !dir.is_dir() {
        return Err(ConfigError::Validation(format!(
            "profiles directory is not a directory: {}",
            dir.display()
        )));
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).map_err(|source| ConfigError::Read {
        path: dir.to_owned(),
        source,
    })? {
        let entry = entry.map_err(|source| ConfigError::Read {
            path: dir.to_owned(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        // Only regular files are profiles; skip directories named `*.toml`.
        if !path.is_file() {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            names.push(stem.to_owned());
        }
    }
    names.sort_unstable();
    Ok(names)
}

/// Loads `dir/<name>.toml` with recursive include resolution.
///
/// Takes a validated [`ProfileName`] rather than a raw string: confinement to a
/// single normal path component is already guaranteed by the type, so this
/// function does no divergent re-validation. In particular an ordinary name
/// like `analysis..v2` (a single component that merely contains `..`) loads
/// correctly, where the previous substring check wrongly rejected it
/// (PROFILE-010).
///
/// # Errors
/// Returns [`ConfigError`] when the file is missing, includes cycle or exceed
/// depth, merge fails, or the resolved document fails config validation.
pub(crate) fn load_named(dir: &Path, name: &ProfileName) -> Result<Config, ConfigError> {
    let path = dir.join(format!("{}.toml", name.as_str()));
    load_path(&path)
}

/// Loads a profile TOML path with recursive include resolution.
///
/// # Errors
/// Returns [`ConfigError`] on read, include, parse, or validation failure.
pub(crate) fn load_path(path: &Path) -> Result<Config, ConfigError> {
    let mut stack = Vec::new();
    let mut visiting = HashSet::new();
    let value = load_value(path, 0, &mut stack, &mut visiting)?;
    // Interpolate and deserialize the merged include tree directly from the
    // `toml::Value`, avoiding a re-serialize round-trip (and its ser error).
    Config::from_value(value)
}

fn load_value(
    path: &Path,
    depth: usize,
    stack: &mut Vec<PathBuf>,
    visiting: &mut HashSet<PathBuf>,
) -> Result<Value, ConfigError> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(ConfigError::IncludeDepth {
            path: path.to_owned(),
            max: MAX_INCLUDE_DEPTH,
        });
    }

    let canonical = canonicalize_for_cycle(path);
    if !visiting.insert(canonical.clone()) {
        return Err(ConfigError::IncludeCycle {
            path: canonical,
            chain: stack.clone(),
        });
    }
    stack.push(canonical.clone());

    let raw = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_owned(),
        source,
    })?;
    let mut doc: Value = toml::from_str(&raw).map_err(|source| ConfigError::Parse {
        path: Some(path.to_owned()),
        source: Box::new(source),
    })?;

    let includes = take_includes(&mut doc)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

    let mut merged = Value::Table(toml::map::Map::new());
    for include_name in &includes {
        let include_path = resolve_include(base_dir, include_name)?;
        let parent_doc = load_value(&include_path, depth + 1, stack, visiting)?;
        merge_docs(&mut merged, parent_doc, &include_path)?;
    }
    merge_docs(&mut merged, doc, path)?;

    stack.pop();
    visiting.remove(&canonical);
    Ok(merged)
}

fn canonicalize_for_cycle(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn take_includes(doc: &mut Value) -> Result<Vec<String>, ConfigError> {
    let Some(table) = doc.as_table_mut() else {
        return Ok(Vec::new());
    };
    let Some(include_val) = table.remove("include") else {
        return Ok(Vec::new());
    };
    let Value::Array(items) = include_val else {
        return Err(ConfigError::Validation(
            "include must be an array of strings".to_string(),
        ));
    };
    let mut names = Vec::with_capacity(items.len());
    for item in items {
        let Some(name) = item.as_str() else {
            return Err(ConfigError::Validation(
                "include entries must be strings".to_string(),
            ));
        };
        names.push(name.to_owned());
    }
    Ok(names)
}

fn resolve_include(base_dir: &Path, name: &str) -> Result<PathBuf, ConfigError> {
    let path = Path::new(name);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    };
    if !resolved.exists() {
        return Err(ConfigError::Read {
            path: resolved.clone(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "include not found"),
        });
    }
    Ok(resolved)
}
