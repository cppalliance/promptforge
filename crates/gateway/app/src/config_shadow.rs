//! Shadow-file bookkeeping: which real config files carry a pending
//! `.next` shadow, and how those paths are rendered for the wire.
//!
//! Three readers share this. `GET /admin/config-dirty` reports the census
//! as pending state, `POST /admin/config-apply` takes it under the apply
//! lock to decide what to promote, and the Apply command renders the same
//! file names into its outcome. The shadow mechanics themselves live in
//! `gateway-config`; this module only assembles the census and puts its
//! paths in comparable and displayable form.

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
