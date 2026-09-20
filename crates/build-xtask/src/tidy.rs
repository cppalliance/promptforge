//! Tidy-style architecture checks for the workshop server decomposition,
//! the harness family, and the sans-I/O engine (manifest guard,
//! retired-symbol scan, and `test-support` leak guard, run from
//! `engine_guards`).
//!
//! Each check returns a list of human-readable violations. The `#[test]`
//! wrappers assert the lists are empty, so `cargo test -p build-xtask`
//! enforces the architecture; `cargo xtask tidy` prints the same report
//! on demand. The file ceiling and lint inheritance checks bind every
//! crate whose crate docs carry the `## Invariants` marker: the
//! `workshop-*` crates today and the `harness-*` crates as they land.

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

/// Marker in a crate's `lib.rs` (or `main.rs`) crate docs opting the crate
/// into the decomposed-architecture checks. The `new-crate` scaffolder emits
/// it; every `workshop-*` and `harness-*` crate carries it, and crates
/// outside those families are left alone.
const INVARIANT_MARKER: &str = "//! ## Invariants";

/// Run every check and return all violations.
#[must_use]
pub(crate) fn all_violations(root: &Path) -> Vec<String> {
    let mut violations = tier_dependency_violations(root);
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

/// Check that tiered `workshop-*` crates depend only on lower tiers.
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

/// Collect the `workshop-*` dependency names of every kind (normal, dev,
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

/// Check the 500-line file ceiling on every crate participating in the
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

/// Check that every participating crate inherits `[lints] workspace = true`
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

/// Crates under `crates/` whose crate docs carry the invariant marker. A
/// directory containing a `Cargo.toml` is a crate and is not descended
/// into; any other directory is a container and the walk descends one
/// level, so crates nested under `crates/workshop/` and `crates/harness/`
/// stay visible.
fn participating_crates(root: &Path) -> Vec<PathBuf> {
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
            if carries_marker(&dir) {
                crates.push(dir);
            }
        } else if let Ok(inner) = fs::read_dir(&dir) {
            for entry in inner.flatten() {
                let sub = entry.path();
                if sub.is_dir() && sub.join("Cargo.toml").exists() && carries_marker(&sub) {
                    crates.push(sub);
                }
            }
        }
    }
    crates
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
mod tests {
    use super::*;

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("build-xtask lives at <root>/crates/build-xtask")
            .to_path_buf()
    }

    #[test]
    fn workshop_tier_dependencies_flow_one_way() {
        let violations = tier_dependency_violations(&workspace_root());
        assert!(
            violations.is_empty(),
            "tier violations:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn participating_crates_respect_the_file_line_ceiling() {
        let violations = file_ceiling_violations(&workspace_root());
        assert!(
            violations.is_empty(),
            "ceiling violations:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn participating_crates_inherit_workspace_lints() {
        let violations = lint_inheritance_violations(&workspace_root());
        assert!(
            violations.is_empty(),
            "lint violations:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn a_tiered_crate_whose_manifest_is_missing_is_reported_not_skipped() {
        let root = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir_all(root.path().join("crates")).expect("the crates directory creates");
        let violations = tier_dependency_violations(root.path());
        let tiered = [VOCABULARY, SERVICES, FEATURES, SHELL].concat();
        assert_eq!(
            violations.len(),
            tiered.len(),
            "every tiered crate's missing manifest is reported: {violations:?}"
        );
        for name in tiered {
            assert!(
                violations.iter().any(|v| v.contains(name)),
                "{name} is named in the violations: {violations:?}"
            );
        }
    }

    /// Write a crate under `crates/<dir>/` with the given `lib.rs` docs and
    /// one source file of `lines` lines.
    fn write_marked_crate(root: &Path, dir: &str, lib_docs: &str, lines: usize) {
        let src = root.join("crates").join(dir).join("src");
        std::fs::create_dir_all(&src).expect("the crate source directory creates");
        std::fs::write(
            src.parent().expect("src has a parent").join("Cargo.toml"),
            "[package]\nname = \"fixture\"\n[lints]\nworkspace = true\n",
        )
        .expect("the manifest writes");
        std::fs::write(src.join("lib.rs"), lib_docs).expect("lib.rs writes");
        std::fs::write(src.join("big.rs"), "// line\n".repeat(lines)).expect("big.rs writes");
    }

    #[test]
    fn a_harness_crate_carrying_the_marker_is_held_to_the_ceiling() {
        let root = tempfile::TempDir::new().expect("tempdir");
        write_marked_crate(
            root.path(),
            "harness/runner",
            "//! Effect loop.\n//!\n//! ## Invariants\n//!\n//! - none\n",
            MAX_FILE_LINES + 1,
        );
        let violations = file_ceiling_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains("big.rs") && violations[0].contains("over the 500-line ceiling"),
            "the oversized harness file is reported: {violations:?}"
        );
    }

    #[test]
    fn a_harness_crate_without_the_marker_is_outside_the_ceiling() {
        let root = tempfile::TempDir::new().expect("tempdir");
        write_marked_crate(
            root.path(),
            "harness/runner",
            "//! Effect loop, not yet opted in.\n",
            MAX_FILE_LINES + 1,
        );
        assert!(
            file_ceiling_violations(root.path()).is_empty(),
            "the marker is what opts a harness crate into the ceiling"
        );
    }

    #[test]
    fn tier_table_grants_each_tier_only_lower_tiers() {
        assert_eq!(allowed_dependencies("workshop-protocol"), Some(Vec::new()));
        assert_eq!(
            allowed_dependencies("workshop-registry"),
            Some(vec!["workshop-protocol"])
        );
        assert_eq!(
            allowed_dependencies("workshop-gateway"),
            Some(VOCABULARY.to_vec())
        );
        assert_eq!(
            allowed_dependencies("workshop-workspace"),
            Some([VOCABULARY, SERVICES].concat())
        );
        assert_eq!(
            allowed_dependencies("workshop-server"),
            Some([VOCABULARY, SERVICES, FEATURES].concat())
        );
        assert_eq!(allowed_dependencies("gateway"), None);
    }
}
