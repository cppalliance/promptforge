//! Engine guards, live: the manifest guard and the retired-symbol scan run
//! over the engine crates, and the `test-support` leak guard runs over the
//! whole workspace, as part of `cargo test -p build-xtask` and
//! `cargo xtask tidy`.
//!
//! The engine is the `promptforge` facade and every crate under the
//! `crates/promptforge-internal/` container. The root crate is named, so
//! a missing manifest is reported rather than skipped; the container's
//! members are discovered, so a new engine crate is covered the moment it
//! lands.

use std::fs;
use std::path::{Path, PathBuf};

/// The engine crates that live directly under `crates/`.
const ENGINE_ROOT_CRATES: [&str; 1] = ["promptforge"];

/// The private container whose every member is an engine crate.
const ENGINE_CONTAINER: &str = "promptforge-internal";

/// The identifiers the sans-I/O engine plan retired. Live engine source
/// (outside `#[cfg(test)]`, `tests/`, and test-support modules) may not
/// name any of them again.
pub(crate) const RETIRED_SEEDS: [&str; 8] = [
    "install_agent_chat_shim",
    "EventsSnapshot",
    "install_runtime_events",
    "GatewaySource",
    "run_models_loop",
    "LuaFanoutResult",
    "Observer",
    "DebugCapture",
];

/// Every engine crate directory: the named root crates first, whether or
/// not they exist, then every crate under the container in directory
/// order.
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

/// Every crate directory under `dir`: a directory holding a `Cargo.toml`
/// is a crate and is not descended into; any other directory is a
/// container and the walk descends.
pub(crate) fn collect_crates(dir: &Path, crates: &mut Vec<PathBuf>) {
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

/// Runs the manifest guard over every engine crate.
#[must_use]
pub(crate) fn engine_manifest_violations(root: &Path) -> Vec<String> {
    engine_crates(root)
        .iter()
        .flat_map(|dir| crate::engine_deps::forbidden_engine_dependencies(&dir.join("Cargo.toml")))
        .map(|violation| violation.to_string())
        .collect()
}

/// Runs the retired-symbol scan over every engine crate's live source. The
/// scan takes the whole crate directory, so `build.rs`, `benches/`, and
/// `examples/` are covered too; it skips `tests/` and test support itself.
#[must_use]
pub(crate) fn retired_symbol_violations(root: &Path) -> Vec<String> {
    engine_crates(root)
        .iter()
        .flat_map(|dir| crate::retired_symbols::retired_symbols(dir, &RETIRED_SEEDS))
        .map(|hit| hit.to_string())
        .collect()
}

/// Every engine guard, in one list: the manifest guard, the retired-symbol
/// scan, and the workspace-wide `test-support` leak guard.
#[must_use]
pub(crate) fn engine_guard_violations(root: &Path) -> Vec<String> {
    let mut violations = engine_manifest_violations(root);
    violations.extend(retired_symbol_violations(root));
    violations.extend(crate::test_support_leak::test_support_leak_violations(root));
    violations
}

#[cfg(test)]
#[path = "engine_guards-tests.rs"]
mod tests;
