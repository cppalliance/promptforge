//! Engine manifest guard: the sans-I/O engine crates declare no async
//! runtime and no HTTP client.
//!
//! The engine (the `promptforge` facade and the crates under
//! `crates/promptforge-internal/`) is a deterministic state machine; every
//! wait becomes an effect the harness performs. Its manifests therefore may
//! not name `tokio`, `tokio-util`, `async-trait`, or `reqwest` in
//! `[dependencies]`, `[build-dependencies]`, or the target-specific forms
//! of either. `[dev-dependencies]` are outside the guard: the engine's own
//! suites drive it from a tokio test harness against a mock gateway.
//!
//! Exemption: an entry marked `optional = true` that only the
//! `test-support` feature enables is exempt, so the tokio test driver can
//! ship behind that feature for the engine's own suite and the companion
//! crates' suites. "Only" is transitive: a feature that enables
//! `test-support` (such as `default`) would enable the dependency too, so
//! its presence voids the exemption. The exemption is safe only while
//! `test-support` is enabled from `[dev-dependencies]` alone; the
//! `test_support_leak` guard fails the build when any non-dev table in the
//! workspace enables it.
//!
//! The check reads declared dependencies, not the resolved graph, so
//! `workspace-hack` unification is irrelevant to it.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// The crates an engine manifest may not declare outside `[dev-dependencies]`.
const FORBIDDEN: [&str; 4] = ["tokio", "tokio-util", "async-trait", "reqwest"];

/// The one feature that may gate an optional forbidden dependency.
pub(crate) const EXEMPTING_FEATURE: &str = "test-support";

/// The dependency tables the guard scans, directly and under `[target]`.
const CHECKED_KINDS: [&str; 2] = ["dependencies", "build-dependencies"];

/// One finding from scanning an engine manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Violation {
    /// A forbidden crate declared outside `[dev-dependencies]`.
    Forbidden {
        /// The manifest scanned.
        manifest: PathBuf,
        /// The table the entry sits in, as it would head the section in the
        /// manifest (`dependencies`, `target.'cfg(windows)'.build-dependencies`).
        table: String,
        /// The forbidden package name, after resolving `package = ...` renames.
        package: String,
    },
    /// The manifest could not be read or parsed.
    Unreadable {
        /// The manifest scanned.
        manifest: PathBuf,
        /// What went wrong.
        error: String,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Forbidden {
                manifest,
                table,
                package,
            } => write!(
                f,
                "{}: [{table}] declares {package}; engine crates declare {} only under [dev-dependencies]",
                manifest.display(),
                FORBIDDEN.join(", ")
            ),
            Self::Unreadable { manifest, error } => {
                write!(f, "{}: {error}", manifest.display())
            }
        }
    }
}

/// Scans one engine manifest for forbidden dependencies. A manifest that
/// cannot be read or parsed yields one [`Violation::Unreadable`].
#[must_use]
pub(crate) fn forbidden_engine_dependencies(manifest: &Path) -> Vec<Violation> {
    let text = match fs::read_to_string(manifest) {
        Ok(text) => text,
        Err(error) => {
            return vec![Violation::Unreadable {
                manifest: manifest.to_path_buf(),
                error: format!("unreadable manifest: {error}"),
            }];
        }
    };
    let value: toml::Value = match toml::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return vec![Violation::Unreadable {
                manifest: manifest.to_path_buf(),
                error: format!("unparseable manifest: {error}"),
            }];
        }
    };
    let features = feature_lists(&value);
    let mut violations = Vec::new();
    for (table, entries) in crate::manifest::dependency_tables(&value, &CHECKED_KINDS) {
        for (key, entry) in entries {
            let package = entry
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(key);
            if !FORBIDDEN.contains(&package) {
                continue;
            }
            if is_optional(entry) && is_exempt(&features, key) {
                continue;
            }
            violations.push(Violation::Forbidden {
                manifest: manifest.to_path_buf(),
                table: table.clone(),
                package: package.to_owned(),
            });
        }
    }
    violations
}

/// Whether a dependency entry is declared `optional = true`.
fn is_optional(entry: &toml::Value) -> bool {
    entry
        .get("optional")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// The `[features]` table as `(feature, items)` pairs, in manifest order.
/// Items that are not strings are dropped; cargo would reject them anyway.
fn feature_lists(manifest: &toml::Value) -> Vec<(&str, Vec<&str>)> {
    manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|table| {
            table.iter().map(|(name, list)| {
                let items = list
                    .as_array()
                    .map(|items| items.iter().filter_map(toml::Value::as_str).collect())
                    .unwrap_or_default();
                (name.as_str(), items)
            })
        })
        .collect()
}

/// Whether the interim exemption holds for the optional dependency `key`:
/// `test-support` is the one feature, directly or through other features,
/// that enables it.
fn is_exempt(features: &[(&str, Vec<&str>)], key: &str) -> bool {
    let enablers = enabling_features(features, key);
    enablers.len() == 1 && enablers.contains(EXEMPTING_FEATURE)
}

/// Every feature that enables the optional dependency `key`, directly or
/// through another feature, following cargo's rules:
///
/// - `dep:key` enables it, and once any feature uses that form no implicit
///   feature named `key` exists.
/// - `key/<feature>` enables it whether or not `dep:` is in use; the weak
///   form `key?/<feature>` never does.
/// - Without `dep:`, cargo creates the implicit feature `key`, and every
///   feature listing `key` enables it.
/// - A feature that lists an enabling feature is itself an enabler, so
///   `default = ["test-support"]` counts.
fn enabling_features(features: &[(&str, Vec<&str>)], key: &str) -> BTreeSet<String> {
    let explicit_dep = format!("dep:{key}");
    let strong_prefix = format!("{key}/");
    let uses_dep_syntax = features
        .iter()
        .any(|(_, items)| items.contains(&explicit_dep.as_str()));
    let mut enablers: BTreeSet<String> = features
        .iter()
        .filter(|(_, items)| {
            items.iter().any(|item| {
                *item == explicit_dep
                    || item.starts_with(&strong_prefix)
                    || (!uses_dep_syntax && *item == key)
            })
        })
        .map(|(name, _)| (*name).to_owned())
        .collect();
    if !uses_dep_syntax {
        enablers.insert(key.to_owned());
    }
    loop {
        let before = enablers.len();
        for (name, items) in features {
            if items.iter().any(|item| enablers.contains(*item)) {
                enablers.insert((*name).to_owned());
            }
        }
        if enablers.len() == before {
            return enablers;
        }
    }
}

#[cfg(test)]
#[path = "engine_deps-tests.rs"]
mod tests;
