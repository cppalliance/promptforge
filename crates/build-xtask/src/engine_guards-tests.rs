//! The live Engine guards over this workspace, plus fixture tests that a
//! forbidden dependency in an Engine crate fails the guard.

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

/// A fake workspace whose root Engine crates and clippy files are clean, so
/// a fixture can add one container crate and see only that crate's
/// findings.
fn clean_engine_root() -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("tempdir");
    test_support::write_engine_clippy_tomls(root.path());
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
    let violations = engine_dependency_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "engine manifest violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_engine_crate_set_is_the_root_crates_plus_every_container_member() {
    let root = clean_engine_root();
    write_crate(root.path(), "runvil-internal/lua", "", "pub struct Vm;\n");
    write_crate(
        root.path(),
        "runvil-internal/store",
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
            "runvil",
            "runvil-internal/lua",
            "runvil-internal/store",
            "runvil-plugin",
        ],
        "harness and gateway crates are outside the engine"
    );
}

#[test]
fn a_forbidden_dependency_in_the_plugin_contract_fails_the_guard() {
    let root = clean_engine_root();
    write_crate(
        root.path(),
        "runvil-plugin",
        "[dependencies]\nasync-trait = \"0.1\"\n",
        "pub trait Plugin {}\n",
    );
    let violations = engine_guard_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("async-trait") && violations[0].contains("runvil-plugin"),
        "the Plugin contract is an engine root crate, so the manifest guard binds it: {violations:?}"
    );
}

#[test]
fn the_facade_forwarding_the_engine_test_support_feature_passes_every_engine_guard() {
    let root = tempfile::TempDir::new().expect("tempdir");
    test_support::write_engine_clippy_tomls(root.path());
    test_support::write_crate(
        root.path(),
        "runvil-internal/engine",
        "runvil-engine",
        "[features]\ntest-support = []\n",
    );
    test_support::write_crate(root.path(), "runvil-internal/types", "runvil-types", "");
    test_support::write_crate(
        root.path(),
        "runvil",
        "runvil",
        "[dependencies]\nrunvil-engine = { path = \"../runvil-internal/engine\" }\n\
         [features]\ntest-support = [\"runvil-engine/test-support\"]\n",
    );
    test_support::write_crate(root.path(), "runvil-plugin", "runvil-plugin", "");
    let violations = engine_guard_violations(root.path());
    assert!(
        violations.is_empty(),
        "the facade is an engine crate, so forwarding its own test-support is gated: {violations:?}"
    );
}

#[test]
fn a_forbidden_dependency_in_a_container_crate_fails_the_guard() {
    let root = clean_engine_root();
    write_crate(
        root.path(),
        "runvil-internal/store",
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
    test_support::write_engine_clippy_tomls(root.path());
    write_crate(
        root.path(),
        "runvil-internal/engine",
        "[dependencies]\nserde = \"1\"\n",
        "pub struct Live;\n",
    );
    write_crate(
        root.path(),
        "runvil-plugin",
        "[dependencies]\nserde = \"1\"\n",
        "pub struct Live;\n",
    );
    let facade_manifest = root.path().join("crates").join("runvil").join("Cargo.toml");
    let violations = engine_guard_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with(&facade_manifest.display().to_string())
            && violations[0].contains("unreadable manifest"),
        "{violations:?}"
    );
}
