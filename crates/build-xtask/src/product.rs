//! Product-boundary check: codifies the AGENTS.md dependency matrix.
//!
//! Every workspace crate is classified by package name into a product
//! family, and its dependencies of every kind (normal, dev, build, and
//! target-specific) are checked against the matrix:
//!
//! - `promptforge-*` crates must not depend on gateway or workshop crates.
//! - `gateway`/`gateway-*` crates must not depend on promptforge or
//!   workshop crates.
//! - `workshop`/`workshop-*` crates must not depend on gateway crates.
//! - `shared-*` crates must not depend on any product crate.
//! - One door: a crate outside the promptforge family may depend on
//!   `promptforge-*` only through `promptforge-api`.
//! - Shell boundary: the `workshop` shell depends on `workshop-server-api`
//!   and never on `workshop-server`.

use std::fs;
use std::path::Path;

/// The dependency tables cargo recognizes, directly and under `[target]`.
const DEP_KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

/// The product family a package name belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Promptforge,
    Gateway,
    Workshop,
    Shared,
    Build,
    /// Named after no product family; carries no matrix rules of its own.
    Unaffiliated,
}

/// Classify a package name into its product family.
fn family(package: &str) -> Family {
    if package.starts_with("promptforge-") {
        Family::Promptforge
    } else if package == "gateway" || package.starts_with("gateway-") {
        Family::Gateway
    } else if package == "workshop" || package.starts_with("workshop-") {
        Family::Workshop
    } else if package.starts_with("shared-") {
        Family::Shared
    } else if package.starts_with("build-") {
        Family::Build
    } else {
        Family::Unaffiliated
    }
}

/// Check every workspace manifest against the product-boundary matrix.
#[must_use]
pub(crate) fn product_boundary_violations(root: &Path) -> Vec<String> {
    let (crates, mut violations) = workspace_crates(root);
    let members: Vec<&str> = crates.iter().map(|(package, _)| package.as_str()).collect();
    for (package, deps) in &crates {
        for dep in deps {
            // Only workspace members are bound by the matrix; a crates.io
            // package that happens to carry a product prefix is not.
            if !members.contains(&dep.as_str()) {
                continue;
            }
            if let Some(reason) = boundary_breach(package, dep) {
                violations.push(format!("{package} depends on {dep}: {reason}"));
            }
        }
    }
    violations
}

/// The Tauri shell crate, bound by the shell-boundary rule.
const SHELL: &str = "workshop";
/// The server crate the shell must never name directly.
const SERVER: &str = "workshop-server";

/// The reason a dependency from `package` to `dep` breaches the matrix,
/// or `None` when the edge is legal.
fn boundary_breach(package: &str, dep: &str) -> Option<&'static str> {
    if package == SHELL && dep == SERVER {
        return Some("the workshop shell depends on workshop-server-api, never on workshop-server");
    }
    let (from, to) = (family(package), family(dep));
    let family_rule = match (from, to) {
        (Family::Promptforge, Family::Gateway | Family::Workshop) => {
            Some("promptforge crates must not depend on gateway or workshop crates")
        }
        (Family::Gateway, Family::Promptforge | Family::Workshop) => {
            Some("gateway crates must not depend on promptforge or workshop crates")
        }
        (Family::Workshop, Family::Gateway) => {
            Some("workshop crates must not depend on gateway crates")
        }
        (Family::Shared, Family::Promptforge | Family::Gateway | Family::Workshop) => {
            Some("shared crates must not depend on product crates")
        }
        _ => None,
    };
    family_rule.or_else(|| {
        if from != Family::Promptforge && to == Family::Promptforge && dep != "promptforge-api" {
            Some("outside crates may depend on promptforge-* only through promptforge-api")
        } else {
            None
        }
    })
}

/// Every workspace crate's package name and dependency package names,
/// plus violations for manifests that could not be read or parsed.
fn workspace_crates(root: &Path) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let mut crates = Vec::new();
    let mut violations = Vec::new();
    let crates_dir = root.join("crates");
    let entries = match fs::read_dir(&crates_dir) {
        Ok(entries) => entries,
        Err(error) => {
            violations.push(format!(
                "{}: unreadable crates directory: {error}",
                crates_dir.display()
            ));
            return (crates, violations);
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                violations.push(format!(
                    "{}: unreadable directory entry: {error}",
                    crates_dir.display()
                ));
                continue;
            }
        };
        if !entry.path().is_dir() {
            continue;
        }
        let manifest_path = entry.path().join("Cargo.toml");
        if !manifest_path.exists() {
            // Not a crate: a manifestless directory declares no
            // dependencies and cannot breach the boundary.
            continue;
        }
        let text = match fs::read_to_string(&manifest_path) {
            Ok(text) => text,
            Err(error) => {
                violations.push(format!(
                    "{}: unreadable manifest: {error}",
                    manifest_path.display()
                ));
                continue;
            }
        };
        let manifest = match toml::from_str::<toml::Value>(&text) {
            Ok(manifest) => manifest,
            Err(error) => {
                violations.push(format!(
                    "{}: unparseable manifest: {error}",
                    manifest_path.display()
                ));
                continue;
            }
        };
        let Some(package) = manifest
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
        else {
            violations.push(format!(
                "{}: manifest has no package name",
                manifest_path.display()
            ));
            continue;
        };
        crates.push((package.to_owned(), manifest_dependencies(&manifest)));
    }
    (crates, violations)
}

/// Every dependency package name declared in a manifest, across normal,
/// dev, build, and target-specific tables, resolving `package` renames.
fn manifest_dependencies(manifest: &toml::Value) -> Vec<String> {
    let mut names = Vec::new();
    for kind in DEP_KINDS {
        if let Some(table) = manifest.get(kind).and_then(toml::Value::as_table) {
            collect_deps(table, &mut names);
        }
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for kind in DEP_KINDS {
                if let Some(table) = target.get(kind).and_then(toml::Value::as_table) {
                    collect_deps(table, &mut names);
                }
            }
        }
    }
    names
}

fn collect_deps(table: &toml::map::Map<String, toml::Value>, names: &mut Vec<String>) {
    for (key, value) in table {
        let package = value
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(key);
        if !names.iter().any(|name| name == package) {
            names.push(package.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("build-xtask lives at <root>/crates/build-xtask")
            .to_path_buf()
    }

    /// Write a minimal crate manifest into a fake workspace.
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
        let (_, deps) = crates
            .iter()
            .find(|(package, _)| package == "workshop")
            .expect("the workshop shell crate is a workspace member");
        assert!(
            deps.iter().any(|dep| dep == "workshop-server-api"),
            "the shell reaches the server through the api crate: {deps:?}"
        );
        assert!(
            !deps.iter().any(|dep| dep == "workshop-server"),
            "the shell never depends on workshop-server directly: {deps:?}"
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
            "[dependencies]\npromptforge-lua = { path = \"../promptforge-lua\" }\npromptforge-api = { path = \"../promptforge-api\" }\n",
        );
        write_crate(root.path(), "promptforge-lua", "promptforge-lua", "");
        write_crate(root.path(), "promptforge-api", "promptforge-api", "");
        let violations = product_boundary_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains("workshop-sessions")
                && violations[0].contains("promptforge-lua"),
            "the violation names the crate and the forbidden dep: {violations:?}"
        );
    }

    #[test]
    fn a_gateway_crate_depending_on_promptforge_api_is_reported() {
        let root = tempfile::TempDir::new().expect("tempdir");
        write_crate(
            root.path(),
            "gateway-routing",
            "gateway-routing",
            "[dependencies]\npromptforge-api = { path = \"../promptforge-api\" }\n",
        );
        write_crate(root.path(), "promptforge-api", "promptforge-api", "");
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
            "promptforge-api",
            "promptforge-api",
            "[dependencies]\ngateway-protocol = { path = \"../gateway-protocol\" }\nworkshop-protocol = { path = \"../workshop-protocol\" }\n",
        );
        write_crate(root.path(), "gateway-protocol", "gateway-protocol", "");
        write_crate(root.path(), "workshop-protocol", "workshop-protocol", "");
        let violations = product_boundary_violations(root.path());
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(
            violations.iter().all(|v| v.contains("promptforge-api")),
            "the violations name the promptforge crate: {violations:?}"
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
        assert_eq!(family("promptforge-api"), Family::Promptforge);
        assert_eq!(family("gateway"), Family::Gateway);
        assert_eq!(family("gateway-config"), Family::Gateway);
        assert_eq!(family("workshop"), Family::Workshop);
        assert_eq!(family("workshop-server"), Family::Workshop);
        assert_eq!(family("shared-vfs"), Family::Shared);
        assert_eq!(family("build-xtask"), Family::Build);
        assert_eq!(family("serde"), Family::Unaffiliated);
    }
}
