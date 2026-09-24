//! The pinned toolchain: refusal on any other toolchain, and agreement
//! between the pin, the workspace manifest, and `rustdoc-types` itself.

use super::*;

#[test]
fn a_toolchain_other_than_the_pinned_nightly_fails_naming_it() {
    for active in [
        "stable-x86_64-pc-windows-msvc",
        "nightly",
        "nightly-2026-09-06",
    ] {
        let message = require_pinned(Some(active)).expect_err(active);
        assert!(
            message.contains(&format!("`{}`", PINNED.nightly)),
            "{message}"
        );
        assert!(message.contains(&format!("`{active}`")), "{message}");
        assert!(
            message.contains(&format!("cargo +{} xtask api", PINNED.nightly)),
            "{message}"
        );
    }
}

#[test]
fn an_unset_toolchain_fails_naming_the_pinned_nightly() {
    let message = require_pinned(None).expect_err("no toolchain is refused");
    assert!(message.contains(PINNED.nightly), "{message}");
    assert!(message.contains("RUSTUP_TOOLCHAIN is unset"), "{message}");
}

#[test]
fn the_pinned_nightly_passes_with_or_without_a_host_triple() {
    let with_triple = format!("{}-x86_64-pc-windows-msvc", PINNED.nightly);
    assert_eq!(require_pinned(Some(PINNED.nightly)), Ok(()));
    assert_eq!(require_pinned(Some(&with_triple)), Ok(()));
    let bare_dash = format!("{}-", PINNED.nightly);
    assert!(require_pinned(Some(&bare_dash)).is_err());
}

#[test]
fn the_command_refuses_another_toolchain_before_building_anything() {
    let empty = tempfile::TempDir::new().expect("tempdir");
    let args = ["--check".to_owned()];
    let message = super::super::outcome(empty.path(), &args, Some("stable-x86_64-pc-windows-msvc"))
        .expect_err("stable is refused");
    assert!(
        message.starts_with("cargo xtask api: required the pinned nightly"),
        "{message}"
    );
    assert!(message.contains(PINNED.nightly), "{message}");
}

#[test]
fn an_unknown_argument_is_refused_with_the_usage() {
    let empty = tempfile::TempDir::new().expect("tempdir");
    let message = super::super::outcome(empty.path(), &["--fix".to_owned()], Some(PINNED.nightly))
        .expect_err("an unknown flag is refused");
    assert_eq!(message, super::super::USAGE);
}

#[test]
fn the_pinned_rustdoc_types_release_reads_the_pinned_format() {
    let minor = PINNED
        .rustdoc_types
        .split('.')
        .nth(1)
        .and_then(|minor| minor.parse::<u32>().ok());
    assert_eq!(minor, Some(rustdoc_types::FORMAT_VERSION));
}

#[test]
fn the_workspace_manifest_pins_the_same_rustdoc_types_release() {
    let root = crate::product::test_support::workspace_root();
    let text = std::fs::read_to_string(root.join("Cargo.toml")).expect("the root manifest reads");
    let manifest: toml::Value = toml::from_str(&text).expect("the root manifest parses");
    let pin = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.get("rustdoc-types"))
        .and_then(toml::Value::as_str);
    assert_eq!(pin, Some(format!("={}", PINNED.rustdoc_types).as_str()));
}
