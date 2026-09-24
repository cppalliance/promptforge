//! The live engine guards over this workspace, plus fixture tests that a
//! reintroduced retired symbol or a forbidden dependency in an engine
//! crate fails the guard.

use std::path::{Path, PathBuf};

use super::*;
use crate::product::test_support;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("build-xtask lives at <root>/crates/build-xtask")
        .to_path_buf()
}

/// Writes one crate under `<root>/crates/<dir>/` with the given manifest
/// body (after `[package]`) and `src/lib.rs` text.
fn write_crate(root: &Path, dir: &str, manifest: &str, lib: &str) {
    let crate_dir = root.join("crates").join(dir);
    let src = crate_dir.join("src");
    std::fs::create_dir_all(&src).expect("the crate source directory creates");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!("[package]\nname = \"fixture\"\n{manifest}"),
    )
    .expect("the manifest writes");
    std::fs::write(src.join("lib.rs"), lib).expect("lib.rs writes");
}

/// A fake workspace whose root engine crates are clean, so a fixture can
/// add one container crate and see only that crate's findings.
fn clean_engine_root() -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("tempdir");
    for name in ENGINE_ROOT_CRATES {
        write_crate(
            root.path(),
            name,
            "[dependencies]\nserde = \"1\"\n",
            "pub struct Live;\n",
        );
    }
    root
}

#[test]
fn engine_crates_declare_no_forbidden_dependencies() {
    let violations = engine_manifest_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "engine manifest violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn engine_sources_name_no_retired_symbols() {
    let violations = retired_symbol_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "retired symbols in live engine source:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_engine_crate_set_is_the_root_crates_plus_every_container_member() {
    let root = clean_engine_root();
    write_crate(
        root.path(),
        "promptforge-internal/lua",
        "",
        "pub struct Vm;\n",
    );
    write_crate(
        root.path(),
        "promptforge-internal/store",
        "",
        "pub struct Store;\n",
    );
    write_crate(root.path(), "harness/runner", "", "pub struct Runner;\n");
    write_crate(root.path(), "gateway-api-types", "", "pub struct Api;\n");
    let mut names: Vec<String> = engine_crates(root.path())
        .iter()
        .map(|dir| {
            dir.strip_prefix(root.path().join("crates"))
                .expect("under crates/")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "promptforge",
            "promptforge-internal/lua",
            "promptforge-internal/store",
        ],
        "harness and gateway crates are outside the engine"
    );
}

#[test]
fn the_facade_forwarding_the_engine_test_support_feature_passes_every_engine_guard() {
    let root = tempfile::TempDir::new().expect("tempdir");
    test_support::write_crate(
        root.path(),
        "promptforge-internal/engine",
        "promptforge-engine",
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ntest-support = [\"dep:tokio\"]\n",
    );
    test_support::write_crate(
        root.path(),
        "promptforge-internal/types",
        "promptforge-types",
        "",
    );
    test_support::write_crate(
        root.path(),
        "promptforge",
        "promptforge",
        "[dependencies]\npromptforge-engine = { path = \"../promptforge-internal/engine\" }\n\
         [features]\ntest-support = [\"promptforge-engine/test-support\"]\n",
    );
    let violations = engine_guard_violations(root.path());
    assert!(
        violations.is_empty(),
        "the facade is an engine crate, so forwarding its own test-support is gated: {violations:?}"
    );
}

#[test]
fn a_retired_symbol_reintroduced_in_an_engine_crate_fails_the_guard() {
    let root = clean_engine_root();
    write_crate(
        root.path(),
        "promptforge-internal/lua",
        "",
        "pub struct Vm;\n\npub fn install_agent_chat_shim() {}\n",
    );
    let violations = engine_guard_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("install_agent_chat_shim")
            && violations[0].contains("lib.rs")
            && violations[0].contains(":3"),
        "the reintroduced seed is reported with its file and line: {violations:?}"
    );
}

#[test]
fn every_seed_is_caught_and_a_seed_confined_to_test_code_passes() {
    let root = clean_engine_root();
    let live = RETIRED_SEEDS
        .iter()
        .map(|seed| format!("pub struct {seed};\n"))
        .collect::<Vec<_>>()
        .concat();
    write_crate(root.path(), "promptforge-internal/live", "", &live);
    let test_items = RETIRED_SEEDS
        .iter()
        .map(|seed| format!("    struct {seed};\n"))
        .collect::<Vec<_>>()
        .concat();
    let test_only = format!(
        "pub struct Live;\n// {}\n#[cfg(test)]\nmod tests {{\n{test_items}}}\n",
        RETIRED_SEEDS.join(" "),
    );
    write_crate(root.path(), "promptforge-internal/quiet", "", &test_only);
    let violations = retired_symbol_violations(root.path());
    let mut symbols: Vec<&str> = violations
        .iter()
        .map(|v| {
            RETIRED_SEEDS
                .iter()
                .copied()
                .find(|seed| v.contains(seed))
                .expect("a violation names a seed")
        })
        .collect();
    symbols.sort_unstable();
    let mut expected = RETIRED_SEEDS.to_vec();
    expected.sort_unstable();
    assert_eq!(symbols, expected, "{violations:?}");
    assert!(
        violations.iter().all(|v| !v.contains("quiet")),
        "the crate whose seeds sit in a comment and a cfg(test) module is clean: {violations:?}"
    );
}

#[test]
fn a_forbidden_dependency_in_a_container_crate_fails_the_guard() {
    let root = clean_engine_root();
    write_crate(
        root.path(),
        "promptforge-internal/store",
        "[dependencies]\ntokio = { version = \"1\", features = [\"rt\"] }\n",
        "pub struct Store;\n",
    );
    let violations = engine_guard_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("tokio") && violations[0].contains("store"),
        "the forbidden dependency is reported against its crate: {violations:?}"
    );
}

#[test]
fn a_missing_root_engine_crate_is_reported_not_skipped() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "promptforge-internal/engine",
        "[dependencies]\nserde = \"1\"\n",
        "pub struct Live;\n",
    );
    let facade_manifest = root
        .path()
        .join("crates")
        .join("promptforge")
        .join("Cargo.toml");
    let violations = engine_guard_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with(&facade_manifest.display().to_string())
            && violations[0].contains("unreadable manifest"),
        "{violations:?}"
    );
}
