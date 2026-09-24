//! Internal crate discovery, rustdoc JSON from another nightly, and the
//! `test-support` build: the test drivers the facade exposes only under
//! that feature are closure checked too, and stay out of the listing.

use super::super::fixture::{leak, workspace};
use super::super::report;
use super::*;

#[test]
fn internal_packages_are_every_container_crate_by_package_name_sorted() {
    let root = tempfile::TempDir::new().expect("tempdir");
    crate::product::test_support::write_crate(
        root.path(),
        "promptforge-internal/zeta",
        "promptforge-zeta",
        "",
    );
    crate::product::test_support::write_crate(
        root.path(),
        "promptforge-internal/alpha",
        "promptforge-alpha",
        "",
    );
    assert_eq!(
        internal_packages(root.path()),
        Ok(vec![
            "promptforge-alpha".to_owned(),
            "promptforge-zeta".to_owned()
        ])
    );
}

#[test]
fn an_internal_manifest_without_a_package_name_is_an_error() {
    let root = tempfile::TempDir::new().expect("tempdir");
    let dir = root
        .path()
        .join("crates")
        .join("promptforge-internal")
        .join("broken");
    std::fs::create_dir_all(&dir).expect("the crate directory creates");
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").expect("the manifest writes");
    let error = internal_packages(root.path()).expect_err("an unnamed crate is refused");
    assert!(error.ends_with("no readable package name"), "{error}");
}

#[test]
fn json_in_another_format_version_names_the_pinned_nightly() {
    let doc = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(doc.path().join("stale.json"), "{\"format_version\": 1}")
        .expect("the JSON writes");
    let error = read_crate(doc.path(), "stale").expect_err("another format is refused");
    assert!(
        error.contains(&format!(
            "required rustdoc JSON format_version {FORMAT_VERSION} (from `{}`, read by \
             rustdoc-types {}), found 1",
            PINNED.nightly, PINNED.rustdoc_types
        )),
        "{error}"
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn the_test_support_build_is_closure_checked_and_left_out_of_the_listing() {
    let root = workspace(
        "//! Inner.\n\n/// Kept internal.\npub struct Secret;\n\n/// Exposed.\npub struct Visible;\n\n\
         /// A test driver.\n#[cfg(feature = \"test-support\")]\npub fn driver() -> Secret {\n    Secret\n}\n",
        "//! Facade.\npub use promptforge_inner::Visible;\n\n\
         /// Test drivers.\n#[cfg(feature = \"test-support\")]\npub mod test_support {\n    \
         pub use promptforge_inner::driver;\n}\n",
    );
    let report = report(root.path(), &Build::ALL).expect("the fixture documents");
    let leak = leak("promptforge_inner");
    let findings: Vec<(String, Vec<Build>)> = report
        .findings
        .iter()
        .map(|(finding, builds)| (finding.to_string(), builds.iter().copied().collect()))
        .collect();
    assert_eq!(
        findings,
        [(
            format!(
                "promptforge::test_support::driver: mentions `promptforge_inner::Secret` in its \
                 signature: {leak}"
            ),
            vec![Build::TestSupport]
        )]
    );
    assert!(
        report
            .listing
            .contains(&"pub struct promptforge::Visible".to_owned())
    );
    assert!(
        report
            .listing
            .iter()
            .all(|line| !line.contains("test_support")),
        "{:?}",
        report.listing
    );
}
