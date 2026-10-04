//! Fixture tests for the Harness tokio ban, plus the live check over this
//! workspace's `crates/harness-internal/` container and its
//! `crates/harness/` facade.

use std::path::Path;

use super::*;

/// A fake workspace root; `container` and `facade` name its Harness
/// directories whether or not they exist yet.
fn fake_root() -> tempfile::TempDir {
    tempfile::TempDir::new().expect("tempdir")
}

fn container(root: &Path) -> std::path::PathBuf {
    root.join("crates").join("harness-internal")
}

fn facade(root: &Path) -> std::path::PathBuf {
    root.join("crates").join("harness")
}

/// Writes a crate directory whose manifest is a fixture package followed by
/// `tables`.
fn write_crate(dir: &Path, tables: &str) {
    std::fs::create_dir_all(dir).expect("the crate directory creates");
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"fixture\"\n{tables}"),
    )
    .expect("the manifest writes");
}

fn bans(root: &Path) -> Vec<String> {
    harness_tokio_bans(&container(root), &facade(root))
}

#[test]
fn an_absent_container_is_vacuously_clean() {
    let root = fake_root();
    let violations = bans(root.path());
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn an_empty_container_is_vacuously_clean() {
    let root = fake_root();
    std::fs::create_dir_all(container(root.path())).expect("the container creates");
    let violations = bans(root.path());
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn tokio_as_a_normal_dependency_of_an_internal_crate_is_reported() {
    let root = fake_root();
    write_crate(
        &container(root.path()).join("runner"),
        "[dependencies]\ntokio = { workspace = true, features = [\"rt\"] }\n",
    );
    let violations = bans(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("runner")
            && violations[0].contains("[dependencies]")
            && violations[0].contains("tokio"),
        "the violation names the crate, the table, and the package: {violations:?}"
    );
}

#[test]
fn tokio_as_a_dev_dependency_passes() {
    let root = fake_root();
    write_crate(
        &container(root.path()).join("runner"),
        "[dependencies]\nfutures-util.workspace = true\n\
         [dev-dependencies]\ntokio = { workspace = true, features = [\"macros\", \"rt\"] }\n\
         tokio-util.workspace = true\n",
    );
    let violations = bans(root.path());
    assert!(
        violations.is_empty(),
        "the suites drive runs on tokio, so dev-dependencies are outside the ban: {violations:?}"
    );
}

#[test]
fn a_renamed_tokio_util_in_a_target_table_is_reported() {
    let root = fake_root();
    write_crate(
        &container(root.path()).join("plugins"),
        "[target.'cfg(windows)'.dependencies]\ntu = { package = \"tokio-util\", version = \"0.7\" }\n",
    );
    let violations = bans(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("cfg(windows)") && violations[0].contains("tokio-util"),
        "the target table and the package behind the rename are named: {violations:?}"
    );
}

#[test]
fn the_facade_is_held_to_the_ban() {
    let root = fake_root();
    write_crate(&container(root.path()).join("runner"), "");
    write_crate(&facade(root.path()), "[dependencies]\ntokio = \"1\"\n");
    let violations = bans(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains(&facade(root.path()).display().to_string()),
        "the facade's manifest is named: {violations:?}"
    );
}

#[test]
fn a_crate_nested_under_a_manifestless_subdirectory_is_checked() {
    let root = fake_root();
    write_crate(
        &container(root.path()).join("stt").join("engine"),
        "[dependencies]\ntokio = \"1\"\n",
    );
    let violations = bans(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("engine"), "{violations:?}");
}

#[test]
fn an_unparseable_manifest_is_reported() {
    let root = fake_root();
    write_crate(&container(root.path()).join("runner"), "[dependencies\n");
    let violations = bans(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("unparseable"), "{violations:?}");
}

#[test]
fn a_facade_directory_without_a_manifest_is_reported() {
    let root = fake_root();
    std::fs::create_dir_all(facade(root.path())).expect("the facade directory creates");
    let violations = bans(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("unreadable manifest"),
        "{violations:?}"
    );
}

#[test]
fn the_harness_crates_are_the_two_container_crates_and_the_facade() {
    let root = crate::product::test_support::workspace_root();
    let internal = root.join("crates").join("harness-internal");
    let covered = harness_crates(&internal, &root.join("crates").join("harness"));
    let expected = [
        internal.join("runner"),
        internal.join("plugins"),
        root.join("crates").join("harness"),
    ];
    assert_eq!(
        covered.len(),
        expected.len(),
        "the harness container holds two crates beside the facade; covered: {covered:?}"
    );
    for dir in &expected {
        assert!(
            covered.contains(dir),
            "{} is a harness crate; covered: {covered:?}",
            dir.display()
        );
    }
}

#[test]
fn harness_crates_declare_no_tokio_outside_dev_dependencies() {
    let root = crate::product::test_support::workspace_root();
    let crates_dir = root.join("crates");
    let violations = harness_tokio_bans(
        &crates_dir.join("harness-internal"),
        &crates_dir.join("harness"),
    );
    assert!(
        violations.is_empty(),
        "harness tokio-ban violations:\n{}",
        violations.join("\n")
    );
}
