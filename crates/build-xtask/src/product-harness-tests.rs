//! Harness-family fixtures: harness crates depend on `promptforge` and no
//! other product family, and outside crates reach the family only through
//! `harness`.

use super::product_boundary_violations;
use super::test_support::write_crate;

#[test]
fn a_harness_crate_depending_on_the_gateway_public_types_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness-internal/sessions",
        "harness-sessions",
        "[dependencies]\ngateway-api-types = { path = \"../../gateway-api-types\" }\n",
    );
    write_crate(root.path(), "gateway-api-types", "gateway-api-types", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("harness-sessions depends on gateway-api-types:")
            && violations[0].contains("harness crates must not depend on gateway crates"),
        "the gateway public pair is closed to harness crates: {violations:?}"
    );
}

#[test]
fn a_harness_crate_depending_on_a_shared_crate_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness-internal/log",
        "harness-log",
        "[dependencies]\nshared-error-source = { path = \"../../shared-error-source\" }\n",
    );
    write_crate(
        root.path(),
        "shared-error-source",
        "shared-error-source",
        "",
    );
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("harness-log depends on shared-error-source:")
            && violations[0].contains("harness crates must not depend on shared crates"),
        "shared-* crates are closed to harness crates: {violations:?}"
    );
}

#[test]
fn a_harness_crate_depending_on_workspace_hack_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness-internal/runner",
        "harness-runner",
        "[dependencies]\nworkspace-hack = { path = \"../../workspace-hack\" }\n",
    );
    write_crate(root.path(), "workspace-hack", "workspace-hack", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "workspace-hack belongs to no family, so harness crates may name it: {violations:?}"
    );
}

#[test]
fn a_non_workshop_outside_crate_depending_past_the_harness_facade_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "outside-tool",
        "outside-tool",
        "[dependencies]\nharness-runner = { path = \"../harness-runner\" }\n",
    );
    write_crate(root.path(), "harness-runner", "harness-runner", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("outside-tool depends on harness-runner:")
            && violations[0]
                .ends_with("outside crates may depend on the harness family only through harness"),
        "the harness facade rule binds every outside crate: {violations:?}"
    );
}

#[test]
fn a_build_crate_depending_past_the_harness_facade_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "build-xtask",
        "build-xtask",
        "[dependencies]\nharness-runner = { path = \"../harness-internal/runner\" }\n",
    );
    write_crate(root.path(), "harness-internal/runner", "harness-runner", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("build-xtask depends on harness-runner:")
            && violations[0]
                .ends_with("outside crates may depend on the harness family only through harness"),
        "container privacy exempts build-* crates, but the harness facade rule does not: {violations:?}"
    );
}
