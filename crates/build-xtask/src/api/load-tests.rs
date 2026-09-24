//! Internal crate discovery and rustdoc JSON from another nightly.

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
