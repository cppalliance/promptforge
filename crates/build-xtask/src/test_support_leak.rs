//! `test-support` leak guard: no non-dev dependency table anywhere in the
//! workspace enables an engine crate's `test-support` feature.
//!
//! The engine manifest guard (`engine_deps`) lets an engine crate keep an
//! optional forbidden dependency that only its `test-support` feature
//! enables (`promptforge-api-runtime`'s tokio test driver). That exemption
//! is safe only while `test-support` is enabled from `[dev-dependencies]`
//! alone: a production `[dependencies]` entry such as
//! `promptforge-api-runtime = { workspace = true, features = ["test-support"] }`
//! would pull the runtime back into a shipping binary. This guard closes
//! that path. It scans every crate under `crates/` (containers included),
//! the `[dependencies]` and `[build-dependencies]` tables and their
//! `[target.<cfg>]` forms, and the root manifest's
//! `[workspace.dependencies]` table, which every `workspace = true` entry
//! inherits. `[dev-dependencies]` tables are outside the guard.
//!
//! It also scans each crate's `[features]` table: a value of the form
//! `<dep>/test-support` or `<dep>?/test-support` enables the feature on
//! a non-dev dependency (cargo forbids `[features]` from naming
//! dev-dependencies), so `default = ["promptforge-api-runtime/test-support"]`
//! is the same leak spelled through a feature. `<dep>` is resolved through
//! the crate's own dependency entries and their `package` renames. One
//! shape is exempt: an engine crate's own `test-support` feature forwarding
//! to a sibling engine crate's `test-support`, because that forwarding is
//! gated by a feature this guard already confines to dev tables.
//!
//! The check reads declared dependencies, not the resolved graph, so
//! `workspace-hack` unification is irrelevant to it. Manifests that cannot
//! be read or parsed are skipped here; the product-boundary check and the
//! engine manifest guard already report them.

use std::fs;
use std::path::Path;

/// The dependency tables the guard scans, directly and under `[target]`.
const CHECKED_KINDS: [&str; 2] = ["dependencies", "build-dependencies"];

/// The feature no non-dev table may enable on an engine crate.
const GUARDED_FEATURE: &str = crate::engine_deps::EXEMPTING_FEATURE;

/// Scans the workspace for non-dev dependency tables, and `[features]`
/// values, that enable an engine crate's `test-support` feature.
#[must_use]
pub(crate) fn test_support_leak_violations(root: &Path) -> Vec<String> {
    let engine_names = engine_package_names(root);
    if engine_names.is_empty() {
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
            &engine_names,
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
            scan_table(
                &manifest_path,
                table,
                entries,
                &engine_names,
                &mut violations,
            );
        }
        scan_features(
            &manifest_path,
            &manifest,
            &tables,
            &engine_names,
            &mut violations,
        );
    }
    violations
}

/// Reports every `[features]` value that enables the guarded feature on an
/// engine crate through a dependency-feature reference.
fn scan_features(
    manifest_path: &Path,
    manifest: &toml::Value,
    tables: &[(String, &toml::map::Map<String, toml::Value>)],
    engine_names: &[String],
    violations: &mut Vec<String>,
) {
    let Some(features) = manifest.get("features").and_then(toml::Value::as_table) else {
        return;
    };
    let self_name = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str);
    let self_is_engine = self_name.is_some_and(|name| engine_names.iter().any(|e| e == name));
    for (feature, values) in features {
        // An engine crate's own `test-support` forwarding to a sibling's is
        // gated by a feature the dependency-table scan already confines to
        // dev tables; only a feature outside that gate leaks.
        if self_is_engine && feature == GUARDED_FEATURE {
            continue;
        }
        let Some(values) = values.as_array() else {
            continue;
        };
        for dep in values
            .iter()
            .filter_map(toml::Value::as_str)
            .filter_map(guarded_dependency_reference)
        {
            let package = resolve_package(dep, tables);
            if !engine_names.iter().any(|name| name == package) {
                continue;
            }
            violations.push(format!(
                "{}: [features] {feature} enables {package}/{GUARDED_FEATURE}; only [dev-dependencies] may enable an engine crate's {GUARDED_FEATURE} feature",
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

/// Reports every entry in `table` that names an engine crate and lists the
/// guarded feature.
fn scan_table(
    manifest: &Path,
    table: &str,
    entries: &toml::map::Map<String, toml::Value>,
    engine_names: &[String],
    violations: &mut Vec<String>,
) {
    for (key, entry) in entries {
        let package = entry
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(key);
        if !engine_names.iter().any(|name| name == package) {
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
                "{}: [{table}] enables {package}/{GUARDED_FEATURE}; only [dev-dependencies] may enable an engine crate's {GUARDED_FEATURE} feature",
                manifest.display()
            ));
        }
    }
}

/// The package names of every engine crate whose manifest parses.
fn engine_package_names(root: &Path) -> Vec<String> {
    crate::engine_guards::engine_crates(root)
        .iter()
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
