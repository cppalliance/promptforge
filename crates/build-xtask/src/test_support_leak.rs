//! `test-support` leak guard: no non-dev dependency table anywhere in the
//! workspace enables a promptforge or harness crate's `test-support`
//! feature. The promptforge family is the facade plus every
//! `crates/promptforge-internal/` member; the harness family is every
//! `crates/harness/` member plus `crates/harness-api`.
//!
//! The engine manifest guard (`engine_deps`) lets an engine crate keep an
//! optional forbidden dependency that only its `test-support` feature
//! enables (`promptforge-engine`'s tokio test driver), and harness crates
//! keep test fixtures behind theirs (`harness-runner`'s). Both are safe
//! only while `test-support` is enabled from `[dev-dependencies]`
//! alone: a production `[dependencies]` entry such as
//! `promptforge-engine = { workspace = true, features = ["test-support"] }`
//! would pull the test drivers back into a shipping binary. This guard closes
//! that path. It scans every crate under `crates/` (containers included),
//! the `[dependencies]` and `[build-dependencies]` tables and their
//! `[target.<cfg>]` forms, and the root manifest's
//! `[workspace.dependencies]` table, which every `workspace = true` entry
//! inherits. `[dev-dependencies]` tables are outside the guard.
//!
//! It also scans each crate's `[features]` table: a value of the form
//! `<dep>/test-support` or `<dep>?/test-support` enables the feature on
//! a non-dev dependency (cargo forbids `[features]` from naming
//! dev-dependencies), so `default = ["promptforge-engine/test-support"]`
//! is the same leak spelled through a feature. `<dep>` is resolved through
//! the crate's own dependency entries and their `package` renames. One
//! shape is exempt, within each family: a crate's own `test-support`
//! feature forwarding to another `test-support` in its family (one
//! container crate forwarding a sibling's; the guard counts the facade and
//! `harness-api` as members of their families), because that forwarding
//! is gated by a feature this guard already confines to dev tables.
//! Forwarding into the other family is reported.
//!
//! The check reads declared dependencies, not the resolved graph, so
//! `workspace-hack` unification is irrelevant to it. Manifests that cannot
//! be read or parsed are skipped here; the product-boundary check and the
//! engine manifest guard already report them.

use std::fs;
use std::path::{Path, PathBuf};

/// The dependency tables the guard scans, directly and under `[target]`.
const CHECKED_KINDS: [&str; 2] = ["dependencies", "build-dependencies"];

/// The feature no non-dev table may enable on a guarded crate.
const GUARDED_FEATURE: &str = crate::engine_deps::EXEMPTING_FEATURE;

/// Scans the workspace for non-dev dependency tables, and `[features]`
/// values, that enable a promptforge or harness crate's `test-support`
/// feature.
#[must_use]
pub(crate) fn test_support_leak_violations(root: &Path) -> Vec<String> {
    let families = guarded_families(root);
    if families.iter().all(Vec::is_empty) {
        return Vec::new();
    }
    let mut violations = Vec::new();
    let root_manifest = root.join("Cargo.toml");
    if let Some(manifest) = parse_manifest(&root_manifest)
        && let Some(table) = manifest
            .get("workspace")
            .and_then(|w| w.get("dependencies"))
            .and_then(toml::Value::as_table)
    {
        scan_table(
            &root_manifest,
            "workspace.dependencies",
            table,
            &families,
            &mut violations,
        );
    }
    let mut crates = Vec::new();
    crate::engine_guards::collect_crates(&root.join("crates"), &mut crates);
    for dir in crates {
        let manifest_path = dir.join("Cargo.toml");
        let Some(manifest) = parse_manifest(&manifest_path) else {
            continue;
        };
        let tables = crate::manifest::dependency_tables(&manifest, &CHECKED_KINDS);
        for (table, entries) in &tables {
            scan_table(&manifest_path, table, entries, &families, &mut violations);
        }
        scan_features(
            &manifest_path,
            &manifest,
            &tables,
            &families,
            &mut violations,
        );
    }
    violations
}

/// Reports every `[features]` value that enables the guarded feature on a
/// guarded crate through a dependency-feature reference.
fn scan_features(
    manifest_path: &Path,
    manifest: &toml::Value,
    tables: &[(String, &toml::map::Map<String, toml::Value>)],
    families: &[Vec<String>],
    violations: &mut Vec<String>,
) {
    let Some(features) = manifest.get("features").and_then(toml::Value::as_table) else {
        return;
    };
    let self_family = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .and_then(|name| family_of(name, families));
    for (feature, values) in features {
        let Some(values) = values.as_array() else {
            continue;
        };
        for dep in values
            .iter()
            .filter_map(toml::Value::as_str)
            .filter_map(guarded_dependency_reference)
        {
            let package = resolve_package(dep, tables);
            let Some(family) = family_of(package, families) else {
                continue;
            };
            // A crate's own `test-support` forwarding to a family sibling's
            // is gated by a feature the dependency-table scan already
            // confines to dev tables, so any other feature leaks.
            // Cross-family forwarding is reported because the exemption
            // applies within each family only.
            if feature == GUARDED_FEATURE && self_family == Some(family) {
                continue;
            }
            violations.push(format!(
                "{}: [features] {feature} enables {package}/{GUARDED_FEATURE}; only [dev-dependencies] may enable a promptforge or harness crate's {GUARDED_FEATURE} feature",
                manifest_path.display()
            ));
        }
    }
}

/// The dependency key a `[features]` value names when it enables the
/// guarded feature: `<dep>/test-support` or `<dep>?/test-support`.
fn guarded_dependency_reference(value: &str) -> Option<&str> {
    let (dep, feature) = value.split_once('/')?;
    (feature == GUARDED_FEATURE).then(|| dep.strip_suffix('?').unwrap_or(dep))
}

/// The package a dependency key names: its `package` rename when the
/// key appears in one of the crate's dependency tables, else the key.
fn resolve_package<'a>(
    key: &'a str,
    tables: &[(String, &'a toml::map::Map<String, toml::Value>)],
) -> &'a str {
    tables
        .iter()
        .find_map(|(_, entries)| entries.get(key))
        .and_then(|entry| entry.get("package").and_then(toml::Value::as_str))
        .unwrap_or(key)
}

/// Reports every entry in `table` that names a guarded crate and lists the
/// guarded feature.
fn scan_table(
    manifest: &Path,
    table: &str,
    entries: &toml::map::Map<String, toml::Value>,
    families: &[Vec<String>],
    violations: &mut Vec<String>,
) {
    for (key, entry) in entries {
        let package = entry
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(key);
        if family_of(package, families).is_none() {
            continue;
        }
        let enables = entry
            .get("features")
            .and_then(toml::Value::as_array)
            .is_some_and(|features| {
                features
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .any(|feature| feature == GUARDED_FEATURE)
            });
        if enables {
            violations.push(format!(
                "{}: [{table}] enables {package}/{GUARDED_FEATURE}; only [dev-dependencies] may enable a promptforge or harness crate's {GUARDED_FEATURE} feature",
                manifest.display()
            ));
        }
    }
}

/// The index of the family in `families` that names `package`, or `None`
/// when no guarded family does.
fn family_of(package: &str, families: &[Vec<String>]) -> Option<usize> {
    families
        .iter()
        .position(|names| names.iter().any(|name| name == package))
}

/// The package names of each guarded family's crates whose manifests
/// parse: the promptforge crates, then the harness crates.
fn guarded_families(root: &Path) -> [Vec<String>; 2] {
    let crates_dir = root.join("crates");
    [
        package_names(&crate::engine_guards::engine_crates(root)),
        package_names(&crate::harness_bans::harness_crates(
            &crates_dir.join("harness"),
            &crates_dir.join("harness-api"),
        )),
    ]
}

/// The package names of every crate in `dirs` whose manifest parses.
fn package_names(dirs: &[PathBuf]) -> Vec<String> {
    dirs.iter()
        .filter_map(|dir| parse_manifest(&dir.join("Cargo.toml")))
        .filter_map(|manifest| {
            manifest
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

/// Reads and parses one manifest, or `None` when it cannot be read or parsed.
fn parse_manifest(path: &Path) -> Option<toml::Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
}

#[cfg(test)]
#[path = "test_support_leak-tests.rs"]
mod tests;
