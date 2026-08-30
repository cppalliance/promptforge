//! Profile document merging: overlay resolution, keyed-array merges by
//! `id`/`name`, and merge-error location diagnostics.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use toml::Value;

use super::provenance::Provenance;
use crate::error::ConfigError;

/// Merges `overlay` into `base` (later wins for scalars; arrays merge by id/name).
///
/// `path` is the file that produced `overlay`, used in merge error locations
/// and recorded into `provenance` as the origin of everything the overlay
/// writes. The recording is a side channel: it never changes the merge.
pub(super) fn merge_docs(
    base: &mut Value,
    overlay: Value,
    path: &Path,
    provenance: &mut Provenance,
) -> Result<(), ConfigError> {
    let (Value::Table(base_table), Value::Table(overlay_table)) = (base, overlay) else {
        return Err(ConfigError::Validation(format!(
            "{}: profile root must be a table",
            loc(path, None)
        )));
    };

    for (key, overlay_val) in overlay_table {
        match key.as_str() {
            "endpoint" | "model" | "local_model" | "dominion" => {
                let entry = base_table
                    .entry(key.clone())
                    .or_insert_with(|| Value::Array(Vec::new()));
                merge_keyed_array(
                    entry,
                    overlay_val,
                    identity_key(&key)?,
                    path,
                    &key,
                    provenance,
                )?;
            }
            "include" => {
                // Already consumed by the loader; ignore if somehow present.
            }
            _ => match base_table.get_mut(&key) {
                Some(base_val) if base_val.is_table() && overlay_val.is_table() => {
                    merge_tables(base_val, overlay_val, path, &key, provenance)?;
                }
                _ => {
                    record_subtree(&key, &overlay_val, path, provenance);
                    base_table.insert(key, overlay_val);
                }
            },
        }
    }
    Ok(())
}

fn identity_key(array_name: &str) -> Result<&'static str, ConfigError> {
    match array_name {
        "endpoint" | "dominion" => Ok("id"),
        "model" | "local_model" => Ok("name"),
        other => Err(ConfigError::Validation(format!(
            "internal: unknown keyed array {other}"
        ))),
    }
}

fn merge_tables(
    base: &mut Value,
    overlay: Value,
    path: &Path,
    prefix: &str,
    provenance: &mut Provenance,
) -> Result<(), ConfigError> {
    let (Value::Table(base_table), Value::Table(overlay_table)) = (base, overlay) else {
        return Err(ConfigError::Validation(format!(
            "{}: expected tables while merging profile scalars",
            loc(path, None)
        )));
    };
    for (key, overlay_val) in overlay_table {
        let dotted = format!("{prefix}.{key}");
        match base_table.get_mut(&key) {
            Some(base_val) if base_val.is_table() && overlay_val.is_table() => {
                merge_tables(base_val, overlay_val, path, &dotted, provenance)?;
            }
            _ => {
                record_subtree(&dotted, &overlay_val, path, provenance);
                base_table.insert(key, overlay_val);
            }
        }
    }
    Ok(())
}

/// Record `prefix` and, when the written value is a table, every path beneath
/// it: a wholesale insert means the entire subtree came from `source`.
fn record_subtree(prefix: &str, value: &Value, source: &Path, provenance: &mut Provenance) {
    provenance.record_path(prefix, source);
    if let Value::Table(table) = value {
        for (key, child) in table {
            record_subtree(&format!("{prefix}.{key}"), child, source, provenance);
        }
    }
}

fn merge_keyed_array(
    base: &mut Value,
    overlay: Value,
    key_field: &str,
    path: &Path,
    array_name: &str,
    provenance: &mut Provenance,
) -> Result<(), ConfigError> {
    let Value::Array(overlay_items) = overlay else {
        return Err(merge_type_error(
            path,
            array_name,
            "array of tables",
            &overlay,
            &format!("[[{array_name}]]"),
        ));
    };
    // A previously-merged (inherited) value under this key that is not an array
    // is malformed state. Surface a located type error rather than silently
    // discarding it and replacing it with an empty array (PROFILE-001).
    if !base.is_array() {
        return Err(merge_type_error(
            path,
            array_name,
            "array of tables",
            base,
            &format!("[[{array_name}]]"),
        ));
    }
    let Value::Array(base_items) = base else {
        unreachable!("just ensured array");
    };

    let mut index_by_key: HashMap<String, usize> = HashMap::new();
    for (idx, item) in base_items.iter().enumerate() {
        if let Some(k) = item_key(item, key_field) {
            index_by_key.insert(k, idx);
        }
    }

    for item in overlay_items {
        if let Some(k) = item_key(&item, key_field) {
            provenance.record_entry(array_name, &k, path);
            if let Some(&idx) = index_by_key.get(&k) {
                base_items[idx] = item;
            } else {
                index_by_key.insert(k, base_items.len());
                base_items.push(item);
            }
        } else {
            base_items.push(item);
        }
    }
    Ok(())
}

fn merge_type_error(
    path: &Path,
    key: &str,
    expected: &str,
    got: &Value,
    header_hint: &str,
) -> ConfigError {
    // Single read of the source, then locate in memory (PROFILE-007): avoids
    // re-reading the file up to three times for one diagnostic.
    let source = fs::read_to_string(path).ok();
    let line = source.as_deref().and_then(|text| {
        line_of_header_in(text, header_hint)
            .or_else(|| line_of_header_in(text, &format!("[[{key}]]")))
            .or_else(|| line_containing_in(text, key))
    });
    ConfigError::Validation(format!(
        "{}: expected {expected} while merging {key}, got {}",
        loc(path, line),
        value_kind(got)
    ))
}

fn loc(path: &Path, line: Option<usize>) -> String {
    match line {
        Some(n) => format!("{}:{n}", path.display()),
        None => path.display().to_string(),
    }
}

fn line_of_header_in(text: &str, header: &str) -> Option<usize> {
    text.lines()
        .enumerate()
        .find(|(_, line)| line.trim_start().starts_with(header))
        .map(|(i, _)| i + 1)
}

fn line_containing_in(text: &str, needle: &str) -> Option<usize> {
    text.lines()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .map(|(i, _)| i + 1)
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Integer(_) => "integer",
        Value::Float(_) => "float",
        Value::Boolean(_) => "boolean",
        Value::Datetime(_) => "datetime",
        Value::Array(_) => "array",
        Value::Table(_) => "table",
    }
}

fn item_key(item: &Value, key_field: &str) -> Option<String> {
    item.get(key_field)?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn merge_keyed_array_rejects_non_array_inherited_state() {
        // PROFILE-001: an inherited (already-merged) value that is not an array
        // is surfaced as a located error, never silently coerced to an empty
        // array that discards the malformed state.
        let mut base = Value::String("oops".to_owned());
        let overlay = Value::Array(Vec::new());
        let err = merge_keyed_array(
            &mut base,
            overlay,
            "id",
            Path::new("p.toml"),
            "model",
            &mut Provenance::default(),
        )
        .expect_err("non-array inherited base must error");
        assert!(matches!(err, ConfigError::Validation(_)));
        assert!(err.to_string().contains("model"));
    }

    #[test]
    fn dominion_is_a_plain_keyed_array() {
        // `[[dominion]]` merges by `id` exactly like `[[endpoint]]` and
        // `[[model]]`: a same-id overlay replaces the earlier definition, and
        // a new id appends without disturbing the inherited entries.
        let mut base: Value = toml::from_str(
            "[[dominion]]\nid = 'pool'\nkind = 'remote'\nmax_concurrency = 2\n\
             [[dominion]]\nid = 'gpu0'\nkind = 'local'\n",
        )
        .expect("parse base");
        let overlay: Value =
            toml::from_str("[[dominion]]\nid = 'pool'\nkind = 'remote'\nmax_concurrency = 8\n")
                .expect("parse overlay");
        merge_docs(
            &mut base,
            overlay,
            Path::new("p.toml"),
            &mut Provenance::default(),
        )
        .expect("merge dominions");
        let dominions = base
            .get("dominion")
            .and_then(Value::as_array)
            .expect("dominion array");
        assert_eq!(dominions.len(), 2, "same id replaces, no duplicate");
        let pool = dominions
            .iter()
            .find(|d| d.get("id").and_then(Value::as_str) == Some("pool"))
            .expect("pool present");
        assert_eq!(
            pool.get("max_concurrency").and_then(Value::as_integer),
            Some(8),
            "later definition wins"
        );
        assert!(
            dominions
                .iter()
                .any(|d| d.get("id").and_then(Value::as_str) == Some("gpu0")),
            "inherited dominion survives"
        );
    }

    #[test]
    fn keyed_array_provenance_tracks_the_winning_file() {
        // The loader merges parents first, so the base file's entries are
        // recorded before the overlay's: base entries name the base file, a
        // replacement and an addition both name the overlay file.
        let mut merged: Value = Value::Table(toml::map::Map::new());
        let mut provenance = Provenance::default();
        merge_docs(
            &mut merged,
            toml::from_str("[[model]]\nname = 'keep'\n\n[[model]]\nname = 'replaced'\n")
                .expect("parse base"),
            Path::new("base.toml"),
            &mut provenance,
        )
        .expect("merge base");
        merge_docs(
            &mut merged,
            toml::from_str("[[model]]\nname = 'replaced'\n\n[[model]]\nname = 'added'\n")
                .expect("parse overlay"),
            Path::new("child.toml"),
            &mut provenance,
        )
        .expect("merge overlay");

        assert_eq!(
            provenance.entry_source("model", "keep"),
            Some(Path::new("base.toml")),
            "an inherited entry names the file that defined it"
        );
        assert_eq!(
            provenance.entry_source("model", "replaced"),
            Some(Path::new("child.toml")),
            "a replaced entry names the file whose definition won"
        );
        assert_eq!(
            provenance.entry_source("model", "added"),
            Some(Path::new("child.toml")),
            "an appended entry names its own file"
        );
    }

    #[test]
    fn table_path_provenance_tracks_each_written_leaf() {
        // A wholesale-inserted table records every path beneath it; a later
        // field-level override re-records just the overridden leaf.
        let mut merged: Value = Value::Table(toml::map::Map::new());
        let mut provenance = Provenance::default();
        merge_docs(
            &mut merged,
            toml::from_str(
                "[server]\nbind = 'a:1'\napi_key = 'k'\n\n[local]\ncache_dir = '/base'\n",
            )
            .expect("parse base"),
            Path::new("base.toml"),
            &mut provenance,
        )
        .expect("merge base");
        merge_docs(
            &mut merged,
            toml::from_str("[local]\ncache_dir = '/child'\n").expect("parse overlay"),
            Path::new("child.toml"),
            &mut provenance,
        )
        .expect("merge overlay");

        let source = |path: &str| {
            provenance
                .paths()
                .find(|(p, _)| *p == path)
                .map(|(_, source)| source.to_path_buf())
        };
        assert_eq!(
            source("server.api_key"),
            Some(PathBuf::from("base.toml")),
            "an inherited scalar keeps the base file"
        );
        assert_eq!(
            source("local"),
            Some(PathBuf::from("base.toml")),
            "the table itself was established by the base file"
        );
        assert_eq!(
            source("local.cache_dir"),
            Some(PathBuf::from("child.toml")),
            "an overridden leaf names the overriding file"
        );
    }
}
