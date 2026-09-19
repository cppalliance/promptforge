//! Fixture tests for the engine manifest guard: one manifest per case,
//! written into a temporary directory and scanned in isolation.

use std::path::PathBuf;

use super::*;

/// Write one manifest into a fresh temporary directory and return its path
/// beside the directory guard that keeps it alive.
fn manifest(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("Cargo.toml");
    std::fs::write(&path, format!("[package]\nname = \"fixture\"\n{text}"))
        .expect("the manifest writes");
    (dir, path)
}

#[test]
fn a_clean_engine_manifest_has_no_violations() {
    let (_dir, path) = manifest(
        "[dependencies]\nserde = \"1\"\nmlua = { version = \"0.10\", features = [\"lua54\"] }\n\
         [build-dependencies]\ncc = \"1\"\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn a_forbidden_crate_in_dependencies_is_reported() {
    let (_dir, path) =
        manifest("[dependencies]\ntokio = { version = \"1\", features = [\"rt\"] }\n");
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
    let rendered = violations[0].to_string();
    assert!(
        rendered.contains("[dependencies]") && rendered.contains("tokio"),
        "the violation names the table and the crate: {rendered}"
    );
}

#[test]
fn a_forbidden_crate_in_dev_dependencies_only_passes() {
    let (_dir, path) = manifest(
        "[dev-dependencies]\ntokio = { version = \"1\", features = [\"test-util\"] }\n\
         reqwest = \"0.12\"\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert!(
        violations.is_empty(),
        "dev-dependencies are outside the guard: {violations:?}"
    );
}

#[test]
fn an_optional_entry_enabled_only_by_test_support_passes() {
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ntest-support = [\"dep:tokio\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert!(
        violations.is_empty(),
        "the interim test-support exemption applies: {violations:?}"
    );
}

#[test]
fn an_optional_entry_enabled_by_another_feature_is_reported() {
    let (_dir, path) = manifest(
        "[dependencies]\nreqwest = { version = \"0.12\", optional = true }\n\
         [features]\ntest-support = [\"dep:reqwest\"]\nnet = [\"dep:reqwest\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(
        violations.len(),
        1,
        "a second enabling feature voids the exemption: {violations:?}"
    );
}

#[test]
fn a_dep_gated_entry_also_enabled_by_a_strong_feature_path_is_reported() {
    // `tokio/rt` enables the optional dependency even when `dep:` syntax
    // is in use, so `rt` is a second enabler and the exemption does not hold.
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ntest-support = [\"dep:tokio\"]\nrt = [\"tokio/rt\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn a_dep_gated_entry_with_only_a_weak_feature_path_passes() {
    // `tokio?/rt` enables the feature only if something else already
    // enabled the dependency; it is not an enabler on its own.
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ntest-support = [\"dep:tokio\"]\nrt = [\"tokio?/rt\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn an_entry_whose_test_support_gate_is_enabled_by_default_is_reported() {
    // `default` enables `test-support`, which enables the dependency in
    // every build, so the exemption does not hold.
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ndefault = [\"test-support\"]\ntest-support = [\"dep:tokio\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn an_optional_entry_with_an_implicit_feature_is_reported() {
    // Without `dep:` syntax cargo also creates the implicit feature `tokio`,
    // a second way to enable the dependency, so the exemption does not hold.
    let (_dir, path) = manifest(
        "[dependencies]\ntokio = { version = \"1\", optional = true }\n\
         [features]\ntest-support = [\"tokio\"]\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn build_and_target_specific_tables_are_scanned_and_renames_resolved() {
    let (_dir, path) = manifest(
        "[build-dependencies]\nasync-trait = \"0.1\"\n\
         [target.'cfg(windows)'.dependencies]\ntu = { package = \"tokio-util\", version = \"0.7\" }\n\
         [target.'cfg(unix)'.dev-dependencies]\ntokio = \"1\"\n",
    );
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 2, "{violations:?}");
    let rendered: Vec<String> = violations.iter().map(ToString::to_string).collect();
    assert!(
        rendered
            .iter()
            .any(|v| v.contains("[build-dependencies]") && v.contains("async-trait")),
        "the build-dependencies entry is reported: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|v| v.contains("cfg(windows)") && v.contains("tokio-util")),
        "the renamed target-specific entry is reported by package name: {rendered:?}"
    );
}

#[test]
fn an_unreadable_or_unparseable_manifest_is_reported() {
    let (dir, path) = manifest("not [valid toml");
    let violations = forbidden_engine_dependencies(&path);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].to_string().contains("unparseable manifest"),
        "{violations:?}"
    );
    let missing = forbidden_engine_dependencies(&dir.path().join("absent").join("Cargo.toml"));
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(
        missing[0].to_string().contains("unreadable manifest"),
        "{missing:?}"
    );
}
