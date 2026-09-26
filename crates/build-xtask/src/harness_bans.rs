//! Harness clippy-ban check: every internal harness crate forbids raw
//! tokio spawns.
//!
//! The harness spawns only through one instrumented wrapper in
//! `harness-runner` that tags each task with its `EffectId` and
//! `Provenance`, so every crate under `crates/harness-internal/` has a
//! `clippy.toml` whose `disallowed-methods` names `tokio::spawn` and
//! `tokio::task::spawn_blocking`. Clippy reads the nearest `clippy.toml`
//! above each crate's manifest directory, so the file must sit in the crate
//! itself, not only at the workspace root. The `crates/harness/` facade
//! holds only re-exports, so it has no `clippy.toml` and is not checked.
//!
//! The check is vacuously true while the container is empty or absent.

use std::fs;
use std::path::{Path, PathBuf};

/// The methods every harness `clippy.toml` must disallow.
const BANNED: [&str; 2] = ["tokio::spawn", "tokio::task::spawn_blocking"];

/// Checks every crate under `container` for a complete clippy ban list.
#[must_use]
pub(crate) fn harness_clippy_bans(container: &Path) -> Vec<String> {
    let mut crates = Vec::new();
    collect_crates(container, &mut crates);
    crates.iter().filter_map(|dir| check_crate(dir)).collect()
}

/// The harness family's crate directories: every crate under `container`
/// plus `public_crate` when its directory exists.
#[must_use]
pub(crate) fn harness_crates(container: &Path, public_crate: &Path) -> Vec<PathBuf> {
    let mut crates = Vec::new();
    collect_crates(container, &mut crates);
    if public_crate.is_dir() {
        crates.push(public_crate.to_path_buf());
    }
    crates
}

/// Every crate directory under `dir`: a directory holding a `Cargo.toml`
/// is a crate and is not descended into; any other directory is a
/// container and the walk descends.
fn collect_crates(dir: &Path, crates: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let sub = entry.path();
        if !sub.is_dir() {
            continue;
        }
        if sub.join("Cargo.toml").exists() {
            crates.push(sub);
        } else {
            collect_crates(&sub, crates);
        }
    }
}

/// The violation for one crate directory, or `None` when its
/// `clippy.toml` names every banned method.
fn check_crate(dir: &Path) -> Option<String> {
    let path = dir.join("clippy.toml");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return Some(format!(
                "{}: {}; every harness crate has a clippy.toml whose disallowed-methods names {}",
                path.display(),
                if path.exists() {
                    format!("unreadable clippy.toml: {error}")
                } else {
                    "missing clippy.toml".to_owned()
                },
                BANNED.join(" and ")
            ));
        }
    };
    let value: toml::Value = match toml::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return Some(format!(
                "{}: unparseable clippy.toml: {error}",
                path.display()
            ));
        }
    };
    let named = disallowed_methods(&value);
    let missing: Vec<&str> = BANNED
        .iter()
        .copied()
        .filter(|method| !named.contains(method))
        .collect();
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "{}: disallowed-methods lacks {}",
        path.display(),
        missing.join(", ")
    ))
}

/// The method paths a `clippy.toml` disallows, in either entry form: a
/// bare string or a table with a `path` key.
fn disallowed_methods(value: &toml::Value) -> Vec<&str> {
    value
        .get("disallowed-methods")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .as_str()
                .or_else(|| entry.get("path").and_then(toml::Value::as_str))
        })
        .collect()
}

#[cfg(test)]
#[path = "harness_bans-tests.rs"]
mod tests;
