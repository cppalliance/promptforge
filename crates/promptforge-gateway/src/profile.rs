//! Named profiles: recursive `include` resolution and the profiles directory.
//!
//! A profile is a TOML file under `~/.promptforge/profiles/` (or a path passed
//! to `serve`). Includes resolve depth-first relative to the including file;
//! later definitions replace earlier ones with the same `id` or `name`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use toml::Value;

use crate::config::Config;
use crate::error::ConfigError;
use crate::local::artifacts::default_promptforge_root;

/// Maximum `include` nesting depth (guards against runaway trees).
pub const MAX_INCLUDE_DEPTH: usize = 16;

/// Default directory for named profiles: `~/.promptforge/profiles`.
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
pub fn list_profiles(dir: &Path) -> Result<Vec<String>, ConfigError> {
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
        path: dir.display().to_string(),
        source,
    })? {
        let entry = entry.map_err(|source| ConfigError::Read {
            path: dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            names.push(stem.to_owned());
        }
    }
    names.sort_unstable();
    Ok(names)
}

/// Loads `dir/name.toml` with recursive include resolution.
///
/// # Errors
/// Returns [`ConfigError`] when the file is missing, includes cycle or exceed
/// depth, merge fails, or the resolved document fails config validation.
pub fn load_named(dir: &Path, name: &str) -> Result<Config, ConfigError> {
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
        return Err(ConfigError::Validation(format!(
            "invalid profile name {name:?}"
        )));
    }
    let path = dir.join(format!("{name}.toml"));
    load_path(&path)
}

/// Loads a profile TOML path with recursive include resolution.
///
/// # Errors
/// Returns [`ConfigError`] on read, include, parse, or validation failure.
pub fn load_path(path: &Path) -> Result<Config, ConfigError> {
    let mut stack = Vec::new();
    let mut visiting = HashSet::new();
    let value = load_value(path, 0, &mut stack, &mut visiting)?;
    let raw = toml::to_string(&value).map_err(|e| ConfigError::Parse(e.to_string()))?;
    Config::from_toml_str(&raw)
}

fn load_value(
    path: &Path,
    depth: usize,
    stack: &mut Vec<PathBuf>,
    visiting: &mut HashSet<PathBuf>,
) -> Result<Value, ConfigError> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(ConfigError::IncludeDepth {
            path: path.display().to_string(),
            max: MAX_INCLUDE_DEPTH,
        });
    }

    let canonical = canonicalize_for_cycle(path);
    if !visiting.insert(canonical.clone()) {
        let chain = stack
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" -> ");
        return Err(ConfigError::IncludeCycle {
            path: canonical.display().to_string(),
            chain,
        });
    }
    stack.push(canonical.clone());

    let raw = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let mut doc: Value =
        toml::from_str(&raw).map_err(|e| ConfigError::Parse(format!("{}: {e}", path.display())))?;

    let includes = take_includes(&mut doc)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

    let mut merged = Value::Table(toml::map::Map::new());
    for include_name in &includes {
        let include_path = resolve_include(base_dir, include_name)?;
        let parent_doc = load_value(&include_path, depth + 1, stack, visiting)?;
        merge_docs(&mut merged, parent_doc)?;
    }
    merge_docs(&mut merged, doc)?;

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
            path: resolved.display().to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "include not found"),
        });
    }
    Ok(resolved)
}

/// Merges `overlay` into `base` (later wins for scalars; arrays merge by id/name).
fn merge_docs(base: &mut Value, overlay: Value) -> Result<(), ConfigError> {
    let (Value::Table(base_table), Value::Table(overlay_table)) = (base, overlay) else {
        return Err(ConfigError::Validation(
            "profile root must be a table".to_string(),
        ));
    };

    for (key, overlay_val) in overlay_table {
        match key.as_str() {
            "endpoint" | "model" | "local_model" | "device" => {
                let entry = base_table
                    .entry(key.clone())
                    .or_insert_with(|| Value::Array(Vec::new()));
                merge_keyed_array(entry, overlay_val, identity_key(&key)?)?;
            }
            "include" => {
                // Already consumed by the loader; ignore if somehow present.
            }
            _ => match base_table.get_mut(&key) {
                Some(base_val) if base_val.is_table() && overlay_val.is_table() => {
                    merge_tables(base_val, overlay_val)?;
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

fn merge_tables(base: &mut Value, overlay: Value) -> Result<(), ConfigError> {
    let (Value::Table(base_table), Value::Table(overlay_table)) = (base, overlay) else {
        return Err(ConfigError::Validation(
            "expected tables while merging profile scalars".to_string(),
        ));
    };
    for (key, overlay_val) in overlay_table {
        // Nested [[device.lane]] lands as device[].lane arrays.
        if key == "lane" {
            let entry = base_table
                .entry(key.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            merge_keyed_array(entry, overlay_val, "id")?;
            continue;
        }
        match base_table.get_mut(&key) {
            Some(base_val) if base_val.is_table() && overlay_val.is_table() => {
                merge_tables(base_val, overlay_val)?;
            }
            _ => {
                base_table.insert(key, overlay_val);
            }
        }
    }
    Ok(())
}

fn merge_keyed_array(base: &mut Value, overlay: Value, key_field: &str) -> Result<(), ConfigError> {
    let Value::Array(overlay_items) = overlay else {
        return Err(ConfigError::Validation(format!(
            "expected array while merging by {key_field}"
        )));
    };
    if !base.is_array() {
        *base = Value::Array(Vec::new());
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
                if key_field == "id" && item.get("lane").is_some() {
                    let mut existing = base_items[idx].clone();
                    merge_device_entry(&mut existing, item)?;
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

fn merge_device_entry(base: &mut Value, overlay: Value) -> Result<(), ConfigError> {
    let (Value::Table(base_table), Value::Table(overlay_table)) = (base, overlay) else {
        return Err(ConfigError::Validation(
            "device entries must be tables".to_string(),
        ));
    };
    for (key, overlay_val) in overlay_table {
        if key == "lane" {
            let entry = base_table
                .entry(key.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            merge_keyed_array(entry, overlay_val, "id")?;
        } else {
            base_table.insert(key, overlay_val);
        }
    }
    Ok(())
}

fn item_key(item: &Value, key_field: &str) -> Option<String> {
    item.get(key_field)?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn include_merges_and_child_overrides_by_name() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "base.toml",
            r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://base"
api_key = ""
concurrency = 2

[[model]]
name = "m1"
description = "from base"
context = 1000
upstream = "u-base"
endpoints = ["anthropic"]
"#,
        );
        write(
            tmp.path(),
            "child.toml",
            r#"
include = ["base.toml"]

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://child"
api_key = ""
concurrency = 9

[[model]]
name = "m1"
description = "from child"
context = 2000
upstream = "u-child"
endpoints = ["anthropic"]

[[model]]
name = "m2"
description = "extra"
context = 3000
upstream = "u2"
endpoints = ["anthropic"]
"#,
        );

        let config = load_path(&tmp.path().join("child.toml")).unwrap();
        assert_eq!(config.endpoints.len(), 1);
        assert_eq!(config.endpoints[0].base_url, "http://child");
        assert_eq!(config.endpoints[0].concurrency, Some(9));
        assert_eq!(config.models.len(), 2);
        assert_eq!(config.models[0].description, "from child");
        assert_eq!(config.models[0].context, 2000);
        assert_eq!(config.models[1].name, "m2");
    }

    #[test]
    fn include_paths_are_relative_to_including_file() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("nested");
        fs::create_dir(&nested).unwrap();
        write(
            &nested,
            "base.toml",
            r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#,
        );
        write(
            tmp.path(),
            "root.toml",
            r#"
include = ["nested/base.toml"]
"#,
        );
        let config = load_path(&tmp.path().join("root.toml")).unwrap();
        assert_eq!(config.models[0].name, "m");
    }

    #[test]
    fn detects_include_cycles() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "a.toml", r#"include = ["b.toml"]"#);
        write(tmp.path(), "b.toml", r#"include = ["a.toml"]"#);
        let err = load_path(&tmp.path().join("a.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::IncludeCycle { .. }));
    }

    #[test]
    fn rejects_runaway_include_depth() {
        let tmp = TempDir::new().unwrap();
        // Root at depth 0 plus MAX_INCLUDE_DEPTH+1 nested includes exceeds the cap.
        let last = MAX_INCLUDE_DEPTH + 1;
        for i in 0..=last {
            let body = if i == last {
                r#"
[server]
bind = "127.0.0.1:8081"
token = "t"
"#
                .to_owned()
            } else {
                format!("include = [\"n{}.toml\"]", i + 1)
            };
            write(tmp.path(), &format!("n{i}.toml"), &body);
        }
        let err = load_path(&tmp.path().join("n0.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::IncludeDepth { .. }));
    }

    #[test]
    fn later_scalar_wins() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "base.toml",
            r#"
[server]
bind = "127.0.0.1:8081"
token = "base-token"

[queue]
max_depth = 10
"#,
        );
        write(
            tmp.path(),
            "child.toml",
            r#"
include = ["base.toml"]

[server]
token = "child-token"

[queue]
max_depth = 50
"#,
        );
        let config = load_path(&tmp.path().join("child.toml")).unwrap();
        assert_eq!(config.server.token.expose(), "child-token");
        assert_eq!(config.queue.max_depth, 50);
        assert_eq!(config.server.bind.to_string(), "127.0.0.1:8081");
    }

    #[test]
    fn list_profiles_returns_sorted_stems() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "threat.toml",
            "[server]\nbind=\"127.0.0.1:1\"\ntoken=\"t\"\n",
        );
        write(
            tmp.path(),
            "analytical.toml",
            "[server]\nbind=\"127.0.0.1:1\"\ntoken=\"t\"\n",
        );
        write(tmp.path(), "notes.txt", "ignore");
        let names = list_profiles(tmp.path()).unwrap();
        assert_eq!(names, vec!["analytical", "threat"]);
    }

    #[test]
    fn load_named_reads_from_profiles_dir() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "alpha.toml",
            r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#,
        );
        let config = load_named(tmp.path(), "alpha").unwrap();
        assert_eq!(config.models[0].name, "m");
    }

    #[test]
    fn local_model_override_by_name() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "base.toml",
            r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[local_model]]
name = "q"
description = "base"
source = "https://example.com/a.gguf"
context = 1024
"#,
        );
        write(
            tmp.path(),
            "child.toml",
            r#"
include = ["base.toml"]

[[local_model]]
name = "q"
description = "child"
source = "https://example.com/b.gguf"
context = 2048
"#,
        );
        let config = load_path(&tmp.path().join("child.toml")).unwrap();
        assert_eq!(config.local_models.len(), 1);
        assert_eq!(config.local_models[0].description, "child");
        assert_eq!(config.local_models[0].context, 2048);
    }

    #[test]
    fn include_merges_devices_and_nested_lanes() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "base.toml",
            r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[device]]
id = "anthropic"
type = "remote"
concurrency = 10

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""
device = "anthropic"

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["anthropic"]
"#,
        );
        write(
            tmp.path(),
            "child.toml",
            r#"
include = ["base.toml"]

[[device]]
id = "local-gpu"
type = "local"

[[device.lane]]
device = "local-gpu"
id = "generative"
concurrency = 1
"#,
        );
        let config = load_path(&tmp.path().join("child.toml")).unwrap();
        assert_eq!(config.devices.len(), 2);
        let local = config.devices.iter().find(|d| d.id == "local-gpu").unwrap();
        assert_eq!(local.lanes.len(), 1);
        assert_eq!(local.lanes[0].id, "generative");
    }
}
