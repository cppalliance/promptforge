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
//! `workshop` shell) by package name, every other crate whose crate
//! docs carry the `## Invariants` marker, and every crate directory whose
//! manifest the shared walk could not read, parse, or find a package name
//! in - a crate with no readable name cannot be shown exempt. Those read
//! failures are reported by `marker_violations`, their one owner.

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
    violations.extend(walled_tier_violations(root));
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
///
/// A manifest the walk could not read, parse, or find a package name in is
/// reported as itself, not skipped: a crate with no readable name cannot be
/// shown exempt from the marker. This check is the one owner of the shared
/// walk's read failures; the product-boundary check shares the walk and
/// leaves them here.
#[must_use]
pub(crate) fn marker_violations(root: &Path) -> Vec<String> {
    let walk = crate::product::workspace_crates(root);
    let mut violations = walk.violations;
    for krate in &walk.crates {
        if family_requires_marker(&krate.package) && !carries_marker(&root.join(&krate.dir)) {
            violations.push(format!(
                "{}: src/lib.rs lacks the `{INVARIANT_MARKER}` marker required of every \
                 workshop-* and harness-* crate",
                krate.package
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
/// of the crates the families bind by name, every crate carrying the
/// marker, and every crate whose manifest the walk could not read - an
/// unreadable manifest cannot show a crate exempt.
fn participating_crates(root: &Path) -> Vec<PathBuf> {
    let walk = crate::product::workspace_crates(root);
    walk.crates
        .iter()
        .filter(|krate| {
            family_requires_marker(&krate.package) || carries_marker(&root.join(&krate.dir))
        })
        .map(|krate| root.join(&krate.dir))
        .chain(walk.unread.iter().map(|dir| root.join(dir)))
        .collect()
}

/// Whether a crate's `lib.rs` or `main.rs` crate docs carry the marker.
fn carries_marker(dir: &Path) -> bool {
    ["src/lib.rs", "src/main.rs"].iter().any(|candidate| {
        fs::read_to_string(dir.join(candidate)).is_ok_and(|text| text.contains(INVARIANT_MARKER))
    })
}

/// The crate-relative path the tier's modules are spelled by.
const WALLED_PATH: &str = "crate::admin::walled::";
/// The files outside the tier that may name a tier module, keyed on path
/// because a line number moves with any edit: `lib.rs` for the `AppState`
/// field types and the router merge, `registry.rs` for the route
/// enumeration, and `test_support.rs` for the fixture that assembles the
/// same state. `test_support.rs` is `#[cfg(test)]`-gated, but the rule
/// reads source text and never sees a cfg, so it is allowlisted by name
/// like the other two.
const WALLED_ALLOWLIST: [&str; 3] = [
    "crates/gateway/app/src/lib.rs",
    "crates/gateway/app/src/registry.rs",
    "crates/gateway/app/src/test_support.rs",
];

/// Checks that only the walled admin tier's own modules and the three
/// assembly sites in [`WALLED_ALLOWLIST`] name a `crate::admin::walled::`
/// path, so a module that no walled route needs does not sit in a
/// directory defined as routes that read secrets, write files, or launch
/// processes.
///
/// The rule protects that directory's meaning, not the wall: wall
/// enforcement is structural, at the router merge in the gateway app's
/// `lib.rs`. Its reach is one textual match, so it catches a spelled path
/// and misses an alias, a `super::` path, a re-export, and any path a
/// macro generates. It is a tripwire for the common case, not a proof of
/// the boundary.
///
/// A source file the check cannot read is reported rather than skipped: a
/// file that was never scanned cannot be shown clean.
#[must_use]
pub(crate) fn walled_tier_violations(root: &Path) -> Vec<String> {
    let app_src = root.join("crates").join("gateway").join("app").join("src");
    let tier = app_src.join("admin").join("walled");
    let mut violations = Vec::new();
    for file in rust_files(&app_src) {
        if file.starts_with(&tier) {
            continue;
        }
        let relative = slash_path(root, &file);
        if WALLED_ALLOWLIST.contains(&relative.as_str()) {
            continue;
        }
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) => {
                violations.push(format!("{relative}: unreadable source file: {error}"));
                continue;
            }
        };
        for (index, line) in text.lines().enumerate() {
            // A doc or line comment naming a route module in prose states
            // where the other half of a feature lives; it is not a
            // dependency on it, and failing the build on documentation
            // would only teach authors to stop writing it.
            if line.trim_start().starts_with("//") || !line.contains(WALLED_PATH) {
                continue;
            }
            violations.push(format!(
                "{relative}:{} names {WALLED_PATH}, which only the tier's own modules \
                 and the assembly sites ({}) may",
                index + 1,
                WALLED_ALLOWLIST.join(", ")
            ));
        }
    }
    violations
}

/// A file's path relative to the workspace root with `/` separators, so
/// the allowlist and the violations read the same on every platform.
///
/// A component that is not valid UTF-8 is rendered lossily rather than
/// dropped: dropping it would shorten the path and could make it match an
/// allowlist entry, exempting a file the rule should report.
fn slash_path(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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
