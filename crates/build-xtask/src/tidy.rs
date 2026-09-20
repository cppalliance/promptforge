//! Tidy-style architecture checks for the workshop server decomposition,
//! the harness family, and the sans-I/O engine (manifest guard,
//! retired-symbol scan, and `test-support` leak guard, run from
//! `engine_guards`).
//!
//! Each check returns a list of human-readable violations. The `#[test]`
//! wrappers assert the lists are empty, so `cargo test -p build-xtask`
//! enforces the architecture; `cargo xtask tidy` prints the same report
//! on demand. The file ceiling and lint inheritance checks bind every
//! `workshop-*` and `harness-*` crate (plus `harness-api`, minus the
//! `workshop` shell) by package name, and every other crate whose crate
//! docs carry the `## Invariants` marker; a family crate missing the marker
//! is itself a violation.

use std::fs;
use std::path::{Path, PathBuf};

/// Tier 0: vocabulary crates. No internal `workshop-*` dependencies.
const VOCABULARY: &[&str] = &["workshop-protocol", "workshop-registry", "workshop-support"];
/// Tier 1: domain services. Depend on vocabulary crates only.
const SERVICES: &[&str] = &["workshop-gateway", "workshop-menu", "workshop-status"];
/// Tier 2: features. Depend on vocabulary and service crates. The
/// sessions subsystem lives inside the shell since Workshop moved onto the
/// harness, so it has no crate here.
const FEATURES: &[&str] = &["workshop-user-state", "workshop-workspace"];
/// Tier 3: the shell. May depend on every lower tier.
const SHELL: &[&str] = &["workshop-server"];

/// File-line ceiling from the `AGENTS.md` structural rules.
const MAX_FILE_LINES: usize = 500;

/// Marker in a crate's `lib.rs` (or `main.rs`) crate docs. Mandatory for
/// every `workshop-*` and `harness-*` crate (see [`family_requires_marker`]);
/// on any other crate it opts that crate into the decomposed-architecture
/// checks (`build-xtask` carries it deliberately). The `new-crate`
/// scaffolder emits it.
const INVARIANT_MARKER: &str = "//! ## Invariants";

/// Runs every check and returns all violations.
#[must_use]
pub(crate) fn all_violations(root: &Path) -> Vec<String> {
    let mut violations = tier_dependency_violations(root);
    violations.extend(marker_violations(root));
    violations.extend(file_ceiling_violations(root));
    violations.extend(lint_inheritance_violations(root));
    violations.extend(crate::product::product_boundary_violations(root));
    violations.extend(crate::harness_bans::harness_clippy_bans(
        &root.join("crates").join("harness"),
        &root.join("crates").join("harness-api"),
    ));
    violations.extend(crate::engine_guards::engine_guard_violations(root));
    violations
}

/// The internal `workshop-*` crates a tiered crate may depend on, or `None`
/// when `name` is not part of the decomposition's crate map.
fn allowed_dependencies(name: &str) -> Option<Vec<&'static str>> {
    let allowed = if name == "workshop-registry" {
        // The proxy slots speak the wire types: the status push-channel
        // slot carries `workshop-protocol`'s `StatusBarUpdate`.
        vec!["workshop-protocol"]
    } else if VOCABULARY.contains(&name) {
        Vec::new()
    } else if SERVICES.contains(&name) {
        VOCABULARY.to_vec()
    } else if FEATURES.contains(&name) {
        [VOCABULARY, SERVICES].concat()
    } else if SHELL.contains(&name) {
        [VOCABULARY, SERVICES, FEATURES].concat()
    } else {
        return None;
    };
    Some(allowed)
}

/// The crate directory for a tiered workshop package: the family lives in
/// the `crates/workshop/` container, with the shell at `shell/`.
fn tiered_crate_dir(root: &Path, name: &str) -> PathBuf {
    let short = name.strip_prefix("workshop-").unwrap_or("shell");
    root.join("crates").join("workshop").join(short)
}

/// Checks that tiered `workshop-*` crates depend only on lower tiers.
///
/// Every tiered crate has landed, so a missing manifest is a violation,
/// not a crate to skip.
#[must_use]
pub(crate) fn tier_dependency_violations(root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    for name in [VOCABULARY, SERVICES, FEATURES, SHELL].concat() {
        let Some(allowed) = allowed_dependencies(name) else {
            continue;
        };
        let manifest_path = tiered_crate_dir(root, name).join("Cargo.toml");
        let Ok(text) = fs::read_to_string(&manifest_path) else {
            violations.push(format!(
                "{name}: tiered crate has no manifest at {}",
                manifest_path.display()
            ));
            continue;
        };
        let manifest: toml::Value = match toml::from_str(&text) {
            Ok(manifest) => manifest,
            Err(error) => {
                violations.push(format!(
                    "{}: unparseable manifest: {error}",
                    manifest_path.display()
                ));
                continue;
            }
        };
        for dep in workshop_dependencies(&manifest) {
            if dep == name {
                continue; // self dev-dependency for test fixtures
            }
            if !allowed.contains(&dep.as_str()) {
                violations.push(format!(
                    "{name} depends on {dep}, which its tier forbids (allowed: {})",
                    allowed.join(", ")
                ));
            }
        }
    }
    violations
}

/// Collects the `workshop-*` dependency names of every kind (normal, dev,
/// build, and target-specific) declared in a manifest.
fn workshop_dependencies(manifest: &toml::Value) -> Vec<String> {
    let mut names = Vec::new();
    let kinds = ["dependencies", "dev-dependencies", "build-dependencies"];
    for (_, table) in crate::manifest::dependency_tables(manifest, &kinds) {
        collect_workshop_deps(table, &mut names);
    }
    names
}

fn collect_workshop_deps(table: &toml::map::Map<String, toml::Value>, names: &mut Vec<String>) {
    for (key, value) in table {
        let package = value
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(key);
        if package.starts_with("workshop-") && !names.contains(&package.to_owned()) {
            names.push(package.to_owned());
        }
    }
}

/// Checks the 500-line file ceiling on every crate participating in the
/// decomposed architecture (its `lib.rs` or `main.rs` carries the invariant
/// marker).
#[must_use]
pub(crate) fn file_ceiling_violations(root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    for dir in participating_crates(root) {
        for file in rust_files(&dir) {
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            let lines = text.lines().count();
            if lines > MAX_FILE_LINES {
                violations.push(format!(
                    "{} has {lines} lines, over the {MAX_FILE_LINES}-line ceiling",
                    file.display()
                ));
            }
        }
    }
    violations
}

/// Checks that every participating crate inherits `[lints] workspace = true`
/// (which carries `unreachable_pub`) and that the workspace root sets it.
#[must_use]
pub(crate) fn lint_inheritance_violations(root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    let root_manifest = root.join("Cargo.toml");
    match fs::read_to_string(&root_manifest)
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
    {
        Some(manifest) => {
            let set = manifest
                .get("workspace")
                .and_then(|w| w.get("lints"))
                .and_then(|l| l.get("rust"))
                .and_then(|r| r.get("unreachable_pub"));
            if set.is_none() {
                violations.push(
                    "workspace root does not set `unreachable_pub` in [workspace.lints.rust]"
                        .to_owned(),
                );
            }
        }
        None => violations.push(format!("{}: unparseable manifest", root_manifest.display())),
    }
    for dir in participating_crates(root) {
        let manifest_path = dir.join("Cargo.toml");
        let inherits = fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
            .and_then(|manifest| {
                manifest
                    .get("lints")
                    .and_then(|l| l.get("workspace"))
                    .and_then(toml::Value::as_bool)
            })
            .unwrap_or(false);
        if !inherits {
            violations.push(format!(
                "{} does not inherit `[lints] workspace = true`",
                manifest_path.display()
            ));
        }
    }
    violations
}

/// Check that every crate the families bind by name carries the marker.
#[must_use]
pub(crate) fn marker_violations(root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    for dir in workspace_crates(root) {
        let Some(name) = package_name(&dir) else {
            continue;
        };
        if family_requires_marker(&name) && !carries_marker(&dir) {
            violations.push(format!(
                "{name}: src/lib.rs lacks the `{INVARIANT_MARKER}` marker required of every \
                 workshop-* and harness-* crate"
            ));
        }
    }
    violations
}

/// Whether a package name places the crate in a family that must carry the
/// marker: `workshop-*` and `harness-*` (which covers `harness-api`). The
/// Tauri shell (the `workshop` package) is exempt.
fn family_requires_marker(name: &str) -> bool {
    name != "workshop" && (name.starts_with("workshop-") || name.starts_with("harness-"))
}

/// Crates bound by the file ceiling and lint inheritance checks: the union
/// of the crates the families bind by name and every crate carrying the
/// marker.
fn participating_crates(root: &Path) -> Vec<PathBuf> {
    workspace_crates(root)
        .into_iter()
        .filter(|dir| {
            package_name(dir).is_some_and(|name| family_requires_marker(&name))
                || carries_marker(dir)
        })
        .collect()
}

/// Every crate directory under `crates/`. A directory containing a
/// `Cargo.toml` is a crate and is not descended into; any other directory
/// is a container and the walk descends one level, so crates nested under
/// `crates/workshop/` and `crates/harness/` stay visible.
fn workspace_crates(root: &Path) -> Vec<PathBuf> {
    let mut crates = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("crates")) else {
        return crates;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if dir.join("Cargo.toml").exists() {
            crates.push(dir);
        } else if let Ok(inner) = fs::read_dir(&dir) {
            for entry in inner.flatten() {
                let sub = entry.path();
                if sub.is_dir() && sub.join("Cargo.toml").exists() {
                    crates.push(sub);
                }
            }
        }
    }
    crates
}

/// The `[package] name` declared in a crate directory's manifest.
fn package_name(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let manifest: toml::Value = toml::from_str(&text).ok()?;
    manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
}

/// Whether a crate's `lib.rs` or `main.rs` crate docs carry the marker.
fn carries_marker(dir: &Path) -> bool {
    ["src/lib.rs", "src/main.rs"].iter().any(|candidate| {
        fs::read_to_string(dir.join(candidate)).is_ok_and(|text| text.contains(INVARIANT_MARKER))
    })
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_files(dir, &mut files);
    files
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if entry.file_name() != "target" {
                collect_rust_files(&path, files);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

#[cfg(test)]
#[path = "tidy-tests.rs"]
mod tests;
