//! The unsafe allowlist over this workspace, plus fixtures that each way of
//! relaxing `unsafe_code` outside the four owned crates fails at its line,
//! text that only spells the attribute passes, and an empty scan fails.

use std::path::Path;

use super::*;
use crate::product::test_support::workspace_root;

/// Writes a crate at `crates/<dir>/` holding each `(path, text)` under its
/// `src/`.
fn write_crate(root: &Path, dir: &str, files: &[(&str, &str)]) {
    let krate = dir
        .split('/')
        .fold(root.join("crates"), |path, part| path.join(part));
    let src = krate.join("src");
    std::fs::create_dir_all(&src).expect("the crate source directory creates");
    std::fs::write(
        krate.join("Cargo.toml"),
        format!("[package]\nname = \"{}\"\n", dir.replace('/', "-")),
    )
    .expect("the manifest writes");
    for (path, text) in files {
        std::fs::write(src.join(path), text).expect("the source file writes");
    }
}

const RELAXES: &str = "#![expect(unsafe_code, reason = \"raw FFI\")]\n\
    #[cfg_attr(windows, allow(unsafe_code))]\nmod sys {}\n";

#[test]
fn the_workspace_relaxes_unsafe_code_only_in_its_owned_crates() {
    let violations = unsafe_allowlist_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "unsafe allowlist violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_four_owned_crates_may_relax_unsafe_code_and_text_that_spells_it_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    for dir in [
        "gateway/app",
        "gateway/stt/whisper-ffi",
        "gateway-api-discovery",
        "workshop/desktop",
    ] {
        write_crate(root.path(), dir, &[("lib.rs", RELAXES)]);
    }
    write_crate(
        root.path(),
        "build-fixture",
        &[(
            "lib.rs",
            "#![deny(unsafe_code)]\n\
             // #[allow(unsafe_code)] in a comment\n\
             /// Docs that say `#[expect(unsafe_code)]`.\n\
             pub const TEXT: &str = \"#[allow(unsafe_code)]\";\n\
             pub const RAW: &str = r#\"#![expect(unsafe_code)]\"#;\n\
             #[allow(dead_code)]\nfn unused() {}\n",
        )],
    );
    let violations = unsafe_allowlist_violations(root.path());
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn each_way_of_relaxing_unsafe_code_outside_the_owned_crates_is_reported_at_its_line() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway/local",
        &[(
            "lib.rs",
            "//! Local inference.\n\
             #![cfg_attr(windows, allow(unsafe_code))]\n\
             #[expect(unsafe_code, reason = \"process priority\")]\n\
             mod priority {}\n\
             #[allow(dead_code, unsafe_code)]\n\
             mod probe {}\n\
             #[cfg_attr(unix, cfg_attr(test, expect(unsafe_code)))]\n\
             mod nested {}\n\
             fn body() {\n    #[allow(unsafe_code)]\n    let _x = 1;\n}\n",
        )],
    );
    let violations = unsafe_allowlist_violations(root.path());
    assert_eq!(violations.len(), 5, "{violations:?}");
    for (violation, line) in violations.iter().zip(["2", "3", "5", "7", "10"]) {
        assert!(
            violation.contains(&format!("lib.rs:{line}: ")) && violation.contains("unsafe_code"),
            "each relaxation is named at its line: {violations:?}"
        );
    }
}

#[test]
fn a_source_file_that_does_not_parse_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway/local",
        &[("lib.rs", "fn broken( {\n")],
    );
    let violations = unsafe_allowlist_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("lib.rs") && violations[0].contains("unparseable"),
        "a file that was never scanned cannot be shown clean: {violations:?}"
    );
}

#[test]
fn a_crate_whose_manifest_does_not_parse_is_still_scanned() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway/local",
        &[("lib.rs", "#[allow(unsafe_code)]\nmod sys {}\n")],
    );
    std::fs::write(
        root.path()
            .join("crates")
            .join("gateway")
            .join("local")
            .join("Cargo.toml"),
        "not [valid toml",
    )
    .expect("the manifest rewrites");
    let violations = unsafe_allowlist_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("lib.rs:1: ") && violations[0].contains("relaxes `unsafe_code`"),
        "a crate whose manifest was never read cannot be shown clean: {violations:?}"
    );
}

#[test]
fn an_unsafe_allowlist_scan_that_finds_no_source_fails() {
    let root = tempfile::TempDir::new().expect("tempdir");
    std::fs::create_dir_all(root.path().join("crates")).expect("the crates directory creates");
    let violations = unsafe_allowlist_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("scanned nothing"),
        "a check that read no source cannot show it clean: {violations:?}"
    );
}
