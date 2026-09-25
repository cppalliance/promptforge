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
fn family_crates_have_the_invariants_marker() {
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
fn a_harness_crate_with_the_marker_is_held_to_the_ceiling() {
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
fn a_harness_crate_whose_manifest_has_no_package_name_is_reported_not_skipped() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness/runner",
        "harness-runner",
        UNMARKED,
        MAX_FILE_LINES + 1,
    );
    let manifest = root
        .path()
        .join("crates")
        .join("harness")
        .join("runner")
        .join("Cargo.toml");
    std::fs::write(&manifest, "[lints]\nworkspace = true\n").expect("the manifest rewrites");
    let violations = marker_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("runner") && violations[0].contains("no package name"),
        "the directory and the failure mode are named: {violations:?}"
    );
    let ceiling = file_ceiling_violations(root.path());
    assert_eq!(
        ceiling.len(),
        1,
        "a crate with no readable name is not exempt from the ceiling: {ceiling:?}"
    );
}

#[test]
fn a_manifest_read_failure_is_reported_once_across_the_checks() {
    let root = tempfile::TempDir::new().expect("tempdir");
    let dir = root.path().join("crates").join("broken");
    std::fs::create_dir_all(&dir).expect("the crate directory creates");
    std::fs::write(dir.join("Cargo.toml"), "not [valid toml").expect("the manifest writes");
    let violations = marker_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("unparseable manifest"),
        "the marker check owns the walk's read failures: {violations:?}"
    );
    assert!(
        crate::product::product_boundary_violations(root.path()).is_empty(),
        "the two checks share one walk and report its read failures once"
    );
}

#[test]
fn a_crate_nested_under_a_subsystem_container_is_held_to_the_ceiling() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway/stt/engine",
        "gateway-stt-engine",
        MARKED,
        MAX_FILE_LINES + 1,
    );
    let violations = file_ceiling_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("big.rs"),
        "the walk reaches a crate three levels under crates/: {violations:?}"
    );
}

#[test]
fn the_tidy_checks_and_the_product_checks_enumerate_the_same_crates() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(root.path(), "harness/runner", "harness-runner", MARKED, 1);
    write_crate(
        root.path(),
        "gateway/stt/engine",
        "gateway-stt-engine",
        MARKED,
        1,
    );
    assert_eq!(
        participating_crates(root.path()).len(),
        crate::product::workspace_crates(root.path()).crates.len(),
        "the two walks find the same crates"
    );
}

#[test]
fn the_workshop_shell_without_the_marker_passes_and_stays_outside_the_ceiling() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop/desktop",
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
        "a non-family crate is never required to have the marker"
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

/// Writes `text` into `crates/gateway/app/src/<relative>`, creating the
/// gateway app crate's source tree as needed.
fn write_app_file(root: &Path, relative: &str, text: &str) {
    let path = root
        .join("crates")
        .join("gateway")
        .join("app")
        .join("src")
        .join(relative);
    std::fs::create_dir_all(path.parent().expect("the file has a parent"))
        .expect("the source directory creates");
    std::fs::write(&path, text).expect("the source file writes");
}

/// A spelled tier path in code, the shape the rule is meant to catch.
const NAMES_TIER: &str = "use crate::admin::walled::hf::HfProxy;\n";

#[test]
fn a_file_outside_the_walled_tier_naming_a_tier_module_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_app_file(root.path(), "speech.rs", NAMES_TIER);
    let violations = walled_tier_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("speech.rs") && violations[0].contains(WALLED_PATH),
        "the file and the path it names are reported: {violations:?}"
    );
}

#[test]
fn each_allowlisted_assembly_site_may_name_the_walled_tier() {
    for allowed in WALLED_ALLOWLIST {
        let root = tempfile::TempDir::new().expect("tempdir");
        let relative = allowed
            .strip_prefix("crates/gateway/app/src/")
            .expect("the allowlist keys files in the gateway app's source tree");
        write_app_file(root.path(), relative, NAMES_TIER);
        let violations = walled_tier_violations(root.path());
        assert!(
            violations.is_empty(),
            "{allowed} assembles the tier's own state and may name it: {violations:?}"
        );
    }
}

#[test]
fn a_comment_line_that_spells_the_tier_path_is_not_a_dependency() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_app_file(
        root.path(),
        "commands-apply.rs",
        "//! The route side lives in `crate::admin::walled::config_apply`.\n\
         /// See [`crate::admin::walled::config`].\n\
         pub(crate) fn apply() {}\n",
    );
    let violations = walled_tier_violations(root.path());
    assert!(
        violations.is_empty(),
        "prose naming a module path is documentation, not a dependency: {violations:?}"
    );
}

#[test]
fn the_walled_tiers_own_modules_may_name_each_other() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_app_file(root.path(), "admin/walled/system.rs", NAMES_TIER);
    let violations = walled_tier_violations(root.path());
    assert!(
        violations.is_empty(),
        "a module inside the tier is not outside it: {violations:?}"
    );
}

/// A name no UTF-8 string can hold: a stray byte on unix, an unpaired
/// surrogate on windows.
#[cfg(unix)]
fn undecodable_name() -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(b"lib\xff".to_vec())
}

#[cfg(windows)]
fn undecodable_name() -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    std::ffi::OsString::from_wide(&[u16::from(b'l'), u16::from(b'i'), u16::from(b'b'), 0xd800])
}

#[test]
fn an_undecodable_path_component_keeps_its_place_in_the_relative_path() {
    let root = Path::new("root");
    let file = root
        .join("crates")
        .join(undecodable_name())
        .join("src")
        .join("lib.rs");
    let relative = slash_path(root, &file);
    assert_eq!(
        relative.split('/').count(),
        4,
        "a component that does not decode must not vanish and let the path \
         collide with an allowlist entry: {relative}"
    );
}

#[test]
fn no_file_outside_the_walled_tier_names_its_modules() {
    let violations = walled_tier_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "walled tier violations:\n{}",
        violations.join("\n")
    );
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
