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
fn family_crates_carry_the_invariants_marker() {
    let violations = marker_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "marker violations:\n{}",
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

/// Writes a crate under `crates/<dir>/` named `name`, with the given
/// `lib.rs` docs and one source file of `lines` lines.
fn write_crate(root: &Path, dir: &str, name: &str, lib_docs: &str, lines: usize) {
    let src = root.join("crates").join(dir).join("src");
    std::fs::create_dir_all(&src).expect("the crate source directory creates");
    std::fs::write(
        src.parent().expect("src has a parent").join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\n[lints]\nworkspace = true\n"),
    )
    .expect("the manifest writes");
    std::fs::write(src.join("lib.rs"), lib_docs).expect("lib.rs writes");
    std::fs::write(src.join("big.rs"), "// line\n".repeat(lines)).expect("big.rs writes");
}

const MARKED: &str = "//! Effect loop.\n//!\n//! ## Invariants\n//!\n//! - none\n";
const UNMARKED: &str = "//! Effect loop, with no invariants block.\n";

#[test]
fn a_harness_crate_carrying_the_marker_is_held_to_the_ceiling() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness/runner",
        "harness-runner",
        MARKED,
        MAX_FILE_LINES + 1,
    );
    let violations = file_ceiling_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("big.rs") && violations[0].contains("over the 500-line ceiling"),
        "the oversized harness file is reported: {violations:?}"
    );
    assert!(
        marker_violations(root.path()).is_empty(),
        "a marked family crate is not a marker violation"
    );
}

#[test]
fn a_harness_crate_without_the_marker_is_a_violation_and_still_held_to_the_ceiling() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness/runner",
        "harness-runner",
        UNMARKED,
        MAX_FILE_LINES + 1,
    );
    let violations = marker_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("harness-runner") && violations[0].contains(INVARIANT_MARKER),
        "the crate and the missing marker are named: {violations:?}"
    );
    let ceiling = file_ceiling_violations(root.path());
    assert_eq!(
        ceiling.len(),
        1,
        "family membership, not the marker, opts the crate into the ceiling: {ceiling:?}"
    );
}

#[test]
fn the_workshop_shell_without_the_marker_passes_and_stays_outside_the_ceiling() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop/shell",
        "workshop",
        UNMARKED,
        MAX_FILE_LINES + 1,
    );
    assert!(
        marker_violations(root.path()).is_empty(),
        "the shell is exempt from the marker"
    );
    assert!(
        file_ceiling_violations(root.path()).is_empty(),
        "the unmarked shell does not participate in the ceiling"
    );
}

#[test]
fn a_marked_crate_outside_the_families_still_participates_in_the_ceiling() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "build-fixture",
        "build-fixture",
        MARKED,
        MAX_FILE_LINES + 1,
    );
    let violations = file_ceiling_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("big.rs"),
        "the marker alone opts a crate in: {violations:?}"
    );
    assert!(
        marker_violations(root.path()).is_empty(),
        "a non-family crate is never required to carry the marker"
    );
}

#[test]
fn an_unmarked_crate_outside_the_families_is_left_alone() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway/local",
        "gateway-local",
        UNMARKED,
        MAX_FILE_LINES + 1,
    );
    assert!(marker_violations(root.path()).is_empty());
    assert!(file_ceiling_violations(root.path()).is_empty());
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
