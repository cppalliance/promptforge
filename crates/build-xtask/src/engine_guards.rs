//! Engine guards, live: the dependency guard's manifest scan and
//! resolved-graph check and the clippy guard run over the Engine crates,
//! and the `test-support` leak guard runs over the whole workspace, as
//! part of `cargo test -p build-xtask` and `cargo xtask tidy`.
//!
//! The Engine is the `runvil` facade, the `runvil-plugin`
//! contract, and every crate under the `crates/runvil-internal/`
//! container. The root crates are named, so a missing manifest is
//! reported rather than skipped; the container's members are discovered,
//! so a new Engine crate is covered the moment it lands.

use std::fs;
use std::path::{Path, PathBuf};

/// The Engine crates that live directly under `crates/`.
const ENGINE_ROOT_CRATES: [&str; 2] = ["runvil", "runvil-plugin"];

/// The private container whose every member is an Engine crate.
pub(crate) const ENGINE_CONTAINER: &str = "runvil-internal";

/// Every Engine crate directory: the named root crates first, whether or
/// not they exist, then every crate under the container in path order.
#[must_use]
pub(crate) fn engine_crates(root: &Path) -> Vec<PathBuf> {
    let crates_dir = root.join("crates");
    let mut crates: Vec<PathBuf> = ENGINE_ROOT_CRATES
        .iter()
        .map(|name| crates_dir.join(name))
        .collect();
    collect_crates(&crates_dir.join(ENGINE_CONTAINER), &mut crates);
    crates
}

/// Every crate directory under `dir`, in path order: a directory holding a
/// `Cargo.toml` is a crate and is not descended into; any other directory
/// is a container and the walk descends. The order is sorted because
/// `read_dir` order differs between filesystems, and findings must not.
pub(crate) fn collect_crates(dir: &Path, crates: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut subs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    subs.sort();
    for sub in subs {
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

/// Runs the dependency guard over every Engine crate: its manifest scan,
/// then its resolved-graph check.
#[must_use]
fn engine_dependency_violations(root: &Path) -> Vec<String> {
    engine_crates(root)
        .iter()
        .flat_map(|dir| {
            let mut violations =
                crate::engine_deps::forbidden_engine_dependencies(&dir.join("Cargo.toml"));
            violations.extend(crate::engine_deps::forbidden_engine_graph(root, dir));
            violations
        })
        .map(|violation| violation.to_string())
        .collect()
}

/// Every Engine guard, in one list: the dependency guard, the clippy
/// guard, and the workspace-wide `test-support` leak guard.
#[must_use]
pub(crate) fn engine_guard_violations(root: &Path) -> Vec<String> {
    let mut violations = engine_dependency_violations(root);
    violations.extend(crate::engine_clippy::engine_clippy_violations(root));
    violations.extend(crate::test_support_leak::test_support_leak_violations(root));
    violations
}

#[cfg(test)]
#[path = "engine_guards-tests.rs"]
mod tests;
