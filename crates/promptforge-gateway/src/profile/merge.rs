//! Profile document merging: overlay resolution, keyed-array merges by
//! `id`/`name`, device-lane attachment, and merge-error location diagnostics.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use toml::Value;

use crate::error::ConfigError;

/// Merges `overlay` into `base` (later wins for scalars; arrays merge by id/name).
///
/// `path` is the file that produced `overlay`, used in merge error locations.
pub(super) fn merge_docs(base: &mut Value, overlay: Value, path: &Path) -> Result<(), ConfigError> {
    let (Value::Table(base_table), Value::Table(overlay_table)) = (base, overlay) else {
        return Err(ConfigError::Validation(format!(
            "{}: profile root must be a table",
            loc(path, None)
        )));
    };

    for (key, overlay_val) in overlay_table {
        match key.as_str() {
            "endpoint" | "model" | "local_model" => {
                let entry = base_table
                    .entry(key.clone())
                    .or_insert_with(|| Value::Array(Vec::new()));
                merge_keyed_array(entry, overlay_val, identity_key(&key)?, path, &key)?;
            }
            "device" => {
                let entry = base_table
                    .entry(key.clone())
                    .or_insert_with(|| Value::Array(Vec::new()));
                merge_device_overlay(entry, overlay_val, path)?;
            }
            "include" => {
                // Already consumed by the loader; ignore if somehow present.
            }
            _ => match base_table.get_mut(&key) {
                Some(base_val) if base_val.is_table() && overlay_val.is_table() => {
                    merge_tables(base_val, overlay_val, path)?;
                }
                _ => {
                    base_table.insert(key, overlay_val);
                }
            },
        }
    }
    Ok(())
}

fn identity_key(array_name: &str) -> Result<&'static str, ConfigError> {
    match array_name {
        "endpoint" | "device" => Ok("id"),
        "model" | "local_model" => Ok("name"),
        other => Err(ConfigError::Validation(format!(
            "internal: unknown keyed array {other}"
        ))),
    }
}

/// Merge `[[device]]` arrays, or attach orphan `[[device.lane]]` tables.
///
/// A leaf file with only `[[device.lane]]` (no `[[device]]` in that file) parses
/// as a table `{ lane = [...] }` rather than an array. Those lanes attach to the
/// parent device named by each lane's `device` field.
fn merge_device_overlay(base: &mut Value, overlay: Value, path: &Path) -> Result<(), ConfigError> {
    match overlay {
        Value::Array(_) => merge_keyed_array(base, overlay, "id", path, "device"),
        Value::Table(table) => {
            let Some(lanes) = table.get("lane").cloned() else {
                return Err(merge_type_error(
                    path,
                    "device",
                    "array of tables or [[device.lane]]",
                    &Value::Table(table),
                    "[[device]]",
                ));
            };
            if table.keys().any(|k| k != "lane") {
                return Err(merge_type_error(
                    path,
                    "device",
                    "array of tables or [[device.lane]]",
                    &Value::Table(table),
                    "[[device]]",
                ));
            }
            attach_orphan_device_lanes(base, lanes, path)
        }
        other => Err(merge_type_error(
            path,
            "device",
            "array of tables or [[device.lane]]",
            &other,
            "[[device]]",
        )),
    }
}

fn attach_orphan_device_lanes(
    base_devices: &mut Value,
    lanes: Value,
    path: &Path,
) -> Result<(), ConfigError> {
    let Value::Array(lane_items) = lanes else {
        return Err(merge_type_error(
            path,
            "device.lane",
            "array of tables",
            &lanes,
            "[[device.lane]]",
        ));
    };
    // A malformed inherited `device` value that is not an array is surfaced as a
    // located error rather than silently coerced to an empty array that erases
    // the inherited state (PROFILE-001).
    if !base_devices.is_array() {
        return Err(merge_type_error(
            path,
            "device",
            "array of tables",
            base_devices,
            "[[device]]",
        ));
    }
    let Value::Array(devices) = base_devices else {
        unreachable!("just ensured array");
    };

    for lane in lane_items {
        let device_id = lane
            .get("device")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ConfigError::Validation(format!(
                    "{}: [[device.lane]] without a sibling [[device]] in this file needs device = \"...\" to attach to a parent device",
                    loc(path, line_of_header(path, "[[device.lane]]"))
                ))
            })?
            .to_owned();
        let idx = devices
            .iter()
            .position(|d| item_key(d, "id").as_deref() == Some(device_id.as_str()))
            .ok_or_else(|| {
                ConfigError::Validation(format!(
                    "{}: [[device.lane]] names undefined device {device_id}",
                    loc(path, line_of_header(path, "[[device.lane]]"))
                ))
            })?;
        let mut device = devices[idx].clone();
        {
            let Value::Table(device_table) = &mut device else {
                return Err(ConfigError::Validation(format!(
                    "{}: device {device_id} must be a table",
                    loc(path, line_of_header(path, "[[device]]"))
                )));
            };
            let lane_array = device_table
                .entry("lane".to_owned())
                .or_insert_with(|| Value::Array(Vec::new()));
            merge_keyed_array(
                lane_array,
                Value::Array(vec![lane]),
                "id",
                path,
                "device.lane",
            )?;
        }
        devices[idx] = device;
    }
    Ok(())
}

fn merge_tables(base: &mut Value, overlay: Value, path: &Path) -> Result<(), ConfigError> {
    let (Value::Table(base_table), Value::Table(overlay_table)) = (base, overlay) else {
        return Err(ConfigError::Validation(format!(
            "{}: expected tables while merging profile scalars",
            loc(path, None)
        )));
    };
    for (key, overlay_val) in overlay_table {
        // Nested [[device.lane]] lands as device[].lane arrays.
        if key == "lane" {
            let entry = base_table
                .entry(key.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            merge_keyed_array(entry, overlay_val, "id", path, "device.lane")?;
            continue;
        }
        match base_table.get_mut(&key) {
            Some(base_val) if base_val.is_table() && overlay_val.is_table() => {
                merge_tables(base_val, overlay_val, path)?;
            }
            _ => {
                base_table.insert(key, overlay_val);
            }
        }
    }
    Ok(())
}

fn merge_keyed_array(
    base: &mut Value,
    overlay: Value,
    key_field: &str,
    path: &Path,
    array_name: &str,
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
        // Device entries may carry nested lane arrays; merge those if replacing.
        if let Some(k) = item_key(&item, key_field) {
            if let Some(&idx) = index_by_key.get(&k) {
                if array_name == "device" && item.get("lane").is_some() {
                    let mut existing = base_items[idx].clone();
                    merge_device_entry(&mut existing, item, path)?;
                    base_items[idx] = existing;
                } else {
                    base_items[idx] = item;
                }
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

/// Merges a later `[[device]]` definition over an earlier one with the same id.
///
/// Contract (PROFILE-002): the later definition's scalar fields fully replace
/// the earlier ones - a field the earlier device set but the later one omits is
/// dropped, never silently retained as stale state. Lane arrays are the one
/// exception: they union by lane `id` so lanes factored across include files
/// accumulate instead of clobbering each other.
fn merge_device_entry(base: &mut Value, overlay: Value, path: &Path) -> Result<(), ConfigError> {
    let (Value::Table(base_table), Value::Table(mut overlay_table)) = (base, overlay) else {
        return Err(ConfigError::Validation(format!(
            "{}: device entries must be tables",
            loc(path, None)
        )));
    };
    let base_lanes = base_table.remove("lane");
    let overlay_lanes = overlay_table.remove("lane");
    // Replace all scalar fields wholesale with the later definition.
    *base_table = overlay_table;
    // Union the lanes: start from the earlier lanes (if any) and merge the later
    // lanes by id; a present-but-non-array lane collection is a located error.
    let mut lanes = base_lanes.unwrap_or_else(|| Value::Array(Vec::new()));
    if let Some(overlay_lanes) = overlay_lanes {
        merge_keyed_array(&mut lanes, overlay_lanes, "id", path, "device.lane")?;
    } else if !lanes.is_array() {
        return Err(merge_type_error(
            path,
            "device.lane",
            "array of tables",
            &lanes,
            "[[device.lane]]",
        ));
    }
    if lanes.as_array().is_some_and(|items| !items.is_empty()) {
        base_table.insert("lane".to_owned(), lanes);
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

fn line_of_header(path: &Path, header: &str) -> Option<usize> {
    let raw = fs::read_to_string(path).ok()?;
    line_of_header_in(&raw, header)
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
    use super::*;

    #[test]
    fn merge_keyed_array_rejects_non_array_inherited_state() {
        // PROFILE-001: an inherited (already-merged) value that is not an array
        // is surfaced as a located error, never silently coerced to an empty
        // array that discards the malformed state.
        let mut base = Value::String("oops".to_owned());
        let overlay = Value::Array(Vec::new());
        let err = merge_keyed_array(&mut base, overlay, "id", Path::new("p.toml"), "model")
            .expect_err("non-array inherited base must error");
        assert!(matches!(err, ConfigError::Validation(_)));
        assert!(err.to_string().contains("model"));
    }

    #[test]
    fn attach_orphan_device_lanes_rejects_non_array_inherited_devices() {
        // PROFILE-001: a malformed inherited `device` collection (not an array)
        // that receives an orphan `[[device.lane]]` overlay is a located error,
        // never silently coerced to an empty array that erases the inherited
        // state.
        let mut base = Value::String("oops".to_owned());
        let lane: Value = toml::from_str("device = 'gpu'\nid = 'l1'").expect("parse lane");
        let lanes = Value::Array(vec![lane]);
        let err = attach_orphan_device_lanes(&mut base, lanes, Path::new("p.toml"))
            .expect_err("non-array inherited devices must error");
        assert!(matches!(err, ConfigError::Validation(_)));
        assert!(err.to_string().contains("device"), "{err}");
    }

    #[test]
    fn device_overlay_replaces_scalars_and_unions_lanes() {
        // PROFILE-002: the later device definition's scalar fields fully replace
        // the earlier ones (no stale-field retention), while lanes union by id.
        let mut base: Value =
            toml::from_str("id = 'gpu'\nconcurrency = 10\ngone = 'stale'\n[[lane]]\nid = 'a'\n")
                .expect("parse base device");
        let overlay: Value = toml::from_str("id = 'gpu'\ntype = 'local'\n[[lane]]\nid = 'b'\n")
            .expect("parse overlay device");
        merge_device_entry(&mut base, overlay, Path::new("p.toml")).expect("merge device");
        let table = base.as_table().expect("device table");
        // The earlier `concurrency`/`gone` scalars the later def omitted are gone.
        assert!(table.get("concurrency").is_none(), "stale scalar retained");
        assert!(table.get("gone").is_none(), "stale scalar retained");
        assert_eq!(table.get("type").and_then(Value::as_str), Some("local"));
        // Lanes union by id: both `a` (earlier) and `b` (later) survive.
        let lane_ids: Vec<&str> = table
            .get("lane")
            .and_then(Value::as_array)
            .expect("lanes")
            .iter()
            .filter_map(|lane| lane.get("id").and_then(Value::as_str))
            .collect();
        assert!(
            lane_ids.contains(&"a") && lane_ids.contains(&"b"),
            "{lane_ids:?}"
        );
    }
}
