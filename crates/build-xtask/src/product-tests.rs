//! Family-matrix fixtures: the dependency rules between product families,
//! the shell boundary, and the classification itself. Container-privacy
//! fixtures sit in `product-container-tests.rs`.

use super::test_support::{workspace_root, write_crate};
use super::*;

#[test]
fn workspace_respects_the_product_boundary() {
    let violations = product_boundary_violations(&workspace_root());
    assert!(
        violations.is_empty(),
        "product-boundary violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn workshop_depends_on_workshop_server_api_only() {
    let walk = workspace_crates(&workspace_root());
    assert!(walk.violations.is_empty(), "{:?}", walk.violations);
    let shell = walk
        .crates
        .iter()
        .find(|krate| krate.package == "workshop")
        .expect("the workshop shell crate is a workspace member");
    assert!(
        shell.deps.iter().any(|dep| dep == "workshop-server-api"),
        "the shell reaches the server through the api crate: {:?}",
        shell.deps
    );
    assert!(
        !shell.deps.iter().any(|dep| dep == "workshop-server"),
        "the shell never depends on workshop-server directly: {:?}",
        shell.deps
    );
}

#[test]
fn the_shell_re_adding_workshop_server_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop",
        "workshop",
        "[dependencies]\nworkshop-server-api = { path = \"../workshop-server-api\" }\n\
         [dev-dependencies]\nworkshop-server = { path = \"../workshop-server\" }\n",
    );
    write_crate(
        root.path(),
        "workshop-server-api",
        "workshop-server-api",
        "",
    );
    write_crate(root.path(), "workshop-server", "workshop-server", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("workshop depends on workshop-server:")
            && violations[0].contains("workshop-server-api"),
        "the violation names the shell, the forbidden dep, and the facade: {violations:?}"
    );
}

#[test]
fn other_workshop_crates_may_depend_on_workshop_server() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop-server-api",
        "workshop-server-api",
        "[dependencies]\nworkshop-server = { path = \"../workshop-server\" }\n",
    );
    write_crate(root.path(), "workshop-server", "workshop-server", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "the rule binds only the shell crate: {violations:?}"
    );
}

#[test]
fn an_outside_crate_reaching_past_the_one_public_crate_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop-sessions",
        "workshop-sessions",
        "[dependencies]\npromptforge-lua = { path = \"../promptforge-lua\" }\npromptforge-api-runtime = { path = \"../promptforge-api-runtime\" }\n",
    );
    write_crate(root.path(), "promptforge-lua", "promptforge-lua", "");
    write_crate(
        root.path(),
        "promptforge-api-runtime",
        "promptforge-api-runtime",
        "",
    );
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("workshop-sessions") && violations[0].contains("promptforge-lua"),
        "the violation names the crate and the forbidden dep: {violations:?}"
    );
}

#[test]
fn a_gateway_crate_depending_on_promptforge_api_runtime_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "gateway-routing",
        "gateway-routing",
        "[dependencies]\npromptforge-api-runtime = { path = \"../promptforge-api-runtime\" }\n",
    );
    write_crate(
        root.path(),
        "promptforge-api-runtime",
        "promptforge-api-runtime",
        "",
    );
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("gateway-routing"),
        "the violation names the gateway crate: {violations:?}"
    );
}

#[test]
fn a_shared_crate_depending_on_a_product_crate_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "shared-vfs",
        "shared-vfs",
        "[dependencies]\nworkshop-protocol = { path = \"../workshop-protocol\" }\n",
    );
    write_crate(root.path(), "workshop-protocol", "workshop-protocol", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("shared-vfs") && violations[0].contains("workshop-protocol"),
        "the violation names both crates: {violations:?}"
    );
}

#[test]
fn dev_build_and_target_dependencies_are_checked() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop-server",
        "workshop-server",
        "[dev-dependencies]\npromptforge-parser = { path = \"../promptforge-parser\" }\n\
         [target.'cfg(windows)'.dependencies]\ngateway-protocol = { path = \"../gateway-protocol\" }\n",
    );
    write_crate(root.path(), "promptforge-parser", "promptforge-parser", "");
    write_crate(root.path(), "gateway-protocol", "gateway-protocol", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(
        violations.len(),
        2,
        "the dev-dependency and the target-specific dependency are both reported: {violations:?}"
    );
}

#[test]
fn package_renames_are_resolved_before_classification() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop-sessions",
        "workshop-sessions",
        "[dependencies]\npf = { package = \"promptforge-store\", path = \"../promptforge-store\" }\n",
    );
    write_crate(root.path(), "promptforge-store", "promptforge-store", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(
        violations.len(),
        1,
        "the renamed dependency on promptforge-store is reported: {violations:?}"
    );
}

#[test]
fn a_promptforge_crate_depending_on_gateway_or_workshop_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "promptforge-api-runtime",
        "promptforge-api-runtime",
        "[dependencies]\ngateway-protocol = { path = \"../gateway-protocol\" }\nworkshop-protocol = { path = \"../workshop-protocol\" }\n",
    );
    write_crate(root.path(), "gateway-protocol", "gateway-protocol", "");
    write_crate(root.path(), "workshop-protocol", "workshop-protocol", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations
            .iter()
            .all(|v| v.contains("promptforge-api-runtime")),
        "the violations name the promptforge crate: {violations:?}"
    );
}

#[test]
fn an_outside_crate_depending_on_the_public_crates_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop-sessions",
        "workshop-sessions",
        "[dependencies]\npromptforge-api-runtime = { path = \"../promptforge-api-runtime\" }\n\
         promptforge-api-types = { path = \"../promptforge-api-types\" }\n",
    );
    write_crate(
        root.path(),
        "promptforge-api-runtime",
        "promptforge-api-runtime",
        "",
    );
    write_crate(
        root.path(),
        "promptforge-api-types",
        "promptforge-api-types",
        "",
    );
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "both public crates are legal dependencies for outside crates: {violations:?}"
    );
}

#[test]
fn an_unparseable_manifest_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    let dir = root.path().join("crates").join("broken");
    std::fs::create_dir_all(&dir).expect("the crate directory creates");
    std::fs::write(dir.join("Cargo.toml"), "not [valid toml").expect("the manifest writes");
    let violations = workspace_crates(root.path()).violations;
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("unparseable manifest"),
        "the violation reports the parse failure: {violations:?}"
    );
    assert!(
        product_boundary_violations(root.path()).is_empty(),
        "the read failure belongs to the marker check, not this one"
    );
}

#[test]
fn a_workshop_crate_depending_on_the_public_gateway_pair_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop-server",
        "workshop-server",
        "[dependencies]\ngateway-api-types = { path = \"../gateway-api-types\" }\n\
         gateway-api-discovery = { path = \"../gateway-api-discovery\" }\n",
    );
    write_crate(root.path(), "gateway-api-types", "gateway-api-types", "");
    write_crate(
        root.path(),
        "gateway-api-discovery",
        "gateway-api-discovery",
        "",
    );
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "the public gateway pair is legal for workshop crates: {violations:?}"
    );
}

#[test]
fn a_harness_crate_depending_on_the_public_doors_and_shared_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness/runner",
        "harness-runner",
        "[dependencies]\npromptforge-api-runtime = { path = \"../../promptforge-api-runtime\" }\n\
         promptforge-api-types = { path = \"../../promptforge-api-types\" }\n\
         gateway-api-types = { path = \"../../gateway-api-types\" }\n\
         gateway-api-discovery = { path = \"../../gateway-api-discovery\" }\n\
         shared-vfs = { path = \"../../shared-vfs\" }\n",
    );
    for name in [
        "promptforge-api-runtime",
        "promptforge-api-types",
        "gateway-api-types",
        "gateway-api-discovery",
        "shared-vfs",
    ] {
        write_crate(root.path(), name, name, "");
    }
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "the promptforge public pair, the gateway public pair, and shared-* are legal for harness crates: {violations:?}"
    );
}

#[test]
fn a_harness_crate_depending_on_a_workshop_crate_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness/sessions",
        "harness-sessions",
        "[dependencies]\nworkshop-registry = { path = \"../../workshop-registry\" }\n",
    );
    write_crate(root.path(), "workshop-registry", "workshop-registry", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("harness-sessions depends on workshop-registry:")
            && violations[0].contains("harness crates must not depend on workshop crates"),
        "the violation names the harness crate and the workshop dep: {violations:?}"
    );
}

#[test]
fn a_harness_crate_depending_on_a_private_gateway_crate_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness/models",
        "harness-models",
        "[dependencies]\ngateway-routing = { path = \"../../gateway-routing\" }\n",
    );
    write_crate(root.path(), "gateway-routing", "gateway-routing", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("harness-models depends on gateway-routing:")
            && violations[0].contains("gateway-api-types")
            && violations[0].contains("gateway-api-discovery"),
        "the violation names the harness crate and the public pair: {violations:?}"
    );
}

#[test]
fn a_harness_crate_reaching_past_the_promptforge_public_pair_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "harness/runner",
        "harness-runner",
        "[dependencies]\npromptforge-lua = { path = \"../../promptforge-lua\" }\n",
    );
    write_crate(root.path(), "promptforge-lua", "promptforge-lua", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("harness-runner depends on promptforge-lua:"),
        "the single-public-crate rule binds harness crates: {violations:?}"
    );
}

#[test]
fn a_workshop_crate_depending_on_harness_api_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop/server",
        "workshop-server",
        "[dependencies]\nharness-api = { path = \"../../harness-api\" }\n",
    );
    write_crate(root.path(), "harness-api", "harness-api", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "harness-api is the harness public crate for workshop crates: {violations:?}"
    );
}

#[test]
fn a_workshop_crate_depending_on_a_harness_crate_other_than_harness_api_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop/server",
        "workshop-server",
        "[dependencies]\nharness-runner = { path = \"../../harness-runner\" }\n",
    );
    write_crate(root.path(), "harness-runner", "harness-runner", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("workshop-server depends on harness-runner:")
            && violations[0].contains("harness-api"),
        "the violation names the workshop crate and the harness public crate: {violations:?}"
    );
}

#[test]
fn promptforge_gateway_and_shared_crates_depending_on_harness_are_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    let dep = "[dependencies]\nharness-api = { path = \"../harness-api\" }\n";
    write_crate(root.path(), "harness-api", "harness-api", "");
    write_crate(
        root.path(),
        "promptforge-api-runtime",
        "promptforge-api-runtime",
        dep,
    );
    write_crate(root.path(), "gateway-routing", "gateway-routing", dep);
    write_crate(root.path(), "shared-vfs", "shared-vfs", dep);
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 3, "{violations:?}");
    for package in ["promptforge-api-runtime", "gateway-routing", "shared-vfs"] {
        assert!(
            violations
                .iter()
                .any(|v| v.starts_with(&format!("{package} depends on harness-api:"))),
            "{package} depending on the harness public crate is reported: {violations:?}"
        );
    }
}

#[test]
fn family_classification_follows_the_naming_rules() {
    assert_eq!(family("promptforge-api-runtime"), Family::Promptforge);
    assert_eq!(family("gateway"), Family::Gateway);
    assert_eq!(family("gateway-config"), Family::Gateway);
    assert_eq!(family("workshop"), Family::Workshop);
    assert_eq!(family("workshop-server"), Family::Workshop);
    assert_eq!(family("shared-vfs"), Family::Shared);
    assert_eq!(family("build-xtask"), Family::Build);
    assert_eq!(family("serde"), Family::Unaffiliated);
}

#[test]
fn harness_family_classification_follows_the_name_prefix() {
    assert_eq!(family("harness-api"), Family::Harness);
    assert_eq!(family("harness-runner"), Family::Harness);
    assert_eq!(family("harness"), Family::Unaffiliated);
}

#[test]
fn the_bare_promptforge_package_belongs_to_the_promptforge_family() {
    assert_eq!(family("promptforge"), Family::Promptforge);
    assert_eq!(family("promptforger"), Family::Unaffiliated);
}
