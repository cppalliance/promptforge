//! Container-privacy fixtures: the manifestless `crates/<family>/`
//! containers are private to their families, with the named exceptions.

use super::product_boundary_violations;
use super::test_support::write_crate;

#[test]
fn an_outside_crate_depending_into_the_private_container_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop-sessions",
        "workshop-sessions",
        "[dependencies]\npromptforge-lua = { path = \"../promptforge-internal/lua\" }\n",
    );
    write_crate(
        root.path(),
        "promptforge-internal/lua",
        "promptforge-lua",
        "",
    );
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("workshop-sessions depends on promptforge-lua")
            && violations[0].contains("crates/promptforge-internal is private to its family")
            && violations[0].contains("promptforge-api-runtime"),
        "the violation includes the container privacy message: {violations:?}"
    );
}

#[test]
fn a_container_crate_depending_on_a_container_sibling_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "promptforge-internal/parser",
        "promptforge-parser",
        "[dependencies]\npromptforge-lua = { path = \"../lua\" }\n",
    );
    write_crate(
        root.path(),
        "promptforge-internal/lua",
        "promptforge-lua",
        "",
    );
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "container siblings may depend on each other: {violations:?}"
    );
}

#[test]
fn the_public_runtime_depending_into_the_container_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "promptforge-api-runtime",
        "promptforge-api-runtime",
        "[dependencies]\npromptforge-lua = { path = \"../promptforge-internal/lua\" }\n",
    );
    write_crate(
        root.path(),
        "promptforge-internal/lua",
        "promptforge-lua",
        "",
    );
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "the named public crate may depend into the container: {violations:?}"
    );
}

#[test]
fn an_outside_crate_depending_into_the_gateway_container_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "promptforge-api-runtime",
        "promptforge-api-runtime",
        "[dependencies]\ngateway-protocol = { path = \"../gateway/protocol\" }\n",
    );
    write_crate(root.path(), "gateway/protocol", "gateway-protocol", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("promptforge-api-runtime depends on gateway-protocol")
            && violations[0].contains("crates/gateway is private to its family")
            && violations[0].contains("no outside crate"),
        "the violation includes the gateway container privacy message: {violations:?}"
    );
}

#[test]
fn gateway_container_siblings_may_depend_on_each_other() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway/app",
        "gateway",
        "[dependencies]\ngateway-protocol = { path = \"../protocol\" }\n",
    );
    write_crate(root.path(), "gateway/protocol", "gateway-protocol", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "gateway container siblings may depend on each other: {violations:?}"
    );
}

#[test]
fn an_outside_crate_depending_into_the_workshop_container_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "outside-tool",
        "outside-tool",
        "[dependencies]\nworkshop-protocol = { path = \"../workshop/protocol\" }\n",
    );
    write_crate(root.path(), "workshop/protocol", "workshop-protocol", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("outside-tool depends on workshop-protocol")
            && violations[0].contains("crates/workshop is private to its family")
            && violations[0].contains("no outside crate"),
        "the violation includes the workshop container privacy message: {violations:?}"
    );
}

#[test]
fn workshop_container_siblings_may_depend_on_each_other() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop/server",
        "workshop-server",
        "[dependencies]\nworkshop-protocol = { path = \"../protocol\" }\n",
    );
    write_crate(root.path(), "workshop/protocol", "workshop-protocol", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "workshop container siblings may depend on each other: {violations:?}"
    );
}

#[test]
fn a_build_crate_depending_into_a_container_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "build-workshop",
        "build-workshop",
        "[dependencies]\nworkshop-protocol = { path = \"../workshop/protocol\" }\n",
    );
    write_crate(root.path(), "workshop/protocol", "workshop-protocol", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "the build-* crates are meta tooling, exempt from container privacy: {violations:?}"
    );
}

#[test]
fn a_workshop_crate_depending_into_the_gateway_container_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop-server",
        "workshop-server",
        "[dependencies]\ngateway-local = { path = \"../gateway/local\" }\n",
    );
    write_crate(root.path(), "gateway/local", "gateway-local", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("workshop-server depends on gateway-local")
            && violations[0].contains("crates/gateway is private to its family"),
        "the violation includes the gateway container privacy message: {violations:?}"
    );
}

#[test]
fn a_family_crate_depending_into_the_stt_subsystem_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway/app",
        "gateway",
        "[dependencies]\ngateway-stt-engine = { path = \"../stt/engine\" }\n",
    );
    write_crate(root.path(), "gateway/stt/engine", "gateway-stt-engine", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("gateway depends on gateway-stt-engine")
            && violations[0].contains("crates/gateway/stt is private to its family")
            && violations[0].contains("gateway-stt"),
        "the violation names the subsystem and its public member: {violations:?}"
    );
}

#[test]
fn the_stt_public_member_is_visible_to_the_gateway_family() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway/app",
        "gateway",
        "[dependencies]\ngateway-stt = { path = \"../stt/api\" }\n",
    );
    write_crate(root.path(), "gateway/stt/api", "gateway-stt", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "the subsystem's public member is visible one level higher: {violations:?}"
    );
}

#[test]
fn an_outside_crate_depending_into_the_harness_container_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop/server",
        "workshop-server",
        "[dependencies]\nharness-runner = { path = \"../../harness/runner\" }\n",
    );
    write_crate(root.path(), "harness/runner", "harness-runner", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("workshop-server depends on harness-runner")
            && violations[0].contains("crates/harness is private to its family")
            && violations[0].contains("harness-api"),
        "the violation includes the harness container privacy message: {violations:?}"
    );
}

#[test]
fn the_public_harness_crate_depending_into_the_harness_container_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness-api",
        "harness-api",
        "[dependencies]\nharness-runner = { path = \"../harness/runner\" }\n\
         harness-sessions = { path = \"../harness/sessions\" }\n",
    );
    write_crate(root.path(), "harness/runner", "harness-runner", "");
    write_crate(root.path(), "harness/sessions", "harness-sessions", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "harness-api is the one outside crate permitted into crates/harness: {violations:?}"
    );
}

#[test]
fn harness_container_siblings_may_depend_on_each_other() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness/sessions",
        "harness-sessions",
        "[dependencies]\nharness-capabilities = { path = \"../capabilities\" }\n",
    );
    write_crate(
        root.path(),
        "harness/capabilities",
        "harness-capabilities",
        "",
    );
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "harness container siblings may depend on each other: {violations:?}"
    );
}

#[test]
fn stt_subsystem_siblings_may_depend_on_each_other() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway/stt/backend-whisper",
        "gateway-stt-backend-whisper",
        "[dependencies]\ngateway-stt-engine = { path = \"../engine\" }\n",
    );
    write_crate(root.path(), "gateway/stt/engine", "gateway-stt-engine", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "subsystem siblings may depend on each other: {violations:?}"
    );
}
