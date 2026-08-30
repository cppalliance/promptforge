//! Shadow files: pending configuration writes staged beside the real files.
//!
//! Every managed config file can carry a sibling shadow holding pending
//! changes: `gateway.toml` gains `gateway.toml.next`, `default.env` gains
//! `default.env.next`. The suffix appends to the whole file name (never
//! `.next.toml`), so the profile listing's `*.toml` match can never see a
//! shadow as a phantom profile.
//!
//! A save validates before any byte lands: the include chain is resolved
//! preferring shadows, the candidate document is substituted for its target
//! file, and the merged result passes the same validation as a real load.
//! Only after validation does the shadow get written (temp file + rename),
//! so a failed save never leaves a bad shadow behind - at most the previous
//! valid shadow remains. Real files are never touched here; promotion is a
//! separate, explicit apply step.
//!
//! Secrets round-trip without ever crossing the wire: a candidate string
//! leaf equal to `"***"` (the redaction marker `Config::to_json` emits)
//! is replaced with the existing value from the current pending state
//! before validation and write.
//!
//! The include chain round-trips explicitly: `Config::to_json` emits the
//! loaded root's `include` array, so a round-tripped body carries it and
//! the save keeps it verbatim. As a safety net for a candidate that
//! carries no `include` key (an older or partial client), the target
//! file's current one (existing shadow first, then the real file) is
//! grafted on before validation and write. A save can therefore never
//! sever the chain by accident; only a candidate that spells out
//! `include` (even an empty array) changes it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use toml::Value;

use crate::config::{Config, interpolate_value};
use crate::error::ConfigError as Repr;

use super::{
    collect_config_chain, collect_config_chain_with, read_doc_from_disk, server_section,
    workshop_section,
};

/// The redaction marker serialized in place of every secret; on write it
/// means "preserve the existing value".
const REDACTED: &str = "***";

/// The shadow path for a managed file: the same name with `.next` appended.
///
/// `gateway.toml` maps to `gateway.toml.next` and `default.env` to
/// `default.env.next`. The suffix goes after the real extension so the
/// profile listing's `*.toml` match can never pick up a shadow.
///
/// # Examples
/// ```
/// use promptforge_gateway_config::shadow_path;
/// use std::path::Path;
///
/// let shadow = shadow_path(Path::new("profiles/default.toml"));
/// assert_eq!(shadow, Path::new("profiles/default.toml.next"));
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

/// Writes `contents` to `target`'s shadow file atomically and returns the
/// shadow path.
///
/// The bytes go to a temporary sibling first and reach the shadow path only
/// through a rename, so a reader never observes a half-written shadow. An
/// existing shadow is replaced. The real `target` file is never touched.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) of kind
/// [`ConfigErrorKind::Write`](crate::ConfigErrorKind::Write) when the
/// temporary file cannot be written or the rename fails.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::write_shadow;
/// use std::path::Path;
///
/// let shadow = write_shadow(Path::new("profiles/default.env"), "HF_TOKEN=hf_x\n")?;
/// assert!(shadow.ends_with("default.env.next"));
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn write_shadow(target: &Path, contents: &str) -> Result<PathBuf, crate::ConfigError> {
    write_shadow_repr(target, contents).map_err(crate::ConfigError::from)
}

/// Counter distinguishing temp names of concurrent writers in one process.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The crate-internal form of [`write_shadow`].
fn write_shadow_repr(target: &Path, contents: &str) -> Result<PathBuf, Repr> {
    let shadow = shadow_path(target);
    let mut temp_name = shadow.file_name().map_or_else(
        || std::ffi::OsString::from("shadow"),
        std::ffi::OsStr::to_os_string,
    );
    temp_name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = shadow.with_file_name(temp_name);
    if let Err(source) = fs::write(&temp, contents) {
        // A partial write must not leave litter; removal is best-effort.
        let _ = fs::remove_file(&temp);
        return Err(Repr::Write { path: temp, source });
    }
    if let Err(first) = fs::rename(&temp, &shadow) {
        // The rename can fail when the destination exists but cannot be
        // replaced in place (locked, read-only, or a directory - Windows
        // sharing violations are the common case); replace in two steps.
        // Either state a reader can observe is a complete file.
        let _ = fs::remove_file(&shadow);
        if let Err(source) = fs::rename(&temp, &shadow) {
            let _ = fs::remove_file(&temp);
            drop(first);
            return Err(Repr::Write {
                path: shadow,
                source,
            });
        }
    }
    Ok(shadow)
}

/// Promotes `target`'s shadow to the real file by one atomic rename.
///
/// The shadow becomes the real file and disappears in the same rename, so
/// a reader observes either the old or the new content, never a mix. When
/// the destination cannot be replaced in place (locked or read-only -
/// Windows sharing violations are the common case) the promotion falls
/// back to remove-then-rename; either state a reader can observe is a
/// complete file.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) of kind
/// [`ConfigErrorKind::Write`](crate::ConfigErrorKind::Write) when `target`
/// has no shadow to promote or the rename fails.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::{promote_shadow, shadow_path};
/// use std::path::Path;
///
/// let target = Path::new("profiles/default.toml");
/// promote_shadow(target)?;
/// assert!(!shadow_path(target).exists());
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn promote_shadow(target: &Path) -> Result<(), crate::ConfigError> {
    promote_shadow_repr(target).map_err(crate::ConfigError::from)
}

/// The crate-internal form of [`promote_shadow`].
fn promote_shadow_repr(target: &Path) -> Result<(), Repr> {
    let shadow = shadow_path(target);
    if !shadow.is_file() {
        return Err(Repr::Write {
            path: shadow,
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no shadow to promote"),
        });
    }
    if fs::rename(&shadow, target).is_err() {
        // The rename can fail when the destination exists but cannot be
        // replaced in place (locked or read-only - Windows sharing
        // violations are the common case); replace in two steps. Either
        // state a reader can observe is a complete file.
        let _ = fs::remove_file(target);
        if let Err(source) = fs::rename(&shadow, target) {
            return Err(Repr::Write {
                path: target.to_path_buf(),
                source,
            });
        }
    }
    Ok(())
}

/// Saves a candidate document as the active profile leaf's shadow.
///
/// `document` is the full config in the TOML shape (what
/// `GET /admin/config` returns, secrets as `"***"`). Redacted secrets are
/// restored from the current pending state - the include chain resolved
/// with existing shadows preferred - so the winning definition's secret is
/// preserved wherever in the chain it lives. A round-tripped
/// `GET /admin/config` body carries the leaf's `include` array explicitly
/// and the save keeps it verbatim; a candidate without an `include` key
/// inherits the leaf's current one (existing shadow first, then the real
/// file), so no save can sever the include chain by accident. A candidate
/// that spells out `include` (even an empty array) keeps it.
/// The candidate then passes the same merge-and-validate pass as a real
/// profile load (its `include` array drives the resolution), and only a
/// valid result is written to `<leaf>.toml.next`. The real files are
/// never touched and nothing is reloaded.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) when a redacted secret has
/// no existing value to preserve, the resolved chain fails config
/// validation (kind `Validation`, `Parse`, `Read`, `UnresolvedVar`,
/// `IncludeCycle`, or `IncludeDepth`), or the shadow cannot be written
/// (kind [`ConfigErrorKind::Write`](crate::ConfigErrorKind::Write)). On a
/// validation failure no shadow is written; a previously saved valid
/// shadow stays in place.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::save_profile_shadow;
/// use std::path::Path;
///
/// let document = toml::toml! {
///     [server]
///     bind = "127.0.0.1:8081"
///     api_key = "***"
/// };
/// let leaf = Path::new("profiles/default.toml");
/// let shadow = save_profile_shadow(leaf, toml::Value::Table(document))?;
/// assert!(shadow.ends_with("default.toml.next"));
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn save_profile_shadow(leaf: &Path, document: Value) -> Result<PathBuf, crate::ConfigError> {
    save_profile_shadow_repr(leaf, document).map_err(crate::ConfigError::from)
}

/// The crate-internal form of [`save_profile_shadow`].
fn save_profile_shadow_repr(leaf: &Path, mut document: Value) -> Result<PathBuf, Repr> {
    let current_leaf = current_file_doc(leaf)?;
    graft_include(&mut document, current_leaf.as_ref());
    // The restore source is the merged pending view: the body is the merged
    // config, so an inherited entry's secret lives in whichever chain file
    // (or shadow) currently defines it.
    let read = shadow_reader(None);
    let current = collect_config_chain_with(leaf, &read)?.value;
    restore_secrets(&mut document, Some(&current))?;
    validate_chain(leaf, leaf, &document)?;
    write_shadow_repr(leaf, &render_toml(&document)?)
}

/// Saves a candidate document as the shadow of one file in the include
/// chain.
///
/// Unlike [`save_profile_shadow`], the candidate is that single file's
/// content, so redacted secrets are restored from the target's own current
/// state (its existing shadow when one exists, otherwise the real file).
/// A candidate without an `include` key inherits the target's current one
/// from the same source, so a round-tripped body can not sever the
/// target's own include chain; a candidate that spells out `include`
/// (even an empty array) keeps it.
/// Validation still covers the whole pending configuration: the chain is
/// resolved from `leaf` with shadows preferred and the candidate standing
/// in for `target`, and the merged result must validate before
/// `<target>.toml.next` is written. A `target` that the resolved pending
/// chain never reaches is refused outright - it would never be substituted
/// into the merge, so validation would prove nothing about the candidate.
///
/// The caller is responsible for the trust boundary on `target` (the
/// gateway confines it to the profiles directory before calling in).
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) under the same conditions
/// as [`save_profile_shadow`]: an unresolvable `"***"` secret, a failed
/// merge-and-validate pass, or a failed shadow write (kind
/// [`ConfigErrorKind::Write`](crate::ConfigErrorKind::Write)); a `target`
/// outside the pending include chain fails with kind `Validation`. On a
/// validation failure no shadow is written.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::save_include_shadow;
/// use std::path::Path;
///
/// let document = toml::toml! {
///     [[endpoint]]
///     id = "shared"
///     protocol = "openai"
///     base_url = "https://api.example.com/v1"
///     api_key = "***"
/// };
/// let shadow = save_include_shadow(
///     Path::new("profiles/default.toml"),
///     Path::new("profiles/common.toml"),
///     toml::Value::Table(document),
/// )?;
/// assert!(shadow.ends_with("common.toml.next"));
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn save_include_shadow(
    leaf: &Path,
    target: &Path,
    document: Value,
) -> Result<PathBuf, crate::ConfigError> {
    save_include_shadow_repr(leaf, target, document).map_err(crate::ConfigError::from)
}

/// The crate-internal form of [`save_include_shadow`].
fn save_include_shadow_repr(
    leaf: &Path,
    target: &Path,
    mut document: Value,
) -> Result<PathBuf, Repr> {
    let current = current_file_doc(target)?;
    graft_include(&mut document, current.as_ref());
    restore_secrets(&mut document, current.as_ref())?;
    let read = shadow_reader(Some((target, &document)));
    let resolved = collect_config_chain_with(leaf, &read)?;
    // A target the chain never reaches is never substituted into the
    // merge, so "validated" would prove nothing about the candidate.
    if !resolved.chain.iter().any(|entry| same_file(entry, target)) {
        return Err(Repr::Validation(format!(
            "{} is not part of the active profile's pending include chain",
            target.display()
        )));
    }
    Config::from_value(resolved.value)?;
    write_shadow_repr(target, &render_toml(&document)?)
}

/// Saves a candidate document as the boot config's shadow
/// (`gateway.toml.next`).
///
/// Redacted secrets are restored from the boot file's own current state
/// (existing shadow first, then the real file), and a candidate without
/// an `include` key inherits the boot file's current one from the same
/// source, so a boot-sections-only body can not sever the boot file's
/// own include chain. The candidate must carry
/// the boot-owned sections: after resolving its own includes (shadows
/// preferred) and interpolating, a `[server]` section must be present and
/// well-formed and any `[workshop]` section must deserialize. When `leaf`
/// names the active profile, the profile chain is additionally re-validated
/// with the candidate standing in for the boot file, so a boot edit that
/// breaks the active profile's merged config fails at save time.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) when a redacted secret has
/// no existing value to preserve, the candidate lacks a well-formed
/// `[server]` section, a present `[workshop]` section does not deserialize,
/// the active profile's chain fails validation with the candidate in
/// place, or the shadow cannot be written (kind
/// [`ConfigErrorKind::Write`](crate::ConfigErrorKind::Write)). On any
/// failure before the write, no shadow is written.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::save_boot_shadow;
/// use std::path::Path;
///
/// let document = toml::toml! {
///     [server]
///     bind = "127.0.0.1:8081"
///     api_key = "***"
/// };
/// let shadow = save_boot_shadow(
///     Path::new("gateway.toml"),
///     Some(Path::new("profiles/default.toml")),
///     toml::Value::Table(document),
/// )?;
/// assert!(shadow.ends_with("gateway.toml.next"));
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn save_boot_shadow(
    boot: &Path,
    leaf: Option<&Path>,
    document: Value,
) -> Result<PathBuf, crate::ConfigError> {
    save_boot_shadow_repr(boot, leaf, document).map_err(crate::ConfigError::from)
}

/// The crate-internal form of [`save_boot_shadow`].
fn save_boot_shadow_repr(
    boot: &Path,
    leaf: Option<&Path>,
    mut document: Value,
) -> Result<PathBuf, Repr> {
    let current = current_file_doc(boot)?;
    graft_include(&mut document, current.as_ref());
    restore_secrets(&mut document, current.as_ref())?;
    // The boot file is the catalog and may legitimately fail full profile
    // validation, so the gate on the candidate itself is the boot-owned
    // sections: resolve its includes, interpolate, and require [server].
    let read = shadow_reader(Some((boot, &document)));
    let mut resolved = collect_config_chain_with(boot, &read)?.value;
    interpolate_value(&mut resolved)?;
    server_section(&resolved, boot)?;
    workshop_section(&resolved, boot)?;
    if let Some(leaf) = leaf {
        validate_chain(leaf, boot, &document)?;
    }
    write_shadow_repr(boot, &render_toml(&document)?)
}

/// Loads the pending configuration rooted at `leaf`: the include chain
/// resolved with existing shadows preferred, merged and validated exactly
/// like a real profile load.
///
/// Provenance names the shadow file (`<file>.toml.next`) wherever the
/// winning definition came from a file that carries a shadow, so the
/// `Config::to_json` annotations distinguish pending entries from running
/// ones. With no shadows on disk the result equals a real load of `leaf`.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) when a chain file or
/// shadow cannot be read or parsed, an include cycles or exceeds depth, an
/// interpolation fails, or the merged pending result fails config
/// validation.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::load_pending_profile;
/// use std::path::Path;
///
/// let pending = load_pending_profile(Path::new("profiles/default.toml"))?;
/// assert_eq!(pending.to_json()["server"]["api_key"], "***");
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn load_pending_profile(leaf: &Path) -> Result<Config, crate::ConfigError> {
    load_pending_profile_repr(leaf).map_err(crate::ConfigError::from)
}

/// The crate-internal form of [`load_pending_profile`].
fn load_pending_profile_repr(leaf: &Path) -> Result<Config, Repr> {
    let read = shadow_reader(None);
    let resolved = collect_config_chain_with(leaf, &read)?;
    let mut provenance = resolved.provenance;
    provenance.map_sources(|source| {
        let shadow = shadow_path(source);
        shadow.is_file().then_some(shadow)
    });
    let mut config = Config::from_value(resolved.value)?;
    config.set_provenance(provenance);
    // The reader preferred the leaf's shadow, so the recorded include
    // array is the pending one whenever a leaf shadow exists.
    config.set_include(resolved.root_include);
    Ok(config)
}

/// The `${VAR}` references the pending config chain carries, mapped to
/// space-joined labels of the referencing fields, keyed-array entries by
/// identity (`endpoint openai api_key`, `local_model llama source`).
///
/// The chain resolves from `leaf` with shadows preferred - the same
/// pending truth [`load_pending_profile`] renders - but the merged
/// document is walked before interpolation and without secret redaction,
/// because a loaded [`Config`] interpolates every `${VAR}` away and
/// serializes secrets as `"***"`: the references are visible only here.
/// Values never enter the result - only variable names and field labels -
/// so the reply carries no credential material. A file the chain never
/// reaches (a boot file no include names) contributes nothing.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) when a chain file or
/// shadow cannot be read or parsed, or an include cycles or exceeds depth.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::pending_var_references;
/// use std::path::Path;
///
/// let refs = pending_var_references(Path::new("profiles/default.toml"))?;
/// if let Some(labels) = refs.get("OPENAI_API_KEY") {
///     assert!(labels.iter().any(|label| label.ends_with("api_key")));
/// }
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn pending_var_references(
    leaf: &Path,
) -> Result<BTreeMap<String, Vec<String>>, crate::ConfigError> {
    let read = shadow_reader(None);
    let merged = collect_config_chain_with(leaf, &read)
        .map_err(crate::ConfigError::from)?
        .value;
    let mut refs = BTreeMap::new();
    collect_var_references(&merged, &mut Vec::new(), &mut refs);
    Ok(refs)
}

/// The identity key of one keyed config array, when `array` names one.
fn keyed_entry_key(array: &str) -> Option<&'static str> {
    match array {
        "model" | "local_model" => Some("name"),
        "endpoint" | "dominion" => Some("id"),
        _ => None,
    }
}

/// Walks one TOML value recording `${VAR}` hits in its string leaves,
/// labeling each hit with the space-joined key path (keyed-array entries
/// by their identity value).
fn collect_var_references(
    value: &Value,
    label: &mut Vec<String>,
    refs: &mut BTreeMap<String, Vec<String>>,
) {
    match value {
        Value::String(text) => {
            for name in referenced_var_names(text) {
                let labels = refs.entry(name).or_default();
                let joined = label.join(" ");
                if !labels.contains(&joined) {
                    labels.push(joined);
                }
            }
        }
        Value::Array(items) => {
            let id_key = label.last().and_then(|array| keyed_entry_key(array));
            for item in items {
                if let (Some(id_key), Value::Table(table)) = (id_key, item) {
                    let id = table
                        .get(id_key)
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    label.push(id.to_owned());
                    collect_var_references(item, label, refs);
                    label.pop();
                } else {
                    collect_var_references(item, label, refs);
                }
            }
        }
        Value::Table(table) => {
            for (key, child) in table {
                label.push(key.clone());
                collect_var_references(child, label, refs);
                label.pop();
            }
        }
        _ => {}
    }
}

/// The `${VAR}` names one string references, mirroring the interpolation
/// grammar: `$$` is a literal dollar and an unclosed `${...}` references
/// nothing.
fn referenced_var_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
            }
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for nc in chars.by_ref() {
                    if nc == '}' {
                        closed = true;
                        break;
                    }
                    name.push(nc);
                }
                if closed && !name.is_empty() {
                    names.push(name);
                }
            }
            _ => {}
        }
    }
    names
}

/// The pending state of the include chain rooted at one file: which real
/// files carry shadows and which top-level sections the shadows change.
///
/// Produced by [`pending_report`]. `changed_sections` treats an absent
/// section and a vacant one (an empty table or array) as equal, so a
/// round-tripped document that spells out empty defaults reports no
/// phantom change.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PendingReport {
    /// Real files in the chain (real or pending view) that carry a shadow,
    /// in chain order, without duplicates.
    pub shadowed_files: Vec<PathBuf>,
    /// Top-level sections whose merged value differs between the real and
    /// pending views, sorted.
    pub changed_sections: Vec<String>,
}

/// Reports the pending state of the include chain rooted at `root`: the
/// real files that carry shadows, and the top-level sections whose merged
/// value differs between the real view and the pending (shadow-preferring)
/// view.
///
/// `root` may be a profile leaf or the boot config: both views are merged
/// without validation, so the boot file's catalog shape is fine. Files are
/// listed by their real paths, never the `.next` names.
///
/// # Errors
/// Returns a [`ConfigError`](crate::ConfigError) when a chain file or
/// shadow cannot be read or parsed, or an include cycles or exceeds depth,
/// in either view.
///
/// # Examples
/// ```no_run
/// use promptforge_gateway_config::pending_report;
/// use std::path::Path;
///
/// let report = pending_report(Path::new("profiles/default.toml"))?;
/// if !report.shadowed_files.is_empty() {
///     println!("pending sections: {:?}", report.changed_sections);
/// }
/// # Ok::<(), promptforge_gateway_config::ConfigError>(())
/// ```
pub fn pending_report(root: &Path) -> Result<PendingReport, crate::ConfigError> {
    pending_report_repr(root).map_err(crate::ConfigError::from)
}

/// The crate-internal form of [`pending_report`].
fn pending_report_repr(root: &Path) -> Result<PendingReport, Repr> {
    let real = collect_config_chain(root)?;
    let reader = shadow_reader(None);
    let pending = collect_config_chain_with(root, &reader)?;
    // Both chains contribute: a shadow can add an include the real chain
    // never reaches, and vice versa.
    let mut shadowed_files: Vec<PathBuf> = Vec::new();
    for file in real.chain.iter().chain(pending.chain.iter()) {
        if shadow_path(file).is_file() && !shadowed_files.iter().any(|seen| same_file(seen, file)) {
            shadowed_files.push(file.clone());
        }
    }
    Ok(PendingReport {
        shadowed_files,
        changed_sections: changed_sections(&real.value, &pending.value),
    })
}

/// The top-level sections whose value differs between the two merged
/// views, sorted and deduplicated.
fn changed_sections(real: &Value, pending: &Value) -> Vec<String> {
    let empty = toml::map::Map::new();
    let real = real.as_table().unwrap_or(&empty);
    let pending = pending.as_table().unwrap_or(&empty);
    let mut sections: Vec<String> = real
        .keys()
        .chain(pending.keys())
        .filter(|key| section_differs(real.get(key.as_str()), pending.get(key.as_str())))
        .cloned()
        .collect();
    sections.sort_unstable();
    sections.dedup();
    sections
}

/// Whether one section's two views differ. When the section is present in
/// both, plain value inequality decides; otherwise the present side must
/// be vacant for the views to count as equal.
fn section_differs(real: Option<&Value>, pending: Option<&Value>) -> bool {
    match (real, pending) {
        (Some(real), Some(pending)) => real != pending,
        (real, pending) => !(vacant(real) && vacant(pending)),
    }
}

/// Whether a section value is as good as absent: missing, an empty table,
/// or an empty array. A serialized document spells out empty defaults
/// (`dominion = []`) that an operator-authored file simply omits.
fn vacant(value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(Value::Table(table)) => table.is_empty(),
        Some(Value::Array(items)) => items.is_empty(),
        Some(_) => false,
    }
}

/// The target's current document for secret restoration: its existing
/// shadow when one exists (the pending truth the operator last saved),
/// otherwise the real file. `None` when neither exists yet.
fn current_file_doc(target: &Path) -> Result<Option<Value>, Repr> {
    let shadow = shadow_path(target);
    if shadow.is_file() {
        return read_doc_from_disk(&shadow).map(Some);
    }
    if target.is_file() {
        return read_doc_from_disk(target).map(Some);
    }
    Ok(None)
}

/// Grafts `current`'s `include` array onto a candidate that carries no
/// `include` key of its own. `Config::to_json` emits `include`, so a
/// round-tripped body normally spells it out; this is the safety net for
/// an older or partial client whose chainless body would otherwise
/// replace the target file with a chainless copy - severing inheritance
/// and baking inherited entries flat into the shadow. A candidate that
/// spells out `include` (even an empty array) keeps it: removing the
/// chain must be deliberate.
fn graft_include(candidate: &mut Value, current: Option<&Value>) {
    let Value::Table(table) = candidate else {
        return;
    };
    if table.contains_key("include") {
        return;
    }
    if let Some(include) = current.and_then(|value| value.get("include")) {
        table.insert("include".to_owned(), include.clone());
    }
}

/// Resolves the chain from `leaf` with shadows preferred and `candidate`
/// standing in for its target file, then runs full config validation on
/// the merged result.
fn validate_chain(leaf: &Path, target: &Path, candidate: &Value) -> Result<(), Repr> {
    let read = shadow_reader(Some((target, candidate)));
    let merged = collect_config_chain_with(leaf, &read)?.value;
    Config::from_value(merged)?;
    Ok(())
}

/// A chain reader that substitutes `candidate` for its target path and an
/// existing shadow for any other file that has one; everything else reads
/// from disk.
fn shadow_reader<'a>(
    candidate: Option<(&'a Path, &'a Value)>,
) -> impl Fn(&Path) -> Result<Value, Repr> + 'a {
    move |path: &Path| {
        if let Some((target, document)) = candidate
            && same_file(path, target)
        {
            return Ok(document.clone());
        }
        let shadow = shadow_path(path);
        if shadow.is_file() {
            read_doc_from_disk(&shadow)
        } else {
            read_doc_from_disk(path)
        }
    }
}

/// Whether two paths name the same file, comparing canonicalized forms and
/// falling back to the raw paths when canonicalization fails (for example
/// a candidate target that does not exist on disk yet).
fn same_file(a: &Path, b: &Path) -> bool {
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    canonical(a) == canonical(b)
}

/// Renders the candidate as pretty TOML for the shadow file.
fn render_toml(document: &Value) -> Result<String, Repr> {
    toml::to_string_pretty(document)
        .map_err(|source| Repr::Validation(format!("candidate does not render as TOML: {source}")))
}

/// Replaces every `"***"` string leaf in `candidate` with the value at the
/// same position in `current`. Top-level keyed arrays (`[[endpoint]]`,
/// `[[model]]`, `[[local_model]]`, `[[dominion]]`) match entries by
/// `id`/`name`, not by index, so reordering never mismatches a secret.
fn restore_secrets(candidate: &mut Value, current: Option<&Value>) -> Result<(), Repr> {
    if let (Value::Table(table), current) = (&mut *candidate, current) {
        for (key, child) in table.iter_mut() {
            let counterpart = current.and_then(|value| value.get(key.as_str()));
            match key.as_str() {
                "endpoint" | "model" | "local_model" | "dominion" => {
                    restore_keyed_array(child, counterpart, key)?;
                }
                _ => restore_node(child, counterpart, key)?,
            }
        }
        return Ok(());
    }
    restore_node(candidate, current, "")
}

/// [`restore_secrets`] for one keyed array: entries pair by their
/// `id`/`name` field against the current array.
fn restore_keyed_array(
    candidate: &mut Value,
    current: Option<&Value>,
    array_name: &str,
) -> Result<(), Repr> {
    let Value::Array(items) = candidate else {
        // A malformed array is the validation pass's diagnostic to make.
        return restore_node(candidate, current, array_name);
    };
    let key_field = match array_name {
        "endpoint" | "dominion" => "id",
        _ => "name",
    };
    for (index, item) in items.iter_mut().enumerate() {
        let identity = item
            .get(key_field)
            .and_then(Value::as_str)
            .map(str::to_owned);
        let counterpart = match (&identity, current) {
            (Some(id), Some(Value::Array(existing))) => existing
                .iter()
                .find(|entry| entry.get(key_field).and_then(Value::as_str) == Some(id.as_str())),
            _ => None,
        };
        let label = identity.map_or_else(
            || format!("{array_name}[{index}]"),
            |id| format!("{array_name} {id}"),
        );
        restore_node(item, counterpart, &label)?;
    }
    Ok(())
}

/// The recursive body of [`restore_secrets`]: tables walk by key, arrays
/// by index, and a `"***"` string leaf takes the current value or errors.
fn restore_node(candidate: &mut Value, current: Option<&Value>, path: &str) -> Result<(), Repr> {
    match candidate {
        Value::String(text) if text == REDACTED => match current {
            Some(Value::String(existing)) => {
                existing.clone_into(text);
                Ok(())
            }
            _ => Err(Repr::Validation(format!(
                "secret marker \"***\" at {path} has no existing value to preserve; \
                 send a literal or a ${{VAR}} reference"
            ))),
        },
        Value::Table(table) => {
            for (key, child) in table.iter_mut() {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let counterpart = current.and_then(|value| value.get(key.as_str()));
                restore_node(child, counterpart, &child_path)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                let counterpart = current.and_then(|value| value.get(index));
                restore_node(item, counterpart, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A self-contained profile: everything a valid config needs, with a
    /// literal endpoint secret to round-trip.
    const LEAF: &str = r#"
[server]
bind = "127.0.0.1:0"
api_key = "boot-key"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = "disk-secret"

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    fn parse(body: &str) -> Value {
        toml::from_str(body).unwrap()
    }

    #[test]
    fn shadow_path_appends_next_after_the_real_extension() {
        assert_eq!(
            shadow_path(Path::new("profiles/default.toml")),
            PathBuf::from("profiles/default.toml.next")
        );
        assert_eq!(
            shadow_path(Path::new("gateway.env")),
            PathBuf::from("gateway.env.next")
        );
    }

    #[test]
    fn profile_listing_never_sees_a_shadow() {
        // The `.toml.next` suffix exists so `list_profiles`'s `*.toml`
        // match cannot report a phantom `default.next` profile.
        let temp = tempfile::TempDir::new().unwrap();
        let leaf = write(temp.path(), "default.toml", LEAF);
        write_shadow(&leaf, LEAF).unwrap();
        let names = crate::profile::list_profiles(temp.path()).unwrap();
        assert_eq!(names, ["default"]);
    }

    #[test]
    fn pending_var_references_labels_reference_sites_by_entry_identity() {
        let temp = tempfile::TempDir::new().unwrap();
        let leaf = write(
            temp.path(),
            "default.toml",
            &LEAF.replace(
                "api_key = \"disk-secret\"",
                "api_key = \"${PFG_REFS_ENDPOINT_KEY}\"",
            ),
        );

        let refs = pending_var_references(&leaf).unwrap();
        assert_eq!(
            refs.get("PFG_REFS_ENDPOINT_KEY").map(Vec::as_slice),
            Some(&["endpoint e api_key".to_owned()][..]),
            "a secret's raw ${{VAR}} reference is visible pre-interpolation"
        );

        // A staged shadow replaces the real file in the scanned chain.
        let shadowed = LEAF
            .replace(
                "api_key = \"boot-key\"",
                "api_key = \"${PFG_REFS_SERVER_KEY}\"",
            )
            .replace("api_key = \"disk-secret\"", "api_key = \"$${NOT_A_REF}\"");
        write_shadow(&leaf, &shadowed).unwrap();
        let refs = pending_var_references(&leaf).unwrap();
        assert_eq!(
            refs.get("PFG_REFS_SERVER_KEY").map(Vec::as_slice),
            Some(&["server api_key".to_owned()][..]),
            "the shadow-preferred chain feeds the scan"
        );
        assert!(
            !refs.contains_key("PFG_REFS_ENDPOINT_KEY"),
            "the replaced real-file reference is gone"
        );
        assert!(
            !refs.contains_key("NOT_A_REF"),
            "a $$-escaped dollar references nothing"
        );
    }

    #[test]
    fn write_shadow_replaces_an_existing_shadow() {
        let temp = tempfile::TempDir::new().unwrap();
        let target = temp.path().join("default.toml");
        let shadow = write_shadow(&target, "first = 1\n").unwrap();
        write_shadow(&target, "second = 2\n").unwrap();
        assert_eq!(fs::read_to_string(&shadow).unwrap(), "second = 2\n");
    }

    #[test]
    fn a_failed_rename_cleans_up_the_temp_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let target = temp.path().join("default.toml");
        // A directory squatting on the shadow path defeats both rename
        // attempts and the remove between them, driving the cleanup arm.
        fs::create_dir(shadow_path(&target)).unwrap();
        let error = write_shadow(&target, "x = 1\n").unwrap_err();
        assert_eq!(error.kind(), crate::ConfigErrorKind::Write);
        let litter: Vec<String> = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(
            litter.is_empty(),
            "no temp file remains after a failed rename: {litter:?}"
        );
    }

    #[test]
    fn promote_shadow_makes_the_shadow_the_real_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let target = write(temp.path(), "default.toml", "old = 1\n");
        write_shadow(&target, "new = 2\n").unwrap();

        promote_shadow(&target).unwrap();

        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "new = 2\n",
            "the real file carries the shadow's content"
        );
        assert!(
            !shadow_path(&target).exists(),
            "the shadow disappears in the promotion"
        );
    }

    #[test]
    fn promote_shadow_creates_a_real_file_that_did_not_exist() {
        // An env shadow can precede its real file; promotion creates it.
        let temp = tempfile::TempDir::new().unwrap();
        let target = temp.path().join("default.env");
        write_shadow(&target, "HF_TOKEN=x\n").unwrap();

        promote_shadow(&target).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "HF_TOKEN=x\n");
        assert!(!shadow_path(&target).exists());
    }

    #[test]
    fn promote_shadow_without_a_shadow_errors() {
        let temp = tempfile::TempDir::new().unwrap();
        let target = write(temp.path(), "default.toml", "old = 1\n");

        let error = promote_shadow(&target).unwrap_err();
        assert_eq!(error.kind(), crate::ConfigErrorKind::Write);
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "old = 1\n",
            "the real file is untouched by a failed promotion"
        );
    }

    #[test]
    fn save_profile_shadow_writes_the_shadow_and_never_the_real_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let leaf = write(temp.path(), "default.toml", LEAF);

        let mut document = parse(LEAF);
        document["model"][0]["description"] = Value::String("edited".to_owned());
        let shadow = save_profile_shadow(&leaf, document).unwrap();

        assert_eq!(shadow, shadow_path(&leaf));
        assert_eq!(
            fs::read_to_string(&leaf).unwrap(),
            LEAF,
            "the real file is byte-identical after the save"
        );
        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(written["model"][0]["description"].as_str(), Some("edited"));
    }

    #[test]
    fn a_save_failing_validation_leaves_no_shadow_or_temp_litter() {
        let temp = tempfile::TempDir::new().unwrap();
        let leaf = write(temp.path(), "default.toml", LEAF);

        let mut document = parse(LEAF);
        document["model"][0]["endpoints"] = Value::Array(vec![Value::String("ghost".to_owned())]);
        let error = save_profile_shadow(&leaf, document).unwrap_err();
        assert_eq!(error.kind(), crate::ConfigErrorKind::Validation);
        assert!(error.to_string().contains("ghost"), "got: {error}");

        let entries: Vec<String> = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            ["default.toml"],
            "no shadow and no temp file remain after a failed save"
        );
    }

    #[test]
    fn a_redacted_secret_restores_the_on_disk_value_and_a_literal_replaces_it() {
        let temp = tempfile::TempDir::new().unwrap();
        let leaf = write(temp.path(), "default.toml", LEAF);

        let mut document = parse(LEAF);
        document["server"]["api_key"] = Value::String("***".to_owned());
        document["endpoint"][0]["api_key"] = Value::String("new-literal".to_owned());
        let shadow = save_profile_shadow(&leaf, document).unwrap();

        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(
            written["server"]["api_key"].as_str(),
            Some("boot-key"),
            "\"***\" preserves the current on-disk secret"
        );
        assert_eq!(
            written["endpoint"][0]["api_key"].as_str(),
            Some("new-literal"),
            "a literal in the candidate replaces the secret"
        );
    }

    #[test]
    fn restore_secrets_matches_keyed_entries_by_id_not_index() {
        // The current file lists endpoints in the opposite order; the
        // secret must follow the id, not the array position.
        let current = parse(
            "[[endpoint]]\nid = 'b'\napi_key = 'b-secret'\n\
             [[endpoint]]\nid = 'a'\napi_key = 'a-secret'\n",
        );
        let mut candidate = parse(
            "[[endpoint]]\nid = 'a'\napi_key = '***'\n\
             [[endpoint]]\nid = 'b'\napi_key = '***'\n",
        );
        restore_secrets(&mut candidate, Some(&current)).unwrap();
        assert_eq!(
            candidate["endpoint"][0]["api_key"].as_str(),
            Some("a-secret")
        );
        assert_eq!(
            candidate["endpoint"][1]["api_key"].as_str(),
            Some("b-secret")
        );
    }

    #[test]
    fn restore_secrets_keeps_a_var_reference_verbatim() {
        let current = parse("[server]\napi_key = 'old'\n");
        let mut candidate = parse("[server]\napi_key = '${GATEWAY_KEY}'\n");
        restore_secrets(&mut candidate, Some(&current)).unwrap();
        assert_eq!(
            candidate["server"]["api_key"].as_str(),
            Some("${GATEWAY_KEY}"),
            "a ${{VAR}} reference is written as-is, never substituted here"
        );
    }

    #[test]
    fn restore_secrets_errors_when_no_existing_value_backs_the_marker() {
        let mut candidate = parse("[[endpoint]]\nid = 'brand-new'\napi_key = '***'\n");
        let error = restore_secrets(&mut candidate, None).unwrap_err();
        assert!(
            error.to_string().contains("brand-new"),
            "the error names the entry: {error}"
        );
    }

    #[test]
    fn chain_validation_prefers_existing_shadows() {
        // common.toml defines endpoint `old`; its pending shadow renames it
        // to `renamed`. A leaf save referencing `renamed` passes only when
        // the resolver honors the shadow, and one referencing `old` fails
        // for the same reason.
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "common.toml",
            "[[endpoint]]\nid = 'old'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = ''\n",
        );
        write_shadow(
            &temp.path().join("common.toml"),
            "[[endpoint]]\nid = 'renamed'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = ''\n",
        )
        .unwrap();
        let leaf_body = "include = ['common.toml']\n\n\
             [server]\nbind = '127.0.0.1:0'\napi_key = 'k'\n\n\
             [[model]]\nname = 'm'\ndescription = 'p'\ncontext = 1\n\
             upstream = 'u'\nendpoints = ['renamed']\n";
        let leaf = write(temp.path(), "main.toml", leaf_body);

        save_profile_shadow(&leaf, parse(leaf_body))
            .expect("the include's shadow defines `renamed`, so the merge validates");

        let stale = leaf_body.replace("'renamed'", "'old'");
        let error = save_profile_shadow(&leaf, parse(&stale)).unwrap_err();
        assert!(
            error.to_string().contains("old"),
            "the real file's endpoint is shadowed out: {error}"
        );
    }

    #[test]
    fn a_candidate_without_include_grafts_the_current_include_chain() {
        // A round-tripped GET /admin/config body never carries `include`,
        // so without the graft this save would replace the leaf with a
        // chainless copy: validation would lose common.toml's endpoint
        // and inherited entries would bake flat into the leaf shadow.
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "common.toml",
            &common_body("http://127.0.0.1:9"),
        );
        let leaf = write(temp.path(), "main.toml", CHAIN_LEAF);

        let chainless = CHAIN_LEAF.replace("include = ['common.toml']\n", "");
        let shadow = save_profile_shadow(&leaf, parse(&chainless))
            .expect("the graft keeps common.toml in the chain, so the merge validates");

        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(
            written["include"],
            parse("include = ['common.toml']")["include"],
            "the shadow still carries the leaf's include line"
        );
        let json = load_pending_profile(&leaf).unwrap().to_json();
        let endpoint_source = json["endpoint"][0]["source_file"]
            .as_str()
            .expect("a source_file annotation");
        assert!(
            endpoint_source.ends_with("common.toml"),
            "the inherited endpoint stays in the parent, not baked into the leaf: {endpoint_source}"
        );
    }

    #[test]
    fn a_candidate_with_an_explicit_include_keeps_it() {
        // The include editor sends `include` deliberately; the graft must
        // never override an explicit value, an empty array included.
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "common.toml",
            &common_body("http://127.0.0.1:9"),
        );
        write(
            temp.path(),
            "common2.toml",
            &common_body("http://127.0.0.1:10"),
        );
        let leaf = write(temp.path(), "main.toml", CHAIN_LEAF);

        let repointed = CHAIN_LEAF.replace("'common.toml'", "'common2.toml'");
        let shadow = save_profile_shadow(&leaf, parse(&repointed)).unwrap();
        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(
            written["include"],
            parse("include = ['common2.toml']")["include"],
            "an explicit include repoints the chain"
        );

        let severed = format!("include = []\n{LEAF}");
        let shadow = save_profile_shadow(&leaf, parse(&severed)).unwrap();
        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(
            written["include"],
            parse("include = []")["include"],
            "an explicit empty include severs the chain deliberately"
        );
    }

    #[test]
    fn a_profile_save_keeps_a_staged_boot_edit_in_the_pending_view() {
        // The step-18 scenario: a [server] edit staged in gateway.toml.next
        // must survive a round-tripped profile save. Without the graft the
        // leaf shadow drops its include line, the chain stops visiting the
        // boot file, and the staged edit vanishes from the pending view.
        let temp = tempfile::TempDir::new().unwrap();
        let boot = write(
            temp.path(),
            "gateway.toml",
            "[server]\nbind = '127.0.0.1:0'\napi_key = 'boot-key'\n",
        );
        let leaf_body = "include = ['gateway.toml']\n\n\
             [[endpoint]]\nid = 'e'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = ''\n\n\
             [[model]]\nname = 'm'\ndescription = 'p'\ncontext = 1\n\
             upstream = 'u'\nendpoints = ['e']\n";
        let leaf = write(temp.path(), "default.toml", leaf_body);

        save_boot_shadow(
            &boot,
            Some(&leaf),
            parse("[server]\nbind = '127.0.0.1:1'\napi_key = 'boot-key'\n"),
        )
        .unwrap();

        // What the UI sends after its fix: the merged view minus the
        // boot-owned sections, with no include line.
        let candidate = parse(
            "[[endpoint]]\nid = 'e'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = ''\n\n\
             [[model]]\nname = 'm'\ndescription = 'p'\ncontext = 1\n\
             upstream = 'u'\nendpoints = ['e']\n",
        );
        save_profile_shadow(&leaf, candidate).unwrap();

        let json = load_pending_profile(&leaf).unwrap().to_json();
        assert_eq!(
            json["server"]["bind"], "127.0.0.1:1",
            "the staged boot edit still reaches the pending profile view"
        );
    }

    #[test]
    fn an_include_candidate_without_include_keeps_its_own_chain() {
        // An include file can itself include; a round-tripped save of that
        // file must not sever its chain either.
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "common.toml",
            &common_body("http://127.0.0.1:9"),
        );
        let mid = write(
            temp.path(),
            "mid.toml",
            "include = ['common.toml']\n\n[local]\ncache_dir = 'a'\n",
        );
        let leaf_body = CHAIN_LEAF.replace("'common.toml'", "'mid.toml'");
        let leaf = write(temp.path(), "main.toml", &leaf_body);

        let candidate = parse("[local]\ncache_dir = 'b'\n");
        let shadow = save_include_shadow(&leaf, &mid, candidate)
            .expect("the graft keeps common.toml reachable, so the merge validates");
        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(
            written["include"],
            parse("include = ['common.toml']")["include"],
            "the include file's own chain survives the round-trip"
        );
        assert_eq!(written["local"]["cache_dir"].as_str(), Some("b"));
    }

    #[test]
    fn a_boot_candidate_without_include_keeps_the_boot_include_line() {
        // buildBootPayload carries only the boot-owned sections; a boot
        // file with its own includes must not lose them to that save.
        let temp = tempfile::TempDir::new().unwrap();
        write(temp.path(), "extra.toml", "[local]\ncache_dir = 'x'\n");
        let boot = write(
            temp.path(),
            "gateway.toml",
            "include = ['extra.toml']\n\n\
             [server]\nbind = '127.0.0.1:0'\napi_key = 'boot-key'\n",
        );

        let candidate = parse("[server]\nbind = '127.0.0.1:1'\napi_key = '***'\n");
        let shadow = save_boot_shadow(&boot, None, candidate).unwrap();
        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(
            written["include"],
            parse("include = ['extra.toml']")["include"],
            "the boot file's include line survives a boot-sections-only save"
        );
        assert_eq!(
            written["server"]["api_key"].as_str(),
            Some("boot-key"),
            "secret restoration still works alongside the graft"
        );
    }

    #[test]
    fn save_include_shadow_refuses_a_file_outside_the_chain() {
        // A confined target the pending chain never reaches would bypass
        // the merge entirely, so its candidate must be refused unvalidated.
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "common.toml",
            "[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = ''\n",
        );
        let stray = write(
            temp.path(),
            "stray.toml",
            "[[endpoint]]\nid = 'unused'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = ''\n",
        );
        let leaf_body = "include = ['common.toml']\n\n\
             [server]\nbind = '127.0.0.1:0'\napi_key = 'k'\n\n\
             [[model]]\nname = 'm'\ndescription = 'p'\ncontext = 1\n\
             upstream = 'u'\nendpoints = ['shared']\n";
        let leaf = write(temp.path(), "main.toml", leaf_body);

        let candidate = parse("not_even = 'a valid config shape'\n");
        let error = save_include_shadow(&leaf, &stray, candidate).unwrap_err();
        assert_eq!(error.kind(), crate::ConfigErrorKind::Validation);
        assert!(
            error.to_string().contains("pending include chain"),
            "the refusal names the reason: {error}"
        );
        assert!(
            !shadow_path(&stray).exists(),
            "no unvalidated shadow is written for an out-of-chain target"
        );
    }

    #[test]
    fn save_include_shadow_prefers_the_pending_shadow_for_restoration() {
        // current_file_doc reads the target's shadow before its real file,
        // so a "***" save after a pending secret change must keep the new
        // pending secret, never revert to the real file's old one.
        let temp = tempfile::TempDir::new().unwrap();
        let common = write(
            temp.path(),
            "common.toml",
            "[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = 'old-secret'\n",
        );
        write_shadow(
            &common,
            "[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = 'pending-secret'\n",
        )
        .unwrap();
        let leaf_body = "include = ['common.toml']\n\n\
             [server]\nbind = '127.0.0.1:0'\napi_key = 'k'\n\n\
             [[model]]\nname = 'm'\ndescription = 'p'\ncontext = 1\n\
             upstream = 'u'\nendpoints = ['shared']\n";
        let leaf = write(temp.path(), "main.toml", leaf_body);

        let candidate = parse(
            "[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = '***'\n",
        );
        let shadow = save_include_shadow(&leaf, &common, candidate).unwrap();
        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(
            written["endpoint"][0]["api_key"].as_str(),
            Some("pending-secret"),
            "the pending shadow outranks the real file as the restore source"
        );
    }

    #[test]
    fn save_profile_shadow_restores_secrets_from_the_pending_chain() {
        // The leaf save's restore source is the merged pending view, so a
        // chain file's shadow supplies the secret over its real file.
        let temp = tempfile::TempDir::new().unwrap();
        let common = write(
            temp.path(),
            "common.toml",
            "[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = 'old-secret'\n",
        );
        write_shadow(
            &common,
            "[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = 'pending-secret'\n",
        )
        .unwrap();
        let leaf_body = "include = ['common.toml']\n\n\
             [server]\nbind = '127.0.0.1:0'\napi_key = 'k'\n\n\
             [[model]]\nname = 'm'\ndescription = 'p'\ncontext = 1\n\
             upstream = 'u'\nendpoints = ['shared']\n";
        let leaf = write(temp.path(), "main.toml", leaf_body);

        let candidate = parse(&format!(
            "{leaf_body}\n[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = '***'\n"
        ));
        let shadow = save_profile_shadow(&leaf, candidate).unwrap();
        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(
            written["endpoint"][0]["api_key"].as_str(),
            Some("pending-secret"),
            "the merged pending view supplies the preserved secret"
        );
    }

    #[test]
    fn save_include_shadow_restores_from_the_target_file_itself() {
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "common.toml",
            "[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:9'\napi_key = 'common-secret'\n",
        );
        let leaf_body = "include = ['common.toml']\n\n\
             [server]\nbind = '127.0.0.1:0'\napi_key = 'k'\n\n\
             [[model]]\nname = 'm'\ndescription = 'p'\ncontext = 1\n\
             upstream = 'u'\nendpoints = ['shared']\n";
        let leaf = write(temp.path(), "main.toml", leaf_body);

        let candidate = parse(
            "[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = 'http://127.0.0.1:10'\napi_key = '***'\n",
        );
        let shadow =
            save_include_shadow(&leaf, &temp.path().join("common.toml"), candidate).unwrap();
        let written = parse(&fs::read_to_string(&shadow).unwrap());
        assert_eq!(
            written["endpoint"][0]["api_key"].as_str(),
            Some("common-secret"),
            "the include's own file supplies the preserved secret"
        );
        assert!(
            temp.path().join("common.toml.next").is_file(),
            "the shadow sits beside the include"
        );
    }

    /// A leaf including `common.toml`, whose model routes through the
    /// endpoint `shared` that common defines.
    const CHAIN_LEAF: &str = "include = ['common.toml']\n\n\
         [server]\nbind = '127.0.0.1:0'\napi_key = 'k'\n\n\
         [[model]]\nname = 'm'\ndescription = 'p'\ncontext = 1\n\
         upstream = 'u'\nendpoints = ['shared']\n";

    /// The `common.toml` body for the chain fixtures, with a swappable
    /// base URL.
    fn common_body(base_url: &str) -> String {
        format!(
            "[[endpoint]]\nid = 'shared'\nprotocol = 'openai'\n\
             base_url = '{base_url}'\napi_key = ''\n"
        )
    }

    #[test]
    fn load_pending_profile_prefers_shadows_and_names_them_in_provenance() {
        let temp = tempfile::TempDir::new().unwrap();
        let common = write(
            temp.path(),
            "common.toml",
            &common_body("http://127.0.0.1:9"),
        );
        write_shadow(&common, &common_body("http://127.0.0.1:10")).unwrap();
        let leaf = write(temp.path(), "main.toml", CHAIN_LEAF);

        let json = load_pending_profile(&leaf).unwrap().to_json();
        assert_eq!(
            json["endpoint"][0]["base_url"], "http://127.0.0.1:10",
            "the shadow's definition wins the pending merge"
        );
        let endpoint_source = json["endpoint"][0]["source_file"]
            .as_str()
            .expect("a source_file annotation");
        assert!(
            endpoint_source.ends_with("common.toml.next"),
            "a shadow-supplied entry names the shadow file: {endpoint_source}"
        );
        let model_source = json["model"][0]["source_file"]
            .as_str()
            .expect("a source_file annotation");
        assert!(
            model_source.ends_with("main.toml"),
            "an unshadowed file keeps its real name: {model_source}"
        );
    }

    #[test]
    fn the_pending_view_prefers_the_leaf_shadows_include_array() {
        // The chain editor reads the pending payload; a staged reorder or
        // repoint of the leaf's include line must show there, not the real
        // file's old order.
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "common.toml",
            &common_body("http://127.0.0.1:9"),
        );
        write(
            temp.path(),
            "common2.toml",
            &common_body("http://127.0.0.1:10"),
        );
        let leaf = write(temp.path(), "main.toml", CHAIN_LEAF);

        let real = load_pending_profile(&leaf).unwrap().to_json();
        assert_eq!(
            real["include"],
            serde_json::json!(["common.toml"]),
            "no shadow: the real leaf's include line renders"
        );

        write_shadow(
            &leaf,
            &CHAIN_LEAF.replace(
                "include = ['common.toml']",
                "include = ['common2.toml', 'common.toml']",
            ),
        )
        .unwrap();
        let pending = load_pending_profile(&leaf).unwrap().to_json();
        assert_eq!(
            pending["include"],
            serde_json::json!(["common2.toml", "common.toml"]),
            "the leaf shadow's include array outranks the real file's"
        );
    }

    #[test]
    fn load_pending_profile_without_shadows_equals_a_real_load() {
        let temp = tempfile::TempDir::new().unwrap();
        write(
            temp.path(),
            "common.toml",
            &common_body("http://127.0.0.1:9"),
        );
        let leaf = write(temp.path(), "main.toml", CHAIN_LEAF);

        let pending = load_pending_profile(&leaf).unwrap().to_json();
        let real = crate::Config::load(&leaf).unwrap().to_json();
        assert_eq!(
            pending, real,
            "no shadows: the pending view is the real view"
        );
    }

    #[test]
    fn pending_report_lists_shadowed_files_and_changed_sections() {
        let temp = tempfile::TempDir::new().unwrap();
        let common = write(
            temp.path(),
            "common.toml",
            &common_body("http://127.0.0.1:9"),
        );
        let leaf = write(temp.path(), "main.toml", CHAIN_LEAF);

        let clean = pending_report(&leaf).unwrap();
        assert!(
            clean.shadowed_files.is_empty(),
            "{:?}",
            clean.shadowed_files
        );
        assert!(
            clean.changed_sections.is_empty(),
            "{:?}",
            clean.changed_sections
        );

        write_shadow(&common, &common_body("http://127.0.0.1:10")).unwrap();
        let report = pending_report(&leaf).unwrap();
        assert_eq!(
            report.shadowed_files,
            std::slice::from_ref(&common),
            "the shadowed chain file is listed by its real path"
        );
        assert_eq!(
            report.changed_sections,
            ["endpoint"],
            "only the section the shadow changes is reported"
        );
    }

    #[test]
    fn pending_report_treats_vacant_sections_as_absent() {
        // A round-tripped document spells out empty defaults
        // (`dominion = []`, `[local]`); those must not read as changes
        // against a real file that simply omits them.
        let temp = tempfile::TempDir::new().unwrap();
        let leaf = write(temp.path(), "default.toml", LEAF);
        write_shadow(&leaf, &format!("dominion = []\n\n[local]\n{LEAF}")).unwrap();

        let report = pending_report(&leaf).unwrap();
        assert_eq!(report.shadowed_files, std::slice::from_ref(&leaf));
        assert_eq!(
            report.changed_sections,
            Vec::<String>::new(),
            "vacant sections in the shadow report no change"
        );
    }

    #[test]
    fn pending_report_reports_deleted_and_added_sections() {
        // The absent-equals-vacant rule must not mask a real deletion: a
        // shadow that drops a populated section still reports it, and a
        // section the shadow introduces reports symmetrically.
        let temp = tempfile::TempDir::new().unwrap();
        let leaf = write(temp.path(), "default.toml", LEAF);
        let without_model = LEAF
            .split("[[model]]")
            .next()
            .expect("LEAF carries a model block");
        write_shadow(&leaf, &format!("{without_model}[local]\ncache_dir = 'c'\n")).unwrap();

        let report = pending_report(&leaf).unwrap();
        assert_eq!(
            report.changed_sections,
            ["local", "model"],
            "the deleted model section and the added local section both report"
        );
    }

    #[test]
    fn save_boot_shadow_requires_a_server_section() {
        let temp = tempfile::TempDir::new().unwrap();
        let boot = write(temp.path(), "gateway.toml", LEAF);

        let error =
            save_boot_shadow(&boot, None, parse("[local]\ncache_dir = '/tmp'\n")).unwrap_err();
        assert!(
            error.to_string().contains("[server]"),
            "the refusal names the missing section: {error}"
        );
        assert!(
            !shadow_path(&boot).exists(),
            "no shadow is written for a refused boot candidate"
        );

        let shadow = save_boot_shadow(&boot, None, parse(LEAF)).unwrap();
        assert!(shadow.ends_with("gateway.toml.next"));
        assert_eq!(
            fs::read_to_string(&boot).unwrap(),
            LEAF,
            "the real boot file is byte-identical after the save"
        );
    }
}
