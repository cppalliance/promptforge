//! Plugin-family fixtures: a `plugin-*` crate depends only on
//! `runvil-plugin`, `shared-*` crates, and outside libraries; Engine,
//! Gateway, and Harness crates may not depend on a Plugin crate, except
//! `harness-gateway-client`.

use super::product_boundary_violations;
use super::test_support::write_crate;

/// The violations of a workspace holding `plugin-web` with `deps`, beside
/// a crate for each workspace dependency it may name.
fn plugin_web_naming(deps: &str) -> Vec<String> {
    let root = tempfile::TempDir::new().expect("tempdir");
    write_crate(root.path(), "plugin-web", "plugin-web", deps);
    for (dir, name) in [
        ("runvil-plugin", "runvil-plugin"),
        ("runvil", "runvil"),
        ("shared-loopback", "shared-loopback"),
        ("harness", "harness"),
        ("plugin-user-input", "plugin-user-input"),
        ("build-ui", "build-ui"),
    ] {
        write_crate(root.path(), dir, name, "");
    }
    product_boundary_violations(root.path())
}

const PLUGIN_RULE: &str =
    "plugin crates may depend only on runvil-plugin, shared-* crates, and outside libraries";

#[test]
fn a_plugin_crate_naming_the_contract_and_a_shared_crate_passes() {
    let violations = plugin_web_naming(
        "[dependencies]\nrunvil-plugin = { path = \"../runvil-plugin\" }\n\
         shared-loopback = { path = \"../shared-loopback\" }\n\
         [dev-dependencies]\nrunvil-plugin = { path = \"../runvil-plugin\", features = [\"test-support\"] }\n",
    );
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn a_plugin_crate_naming_any_other_workspace_crate_is_reported_in_every_table() {
    for (table, dep) in [
        ("dependencies", "runvil"),
        ("dependencies", "harness"),
        ("dev-dependencies", "plugin-user-input"),
        ("build-dependencies", "build-ui"),
    ] {
        let violations =
            plugin_web_naming(&format!("[{table}]\n{dep} = {{ path = \"../{dep}\" }}\n"));
        assert_eq!(violations.len(), 1, "{table} {dep}: {violations:?}");
        assert!(
            violations[0].starts_with(&format!("plugin-web depends on {dep}:"))
                && violations[0].contains(PLUGIN_RULE),
            "{table} {dep}: {violations:?}"
        );
    }
}

/// The violations of `package` in `dir` naming `plugin-web` under `table`.
fn naming_plugin_web(dir: &str, package: &str, table: &str) -> Vec<String> {
    let root = tempfile::TempDir::new().expect("tempdir");
    let depth = "../".repeat(dir.matches('/').count() + 1);
    write_crate(
        root.path(),
        dir,
        package,
        &format!("[{table}]\nplugin-web = {{ path = \"{depth}plugin-web\" }}\n"),
    );
    write_crate(root.path(), "plugin-web", "plugin-web", "");
    product_boundary_violations(root.path())
}

#[test]
fn engine_gateway_and_harness_crates_naming_a_plugin_crate_are_reported() {
    for (dir, package, table) in [
        ("runvil", "runvil", "dependencies"),
        ("gateway/app", "gateway", "dependencies"),
        ("harness", "harness", "dev-dependencies"),
        ("harness-internal/runner", "harness-runner", "dependencies"),
    ] {
        let violations = naming_plugin_web(dir, package, table);
        assert_eq!(violations.len(), 1, "{package}: {violations:?}");
        assert!(
            violations[0].starts_with(&format!("{package} depends on plugin-web:"))
                && violations[0].contains(
                    "runvil, gateway, and harness crates must not depend on plugin crates"
                ),
            "{package}: {violations:?}"
        );
    }
}

#[test]
fn the_harness_gateway_client_and_workshop_crates_may_name_a_plugin_crate() {
    for (dir, package) in [
        ("harness-gateway-client", "harness-gateway-client"),
        ("workshop/server", "workshop-server"),
    ] {
        let violations = naming_plugin_web(dir, package, "dependencies");
        assert!(violations.is_empty(), "{package}: {violations:?}");
    }
}
