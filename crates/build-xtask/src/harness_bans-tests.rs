//! Fixture tests for the harness clippy-ban check, plus the live check over
//! this workspace's `crates/harness-internal/` container and the coverage
//! of its `crates/harness/` facade.

use std::path::Path;

use super::*;

/// A fake workspace root; `container` and `facade` name its harness
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

/// Writes a crate directory with a manifest and, when given, a `clippy.toml`.
fn write_crate(dir: &Path, clippy: Option<&str>) {
    std::fs::create_dir_all(dir).expect("the crate directory creates");
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"fixture\"\n")
        .expect("the manifest writes");
    if let Some(text) = clippy {
        std::fs::write(dir.join("clippy.toml"), text).expect("clippy.toml writes");
    }
}

const COMPLETE: &str = "disallowed-methods = [\n\
    \"tokio::spawn\",\n\
    { path = \"tokio::task::spawn_blocking\", reason = \"spawn through harness-runner\" },\n\
]\n";

#[test]
fn an_absent_container_is_vacuously_clean() {
    let root = fake_root();
    let violations = harness_clippy_bans(&container(root.path()));
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn an_empty_container_is_vacuously_clean() {
    let root = fake_root();
    std::fs::create_dir_all(container(root.path())).expect("the container creates");
    let violations = harness_clippy_bans(&container(root.path()));
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn a_crate_missing_its_clippy_toml_is_reported() {
    let root = fake_root();
    write_crate(&container(root.path()).join("runner"), None);
    let violations = harness_clippy_bans(&container(root.path()));
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("runner") && violations[0].contains("clippy.toml"),
        "the missing file is named: {violations:?}"
    );
}

#[test]
fn a_clippy_toml_missing_a_banned_method_is_reported() {
    let root = fake_root();
    write_crate(
        &container(root.path()).join("runner"),
        Some("disallowed-methods = [\"tokio::spawn\"]\n"),
    );
    let violations = harness_clippy_bans(&container(root.path()));
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("tokio::task::spawn_blocking")
            && !violations[0].contains("tokio::spawn,"),
        "only the absent method is reported: {violations:?}"
    );
}

#[test]
fn a_clippy_toml_without_the_disallowed_methods_key_is_reported() {
    let root = fake_root();
    write_crate(
        &container(root.path()).join("runner"),
        Some("allow-unwrap-in-tests = true\n"),
    );
    let violations = harness_clippy_bans(&container(root.path()));
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("tokio::spawn") && violations[0].contains("spawn_blocking"),
        "both methods are reported absent: {violations:?}"
    );
}

#[test]
fn a_complete_clippy_toml_in_either_entry_form_passes() {
    let root = fake_root();
    write_crate(&container(root.path()).join("runner"), Some(COMPLETE));
    write_crate(
        &container(root.path()).join("log"),
        Some(
            "disallowed-methods = [\n\
             { path = \"tokio::spawn\" },\n\
             { path = \"tokio::task::spawn_blocking\" },\n\
             \"std::process::exit\",\n]\n",
        ),
    );
    let violations = harness_clippy_bans(&container(root.path()));
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn a_facade_without_a_clippy_toml_passes_though_it_is_a_harness_crate() {
    let root = fake_root();
    write_crate(&container(root.path()).join("runner"), Some(COMPLETE));
    write_crate(&facade(root.path()), None);
    assert!(
        harness_crates(&container(root.path()), &facade(root.path()))
            .contains(&facade(root.path())),
        "the facade is listed as a harness crate"
    );
    let violations = harness_clippy_bans(&container(root.path()));
    assert!(
        violations.is_empty(),
        "the facade defines nothing to spawn from, so it carries no ban: {violations:?}"
    );
}

#[test]
fn a_crate_nested_under_a_manifestless_subdirectory_is_checked() {
    let root = fake_root();
    write_crate(&container(root.path()).join("stt").join("engine"), None);
    let violations = harness_clippy_bans(&container(root.path()));
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("engine"), "{violations:?}");
}

#[test]
fn an_unparseable_clippy_toml_is_reported() {
    let root = fake_root();
    write_crate(
        &container(root.path()).join("runner"),
        Some("disallowed-methods = [ not toml\n"),
    );
    let violations = harness_clippy_bans(&container(root.path()));
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("unparseable"), "{violations:?}");
}

#[test]
fn the_harness_crates_are_the_eight_container_crates_and_the_facade() {
    let root = crate::product::test_support::workspace_root();
    let internal = root.join("crates").join("harness-internal");
    let covered = harness_crates(&internal, &root.join("crates").join("harness"));
    let expected = [
        internal.join("runner"),
        internal.join("models"),
        internal.join("capabilities"),
        internal.join("log"),
        internal.join("sessions"),
        // The first-party capabilities, moved in from the engine's
        // container with the traits they implement.
        internal.join("web"),
        internal.join("webfetch"),
        internal.join("web-search"),
        root.join("crates").join("harness"),
    ];
    assert_eq!(
        covered.len(),
        expected.len(),
        "the harness family is exactly nine crates; covered: {covered:?}"
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
fn harness_crates_ban_raw_tokio_spawns() {
    let root = crate::product::test_support::workspace_root();
    let violations = harness_clippy_bans(&root.join("crates").join("harness-internal"));
    assert!(
        violations.is_empty(),
        "harness clippy-ban violations:\n{}",
        violations.join("\n")
    );
}
