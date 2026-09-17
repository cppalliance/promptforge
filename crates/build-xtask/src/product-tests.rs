use super::*;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("build-xtask lives at <root>/crates/build-xtask")
        .to_path_buf()
}

/// Write a minimal crate manifest into a fake workspace; `dir_name` may
/// carry a slash to nest the crate under a container (`promptforge/lua`).
fn write_crate(root: &Path, dir_name: &str, package: &str, deps: &str) {
    let dir = root.join("crates").join(dir_name);
    std::fs::create_dir_all(&dir).expect("the crate directory creates");
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{package}\"\n{deps}"),
    )
    .expect("the manifest writes");
}

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
    let (crates, violations) = workspace_crates(&workspace_root());
    assert!(violations.is_empty(), "{violations:?}");
    let shell = crates
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
fn an_outside_crate_reaching_past_the_one_door_is_reported() {
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
fn an_outside_crate_depending_into_the_private_container_is_reported() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "workshop-sessions",
        "workshop-sessions",
        "[dependencies]\npromptforge-lua = { path = \"../promptforge/lua\" }\n",
    );
    write_crate(root.path(), "promptforge/lua", "promptforge-lua", "");
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("workshop-sessions depends on promptforge-lua")
            && violations[0].contains("crates/promptforge is private to its family")
            && violations[0].contains("promptforge-api-runtime"),
        "the violation carries the container privacy message: {violations:?}"
    );
}

#[test]
fn a_container_crate_depending_on_a_container_sibling_passes() {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(
        root.path(),
        "promptforge/parser",
        "promptforge-parser",
        "[dependencies]\npromptforge-lua = { path = \"../lua\" }\n",
    );
    write_crate(root.path(), "promptforge/lua", "promptforge-lua", "");
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
        "[dependencies]\npromptforge-lua = { path = \"../promptforge/lua\" }\n",
    );
    write_crate(root.path(), "promptforge/lua", "promptforge-lua", "");
    let violations = product_boundary_violations(root.path());
    assert!(
        violations.is_empty(),
        "the named public crate may depend into the container: {violations:?}"
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
    let violations = product_boundary_violations(root.path());
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("unparseable manifest"),
        "the violation reports the parse failure: {violations:?}"
    );
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
