//! Fixture tests for the `test-support` leak guard, plus the live check
//! over this workspace: no non-dev dependency table anywhere enables a
//! promptforge or harness crate's `test-support` feature.

use std::path::{Path, PathBuf};

use super::*;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("build-xtask lives at <root>/crates/build-xtask")
        .to_path_buf()
}

/// Writes one crate under `<root>/crates/<dir>/` named `name`, with the
/// given manifest body after `[package]`.
fn write_crate(root: &Path, dir: &str, name: &str, manifest: &str) {
    let crate_dir = root.join("crates").join(dir);
    std::fs::create_dir_all(crate_dir.join("src")).expect("the crate directory creates");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\n{manifest}"),
    )
    .expect("the manifest writes");
    std::fs::write(crate_dir.join("src").join("lib.rs"), "pub struct Live;\n")
        .expect("lib.rs writes");
}

/// A fake workspace holding three container engine crates, each exposing
/// a `test-support` feature, so a fixture can add one consumer and see
/// only that consumer's findings.
fn engine_root() -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("tempdir");
    let features = "[features]\ntest-support = []\n";
    write_crate(
        root.path(),
        "promptforge-internal/engine",
        "promptforge-engine",
        features,
    );
    write_crate(
        root.path(),
        "promptforge-internal/types",
        "promptforge-types",
        features,
    );
    write_crate(
        root.path(),
        "promptforge-internal/lua",
        "promptforge-lua",
        features,
    );
    root
}

/// `engine_root` plus one container harness crate, `harness-runner`,
/// exposing a `test-support` feature.
fn guarded_root() -> tempfile::TempDir {
    let root = engine_root();
    write_crate(
        root.path(),
        "harness/runner",
        "harness-runner",
        "[features]\ntest-support = []\n",
    );
    root
}

#[test]
fn no_non_dev_table_in_the_workspace_enables_an_engine_test_support_feature() {
    let violations = test_support_leak_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "test-support leaks:\n{}",
        violations.join("\n")
    );
}

#[test]
fn a_dependencies_table_enabling_an_engine_test_support_feature_is_reported() {
    let root = engine_root();
    write_crate(
        root.path(),
        "harness/capabilities",
        "harness-capabilities",
        "[dependencies]\n\
         promptforge-engine = { workspace = true, features = [\"test-support\"] }\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("[dependencies]")
            && violations[0].contains("promptforge-engine/test-support")
            && violations[0].contains("capabilities"),
        "the leak names the table, the feature, and the consuming crate: {violations:?}"
    );
}

#[test]
fn a_dev_dependencies_table_enabling_an_engine_test_support_feature_passes() {
    let root = engine_root();
    write_crate(
        root.path(),
        "harness/capabilities",
        "harness-capabilities",
        "[dependencies]\npromptforge-engine = { workspace = true }\n\
         [dev-dependencies]\n\
         promptforge-engine = { workspace = true, features = [\"test-support\"] }\n\
         [target.'cfg(unix)'.dev-dependencies]\n\
         promptforge-lua = { workspace = true, features = [\"test-support\"] }\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert!(
        violations.is_empty(),
        "dev-dependencies may enable test-support: {violations:?}"
    );
}

#[test]
fn build_and_target_tables_are_scanned_renames_resolved_and_non_engine_features_ignored() {
    let root = engine_root();
    write_crate(
        root.path(),
        "harness/runner",
        "harness-runner",
        "[build-dependencies]\n\
         rt = { package = \"promptforge-engine\", features = [\"test-support\"] }\n\
         [target.'cfg(windows)'.dependencies]\n\
         promptforge-lua = { workspace = true, features = [\"serialize\", \"test-support\"] }\n\
         [dependencies]\n\
         promptforge-types = { workspace = true, features = [\"serde\"] }\n\
         harness-capabilities = { workspace = true, features = [\"test-support\"] }\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations
            .iter()
            .any(|v| v.contains("[build-dependencies]")
                && v.contains("promptforge-engine/test-support")),
        "the renamed build-dependency is reported by package name: {violations:?}"
    );
    assert!(
        violations
            .iter()
            .any(|v| v.contains("cfg(windows)") && v.contains("promptforge-lua/test-support")),
        "the target-specific entry is reported: {violations:?}"
    );
}

#[test]
fn a_workspace_dependencies_entry_enabling_an_engine_test_support_feature_is_reported() {
    let root = engine_root();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers = []\n[workspace.dependencies]\n\
         promptforge-engine = { path = \"crates/promptforge-internal/engine\", features = [\"test-support\"] }\n\
         promptforge-lua = { path = \"crates/promptforge-internal/lua\" }\n",
    )
    .expect("the root manifest writes");
    let violations = test_support_leak_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("[workspace.dependencies]")
            && violations[0].contains("promptforge-engine/test-support"),
        "the inherited entry is reported against the root manifest: {violations:?}"
    );
}

#[test]
fn a_features_value_enabling_an_engine_test_support_feature_is_reported() {
    let root = engine_root();
    write_crate(
        root.path(),
        "workshop/server",
        "workshop-server",
        "[dependencies]\n\
         promptforge-engine = { workspace = true }\n\
         rt-types = { package = \"promptforge-types\", workspace = true, optional = true }\n\
         promptforge-lua = { workspace = true, optional = true }\n\
         harness-capabilities = { workspace = true }\n\
         [features]\n\
         default = [\"promptforge-engine/test-support\"]\n\
         types = [\"rt-types?/test-support\"]\n\
         fixtures = [\"dep:promptforge-lua\", \"harness-capabilities/test-support\", \"promptforge-lua/serialize\"]\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations.iter().any(|v| v.contains("[features] default")
            && v.contains("promptforge-engine/test-support")
            && v.contains("server")),
        "the plain dependency-feature reference names the feature, the engine crate, and the consuming crate: {violations:?}"
    );
    assert!(
        violations
            .iter()
            .any(|v| v.contains("[features] types")
                && v.contains("promptforge-types/test-support")),
        "the weak `?/` reference resolves its `package` rename: {violations:?}"
    );
}

#[test]
fn an_engine_crate_forwarding_its_own_test_support_feature_passes() {
    let root = engine_root();
    write_crate(
        root.path(),
        "promptforge-internal/engine",
        "promptforge-engine",
        "[dependencies]\npromptforge-lua = { workspace = true }\n\
         [features]\ntest-support = [\"promptforge-lua/test-support\"]\n\
         other = [\"promptforge-lua/serialize\"]\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert!(
        violations.is_empty(),
        "an engine crate's own test-support forwarding is gated by the guarded feature: {violations:?}"
    );
}

#[test]
fn the_facade_forwarding_the_engine_test_support_feature_passes() {
    let root = engine_root();
    write_crate(
        root.path(),
        "promptforge",
        "promptforge",
        "[dependencies]\npromptforge-engine = { workspace = true }\n\
         [features]\ntest-support = [\"promptforge-engine/test-support\"]\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert!(
        violations.is_empty(),
        "the facade is an engine crate, so forwarding its own test-support is gated: {violations:?}"
    );
}

#[test]
fn an_engine_crate_enabling_a_sibling_test_support_feature_outside_dev_is_reported() {
    let root = engine_root();
    write_crate(
        root.path(),
        "promptforge-internal/engine",
        "promptforge-engine",
        "[dependencies]\npromptforge-lua = { workspace = true, features = [\"test-support\"] }\n\
         [features]\ntest-support = []\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("promptforge-lua/test-support"),
        "{violations:?}"
    );
}

#[test]
fn a_dependencies_table_enabling_a_harness_test_support_feature_is_reported() {
    let root = guarded_root();
    write_crate(
        root.path(),
        "harness/models",
        "harness-models",
        "[dependencies]\n\
         harness-runner = { workspace = true, features = [\"test-support\"] }\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("[dependencies]")
            && violations[0].contains("harness-runner/test-support")
            && violations[0].contains("models"),
        "the leak names the table, the feature, and the consuming crate: {violations:?}"
    );
}

#[test]
fn a_dev_dependencies_table_enabling_a_harness_test_support_feature_passes() {
    let root = guarded_root();
    write_crate(
        root.path(),
        "harness/models",
        "harness-models",
        "[dependencies]\nharness-runner = { workspace = true }\n\
         [dev-dependencies]\n\
         harness-runner = { workspace = true, features = [\"test-support\"] }\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert!(
        violations.is_empty(),
        "dev-dependencies may enable a harness crate's test-support: {violations:?}"
    );
}

#[test]
fn a_harness_crate_forwarding_its_own_test_support_feature_passes() {
    let root = guarded_root();
    write_crate(
        root.path(),
        "harness/sessions",
        "harness-sessions",
        "[dependencies]\nharness-runner = { workspace = true }\n\
         [features]\ntest-support = [\"harness-runner/test-support\"]\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert!(
        violations.is_empty(),
        "a harness crate's own test-support forwarding a harness sibling's is gated: {violations:?}"
    );
}

#[test]
fn a_harness_crate_default_feature_enabling_a_sibling_test_support_is_reported() {
    let root = guarded_root();
    write_crate(
        root.path(),
        "harness/models",
        "harness-models",
        "[dependencies]\nharness-runner = { workspace = true }\n\
         [features]\ndefault = [\"harness-runner/test-support\"]\n\
         test-support = [\"harness-runner/test-support\"]\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("[features] default")
            && violations[0].contains("harness-runner/test-support")
            && violations[0].contains("models"),
        "only a crate's own test-support feature is exempt when forwarding a sibling's: {violations:?}"
    );
}

#[test]
fn a_test_support_feature_forwarding_into_the_other_family_is_reported() {
    let root = guarded_root();
    write_crate(
        root.path(),
        "harness/models",
        "harness-models",
        "[dependencies]\npromptforge-lua = { workspace = true }\n\
         [features]\ntest-support = [\"promptforge-lua/test-support\"]\n",
    );
    write_crate(
        root.path(),
        "promptforge-internal/engine",
        "promptforge-engine",
        "[dependencies]\nharness-runner = { workspace = true }\n\
         [features]\ntest-support = [\"harness-runner/test-support\"]\n",
    );
    let violations = test_support_leak_violations(root.path());
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations
            .iter()
            .any(|v| v.contains("models") && v.contains("promptforge-lua/test-support")),
        "a harness crate forwarding a promptforge crate's test-support is reported: {violations:?}"
    );
    assert!(
        violations
            .iter()
            .any(|v| v.contains("engine") && v.contains("harness-runner/test-support")),
        "a promptforge crate forwarding a harness crate's test-support is reported: {violations:?}"
    );
}
