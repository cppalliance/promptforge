//! Named profiles and boot-file inspection: recursive `include` resolution.
//!
//! A profile is a TOML file in a profiles directory chosen by the caller (the
//! gateway uses the boot config's sibling `profiles/` directory). Includes
//! resolve depth-first relative to the including file; later definitions
//! replace earlier ones with the same `id` or `name`.
//!
//! This crate does no env-file loading: `${VAR}` interpolation reads the
//! process environment, and populating that environment (for example from
//! `<profile>.env` and `<boot>.env` files) is the calling binary's job.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use toml::Value;

use crate::config::{Config, ServerConfig, WorkshopConfig, interpolate_value};
use crate::error::ConfigError;

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
mod provenance;
mod shadow;

use merge::merge_docs;
pub use name::{ProfileName, ProfileNameError};
pub(crate) use provenance::Provenance;
pub use shadow::{
    PendingReport, load_pending_profile, pending_report, pending_var_references, promote_shadow,
    save_boot_shadow, save_include_shadow, save_profile_shadow, shadow_path, write_shadow,
};

#[cfg(test)]
mod tests;

/// Lists profile names (`*.toml` stems) in `dir`, sorted.
///
/// Missing directories yield an empty list. Non-directory paths yield
/// a validation-classified error.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) of kind
/// [`ConfigErrorKind::Read`](crate::ConfigErrorKind::Read) when the directory
/// cannot be read, or [`ConfigErrorKind::Validation`](crate::ConfigErrorKind::Validation)
/// when `dir` exists but is not a directory.
pub fn list_profiles(dir: &Path) -> Result<Vec<String>, crate::api_error::ConfigError> {
    list_profiles_repr(dir).map_err(crate::api_error::ConfigError::from)
}

/// The crate-internal form of [`list_profiles`], returning the private
/// representation.
fn list_profiles_repr(dir: &Path) -> Result<Vec<String>, ConfigError> {
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
/// Returns a [`ConfigError`] describing the exact failure:
/// - [`ConfigError::Read`] when `dir/<name>.toml` (or an included file) cannot
///   be read.
/// - [`ConfigError::Parse`] when a profile file is not valid TOML.
/// - [`ConfigError::IncludeCycle`] when the `include` graph revisits a file.
/// - [`ConfigError::IncludeDepth`] when `include` nesting exceeds
///   [`MAX_INCLUDE_DEPTH`].
/// - [`ConfigError::Validation`] when a merge is ill-typed or the resolved
///   document fails config validation.
pub(crate) fn load_named(dir: &Path, name: &ProfileName) -> Result<Config, ConfigError> {
    load_named_with_chain(dir, name).map(|(config, _chain)| config)
}

/// Loads `dir/<name>.toml` like [`load_named`], additionally returning the
/// resolved include chain (root first, depth-first) so the caller can log it
/// and check whether another file (for example the boot config) appears in it.
pub(crate) fn load_named_with_chain(
    dir: &Path,
    name: &ProfileName,
) -> Result<(Config, Vec<PathBuf>), ConfigError> {
    let path = dir.join(format!("{}.toml", name.as_str()));
    let resolved = collect_config_chain(&path)?;
    let mut config = Config::from_value(resolved.value)?;
    config.set_provenance(resolved.provenance);
    config.set_include(resolved.root_include);
    Ok((config, resolved.chain))
}

/// Loads only the `[server]` section of a config file: includes are resolved
/// and `${VAR}` references interpolated, but full validation is skipped.
///
/// The boot file is the catalog and may legitimately fail checks that apply
/// to a loaded profile, so callers enforcing the boot-owned `[server]` rule
/// need the section without running that validation.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) when the file (or an
/// included file) cannot be read or parsed, an include cycles or exceeds
/// depth, an interpolation fails, the `[server]` section is absent, or the
/// section itself does not deserialize (for example a malformed `bind`).
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::load_server;
/// use std::path::Path;
///
/// let server = load_server(Path::new("gateway.toml"))?;
/// println!("boot file binds {}", server.bind());
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn load_server(path: &Path) -> Result<ServerConfig, crate::api_error::ConfigError> {
    load_server_repr(path).map_err(crate::api_error::ConfigError::from)
}

/// The crate-internal form of [`load_server`], returning the private
/// representation.
fn load_server_repr(path: &Path) -> Result<ServerConfig, ConfigError> {
    let mut value = collect_config_chain(path)?.value;
    interpolate_value(&mut value)?;
    server_section(&value, path)
}

/// Loads only the `[workshop]` section of a config file: includes are
/// resolved and `${VAR}` references interpolated, but full validation is
/// skipped. Returns `None` when the section is absent, which is the common
/// case for a headless gateway.
///
/// Like [`load_server`], this serves callers enforcing a boot-owned section
/// rule: the boot file is the catalog and may legitimately fail checks that
/// apply to a loaded profile.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) when the file (or an
/// included file) cannot be read or parsed, an include cycles or exceeds
/// depth, an interpolation fails, or a present `[workshop]` section does not
/// deserialize (for example a malformed `bind` or an unknown field).
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::load_workshop;
/// use std::path::Path;
///
/// if let Some(workshop) = load_workshop(Path::new("gateway.toml"))? {
///     println!("workshop binds {}", workshop.bind());
/// }
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn load_workshop(path: &Path) -> Result<Option<WorkshopConfig>, crate::api_error::ConfigError> {
    load_workshop_repr(path).map_err(crate::api_error::ConfigError::from)
}

/// The crate-internal form of [`load_workshop`], returning the private
/// representation.
fn load_workshop_repr(path: &Path) -> Result<Option<WorkshopConfig>, ConfigError> {
    let mut value = collect_config_chain(path)?.value;
    interpolate_value(&mut value)?;
    workshop_section(&value, path)
}

/// Loads both boot-owned sections of a config file in one include-resolution
/// and interpolation pass: [`load_server`] and [`load_workshop`] combined.
///
/// A caller that needs both sections (the gateway's startup path) avoids
/// parsing the same include tree twice. As with the single-section loaders,
/// full validation is skipped: the boot file is the catalog and may
/// legitimately fail checks that apply to a loaded profile.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) under the same conditions
/// as [`load_server`]: the file (or an included file) cannot be read or
/// parsed, an include cycles or exceeds depth, an interpolation fails, the
/// `[server]` section is absent, or a present section does not deserialize.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::load_boot_sections;
/// use std::path::Path;
///
/// let (server, workshop) = load_boot_sections(Path::new("gateway.toml"))?;
/// println!("boot file binds {}", server.bind());
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn load_boot_sections(
    path: &Path,
) -> Result<(ServerConfig, Option<WorkshopConfig>), crate::api_error::ConfigError> {
    load_boot_sections_repr(path).map_err(crate::api_error::ConfigError::from)
}

/// The crate-internal form of [`load_boot_sections`], returning the private
/// representation.
fn load_boot_sections_repr(
    path: &Path,
) -> Result<(ServerConfig, Option<WorkshopConfig>), ConfigError> {
    let mut value = collect_config_chain(path)?.value;
    interpolate_value(&mut value)?;
    Ok((
        server_section(&value, path)?,
        workshop_section(&value, path)?,
    ))
}

/// Extracts the required `[server]` section from an interpolated document.
fn server_section(value: &Value, path: &Path) -> Result<ServerConfig, ConfigError> {
    let server = value
        .as_table()
        .and_then(|table| table.get("server"))
        .cloned()
        .ok_or_else(|| {
            ConfigError::Validation(format!("no [server] section in {}", path.display()))
        })?;
    server.try_into().map_err(|source| ConfigError::Parse {
        path: Some(path.to_owned()),
        source: Box::new(source),
    })
}

/// Extracts the optional `[workshop]` section from an interpolated document.
fn workshop_section(value: &Value, path: &Path) -> Result<Option<WorkshopConfig>, ConfigError> {
    let Some(workshop) = value
        .as_table()
        .and_then(|table| table.get("workshop"))
        .cloned()
    else {
        return Ok(None);
    };
    workshop
        .try_into()
        .map(Some)
        .map_err(|source| ConfigError::Parse {
            path: Some(path.to_owned()),
            source: Box::new(source),
        })
}

/// Loads a config TOML path with recursive include resolution.
///
/// `${VAR}` interpolation reads the process environment as the caller left
/// it; this crate never populates it from env files.
///
/// # Errors
/// Returns [`ConfigError`] on read, include, parse, or validation failure.
pub(crate) fn load_path(path: &Path) -> Result<Config, ConfigError> {
    let resolved = collect_config_chain(path)?;
    let mut config = Config::from_value(resolved.value)?;
    config.set_provenance(resolved.provenance);
    config.set_include(resolved.root_include);
    Ok(config)
}

/// The product of one include-chain resolution.
pub(crate) struct ChainResolution {
    /// The merged TOML document.
    pub(crate) value: Value,
    /// The visited file paths in include-chain order (root first,
    /// depth-first), letting the caller log the resolved chain and check
    /// whether another file (for example the boot config) appears in it.
    pub(crate) chain: Vec<PathBuf>,
    /// Which file produced each merged entry, recorded without changing
    /// the merge itself.
    pub(crate) provenance: Provenance,
    /// The root file's own `include` array, verbatim and ordered as
    /// written (empty when the root has none). The merge consumes the
    /// `include` keys, so this is the only place the leaf's chain
    /// declaration survives to be serialized back to a reader.
    pub(crate) root_include: Vec<String>,
}

/// Collects config file paths in include-chain order (root first, depth-first)
/// alongside the merged TOML value, the merge's provenance record, and the
/// root file's own `include` array.
///
/// # Errors
/// Returns [`ConfigError`] on read, include, parse, or validation failure.
pub(crate) fn collect_config_chain(path: &Path) -> Result<ChainResolution, ConfigError> {
    collect_config_chain_with(path, &read_doc_from_disk)
}

/// [`collect_config_chain`] over a caller-chosen document reader.
///
/// The reader maps each chain path to its parsed TOML document; the disk
/// reader is the normal case, and the shadow module substitutes pending
/// shadow files (and an in-memory candidate) without the chain walker
/// knowing. Include resolution, cycle detection, and provenance always use
/// the real paths.
pub(crate) fn collect_config_chain_with(
    path: &Path,
    read_doc: &dyn Fn(&Path) -> Result<Value, ConfigError>,
) -> Result<ChainResolution, ConfigError> {
    let mut stack = Vec::new();
    let mut visiting = HashSet::new();
    let mut config_chain = Vec::new();
    let mut provenance = Provenance::default();
    let (value, root_include) = load_value(
        path,
        0,
        &mut stack,
        &mut visiting,
        &mut config_chain,
        &mut provenance,
        read_doc,
    )?;
    Ok(ChainResolution {
        value,
        chain: config_chain,
        provenance,
        root_include,
    })
}

/// Reads and parses one TOML document from disk: the default chain reader.
pub(crate) fn read_doc_from_disk(path: &Path) -> Result<Value, ConfigError> {
    let raw = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_owned(),
        source,
    })?;
    toml::from_str(&raw).map_err(|source| ConfigError::Parse {
        path: Some(path.to_owned()),
        source: Box::new(source),
    })
}

/// Resolves one file's include tree; returns the merged document and the
/// file's own `include` array (the caller keeps the root's, discards the
/// rest).
fn load_value(
    path: &Path,
    depth: usize,
    stack: &mut Vec<PathBuf>,
    visiting: &mut HashSet<PathBuf>,
    config_chain: &mut Vec<PathBuf>,
    provenance: &mut Provenance,
    read_doc: &dyn Fn(&Path) -> Result<Value, ConfigError>,
) -> Result<(Value, Vec<String>), ConfigError> {
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
    config_chain.push(path.to_path_buf());

    let mut doc = read_doc(path)?;

    let includes = take_includes(&mut doc)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

    let mut merged = Value::Table(toml::map::Map::new());
    for include_name in &includes {
        let include_path = resolve_include(base_dir, include_name)?;
        let (parent_doc, _parent_includes) = load_value(
            &include_path,
            depth + 1,
            stack,
            visiting,
            config_chain,
            provenance,
            read_doc,
        )?;
        merge_docs(&mut merged, parent_doc, &include_path, provenance)?;
    }
    merge_docs(&mut merged, doc, path, provenance)?;

    stack.pop();
    visiting.remove(&canonical);
    Ok((merged, includes))
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
