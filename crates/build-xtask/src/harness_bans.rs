//! Harness tokio ban: no Harness crate declares `tokio` or `tokio-util` as
//! a normal dependency.
//!
//! The Harness polls every effect inside the run's own future and starts
//! no task, so any executor can drive a run. The `crates/harness/` facade
//! and every crate under `crates/harness-internal/` therefore may not name
//! `tokio` or `tokio-util` in `[dependencies]` or its target-specific
//! forms. `[dev-dependencies]` are outside the ban: the suites and doc
//! examples drive runs on tokio, as a real Host does.
//!
//! The check reads declared dependencies, not the resolved graph, so
//! `workspace-hack` unification is irrelevant to it. It is vacuously true
//! while the container is empty or absent and the facade is absent.

use std::fs;
use std::path::{Path, PathBuf};

/// The crates a Harness manifest may not declare outside
/// `[dev-dependencies]`.
const BANNED: [&str; 2] = ["tokio", "tokio-util"];

/// The dependency tables the ban scans, directly and under `[target]`.
const CHECKED_KINDS: [&str; 1] = ["dependencies"];

/// Checks every Harness crate's manifest for a banned normal dependency.
#[must_use]
pub(crate) fn harness_tokio_bans(container: &Path, public_crate: &Path) -> Vec<String> {
    harness_crates(container, public_crate)
        .iter()
        .flat_map(|dir| check_manifest(&dir.join("Cargo.toml")))
        .collect()
}

/// The Harness family's crate directories: every crate under `container`
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

/// The violations in one manifest: one per banned entry, or one when the
/// manifest cannot be read or parsed.
fn check_manifest(manifest: &Path) -> Vec<String> {
    let text = match fs::read_to_string(manifest) {
        Ok(text) => text,
        Err(error) => {
            return vec![format!(
                "{}: unreadable manifest: {error}",
                manifest.display()
            )];
        }
    };
    let value: toml::Value = match toml::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return vec![format!(
                "{}: unparseable manifest: {error}",
                manifest.display()
            )];
        }
    };
    let mut violations = Vec::new();
    for (table, entries) in crate::manifest::dependency_tables(&value, &CHECKED_KINDS) {
        for (key, entry) in entries {
            let package = entry
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(key);
            if BANNED.contains(&package) {
                violations.push(format!(
                    "{}: [{table}] declares {package}; harness crates declare {} only under [dev-dependencies]",
                    manifest.display(),
                    BANNED.join(" and ")
                ));
            }
        }
    }
    violations
}

#[cfg(test)]
#[path = "harness_bans-tests.rs"]
mod tests;
