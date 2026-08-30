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

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use toml::Value;

use crate::config::{Config, interpolate_value};
use crate::error::ConfigError as Repr;

use super::{collect_config_chain_with, read_doc_from_disk, server_section, workshop_section};

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

/// Saves a candidate document as the active profile leaf's shadow.
///
/// `document` is the full config in the TOML shape (what
/// `GET /admin/config` returns, secrets as `"***"`). Redacted secrets are
/// restored from the current pending state - the include chain resolved
/// with existing shadows preferred - so the winning definition's secret is
/// preserved wherever in the chain it lives. The candidate then passes the
/// same merge-and-validate pass as a real profile load (its own `include`
/// array drives the resolution), and only a valid result is written to
/// `<leaf>.toml.next`. The real files are never touched and nothing is
/// reloaded.
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
    // The restore source is the merged pending view: the body is the merged
    // config, so an inherited entry's secret lives in whichever chain file
    // (or shadow) currently defines it.
    let read = shadow_reader(None);
    let (current, _chain, _provenance) = collect_config_chain_with(leaf, &read)?;
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
    restore_secrets(&mut document, current.as_ref())?;
    let read = shadow_reader(Some((target, &document)));
    let (merged, chain, _provenance) = collect_config_chain_with(leaf, &read)?;
    // A target the chain never reaches is never substituted into the
    // merge, so "validated" would prove nothing about the candidate.
    if !chain.iter().any(|entry| same_file(entry, target)) {
        return Err(Repr::Validation(format!(
            "{} is not part of the active profile's pending include chain",
            target.display()
        )));
    }
    Config::from_value(merged)?;
    write_shadow_repr(target, &render_toml(&document)?)
}

/// Saves a candidate document as the boot config's shadow
/// (`gateway.toml.next`).
///
/// Redacted secrets are restored from the boot file's own current state
/// (existing shadow first, then the real file). The candidate must carry
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
    restore_secrets(&mut document, current.as_ref())?;
    // The boot file is the catalog and may legitimately fail full profile
    // validation, so the gate on the candidate itself is the boot-owned
    // sections: resolve its includes, interpolate, and require [server].
    let read = shadow_reader(Some((boot, &document)));
    let (mut resolved, _chain, _provenance) = collect_config_chain_with(boot, &read)?;
    interpolate_value(&mut resolved)?;
    server_section(&resolved, boot)?;
    workshop_section(&resolved, boot)?;
    if let Some(leaf) = leaf {
        validate_chain(leaf, boot, &document)?;
    }
    write_shadow_repr(boot, &render_toml(&document)?)
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

/// Resolves the chain from `leaf` with shadows preferred and `candidate`
/// standing in for its target file, then runs full config validation on
/// the merged result.
fn validate_chain(leaf: &Path, target: &Path, candidate: &Value) -> Result<(), Repr> {
    let read = shadow_reader(Some((target, candidate)));
    let (merged, _chain, _provenance) = collect_config_chain_with(leaf, &read)?;
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
