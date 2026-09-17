//! Product-boundary check: codifies the AGENTS.md dependency matrix.
//!
//! Every workspace crate is classified by package name into a product
//! family, and its dependencies of every kind (normal, dev, build, and
//! target-specific) are checked against the matrix:
//!
//! - `promptforge-*` crates must not depend on gateway or workshop crates.
//! - `gateway`/`gateway-*` crates must not depend on promptforge or
//!   workshop crates.
//! - `workshop`/`workshop-*` crates must not depend on gateway crates,
//!   except the family's public pair (`gateway-api`,
//!   `gateway-api-discovery`).
//! - `shared-*` crates must not depend on any product crate.
//! - One door: a crate outside the promptforge family may depend on
//!   `promptforge-*` only through `promptforge-api-runtime` or
//!   `promptforge-api-types`.
//! - Container privacy: the manifestless `crates/promptforge/` and
//!   `crates/gateway/` directories are private to their families; only the
//!   crates inside a container and the container's named outside exception
//!   (`promptforge-api-runtime` for `crates/promptforge/`; the gateway
//!   containers name none) may depend on the crates it holds. Containers
//!   nest: `crates/gateway/stt/` is a subsystem private to the gateway
//!   family, with `gateway-stt` as its public member - the one crate inside
//!   the family outside the subsystem may name.
//! - Shell boundary: the `workshop` shell depends on `workshop-server-api`
//!   and never on `workshop-server`.

use std::fs;
use std::path::{Path, PathBuf};

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

/// A workspace crate: its package name, its manifest directory relative to
/// the workspace root, and its dependency package names.
struct CrateInfo {
    package: String,
    dir: PathBuf,
    deps: Vec<String>,
}

/// Check every workspace manifest against the product-boundary matrix.
#[must_use]
pub(crate) fn product_boundary_violations(root: &Path) -> Vec<String> {
    let (crates, mut violations) = workspace_crates(root);
    for package in &crates {
        for dep_name in &package.deps {
            // Only workspace members are bound by the matrix; a crates.io
            // package that happens to carry a product prefix is not.
            let Some(dep) = crates.iter().find(|krate| &krate.package == dep_name) else {
                continue;
            };
            if let Some(reason) = boundary_breach(package, dep) {
                violations.push(format!(
                    "{} depends on {}: {reason}",
                    package.package, dep.package
                ));
            }
        }
    }
    violations
}

/// The Tauri shell crate, bound by the shell-boundary rule.
const SHELL: &str = "workshop";
/// The server crate the shell must never name directly.
const SERVER: &str = "workshop-server";
/// The promptforge-family crates outside crates may depend on directly.
const PUBLIC_PROMPTFORGE: [&str; 2] = ["promptforge-api-runtime", "promptforge-api-types"];
/// The gateway family's public pair: the only gateway crates workshop
/// crates may name.
const PUBLIC_GATEWAY: [&str; 2] = ["gateway-api", "gateway-api-discovery"];

/// The reason a dependency from `package` to `dep` breaches the matrix,
/// or `None` when the edge is legal.
fn boundary_breach(package: &CrateInfo, dep: &CrateInfo) -> Option<String> {
    if package.package == SHELL && dep.package == SERVER {
        return Some(
            "the workshop shell depends on workshop-server-api, never on workshop-server"
                .to_owned(),
        );
    }
    if let Some(container) = container_of(&dep.dir) {
        // Inside is physical containment: a crate anywhere under
        // crates/<container>/ may name the container's crates, so nested
        // subsystems see their parent family's crates (gateway-stt depends
        // on gateway-config) while the family cannot name the subsystem's
        // private crates back.
        let inside = package
            .dir
            .starts_with(Path::new("crates").join(&container));
        let named = container_named_exception(&container) == Some(package.package.as_str());
        // A container's public member is visible one level higher: to the
        // crates under the container's parent scope only.
        let public = container_public_member(&container) == Some(dep.package.as_str())
            && match parent_scope(&container) {
                Some(parent) => package.dir.starts_with(Path::new("crates").join(parent)),
                None => package.dir.starts_with("crates"),
            };
        if !inside && !named && !public {
            return Some(match container_face(&container) {
                Some(face) => format!(
                    "crates/{container} is private to its family; only {face} may depend into it"
                ),
                None => format!(
                    "crates/{container} is private to its family; no outside crate may depend into it"
                ),
            });
        }
    }
    let (from, to) = (family(&package.package), family(&dep.package));
    let family_rule = match (from, to) {
        (Family::Promptforge, Family::Gateway | Family::Workshop) => {
            Some("promptforge crates must not depend on gateway or workshop crates")
        }
        (Family::Gateway, Family::Promptforge | Family::Workshop) => {
            Some("gateway crates must not depend on promptforge or workshop crates")
        }
        (Family::Workshop, Family::Gateway) if !PUBLIC_GATEWAY.contains(&dep.package.as_str()) => {
            Some("workshop crates must not depend on gateway crates")
        }
        (Family::Shared, Family::Promptforge | Family::Gateway | Family::Workshop) => {
            Some("shared crates must not depend on product crates")
        }
        _ => None,
    };
    family_rule.map(str::to_owned).or_else(|| {
        if from != Family::Promptforge
            && to == Family::Promptforge
            && !PUBLIC_PROMPTFORGE.contains(&dep.package.as_str())
        {
            Some(
                "outside crates may depend on promptforge-* only through promptforge-api-runtime and promptforge-api-types"
                    .to_owned(),
            )
        } else {
            None
        }
    })
}

/// The root-relative path of the deepest manifestless `crates/<container>/`
/// directory holding the crate whose root-relative manifest directory is
/// `dir` (`crates/gateway/stt/engine` is in `gateway/stt`), or `None` for a
/// crate directly under `crates/`.
fn container_of(dir: &Path) -> Option<String> {
    let mut components = dir.components();
    if components.next()?.as_os_str() != "crates" {
        return None;
    }
    let rest: Vec<&str> = components.filter_map(|c| c.as_os_str().to_str()).collect();
    // The last component is the crate's own directory; everything between
    // `crates/` and it is the container path.
    match rest.len() {
        0 | 1 => None,
        _ => Some(rest[..rest.len() - 1].join("/")),
    }
}

/// The scope one level above a container: the container path with its last
/// component dropped, or `None` when the container sits directly under
/// `crates/`.
fn parent_scope(container: &str) -> Option<&str> {
    container.rsplit_once('/').map(|(parent, _)| parent)
}

/// The one crate outside a container permitted to depend into it, when the
/// container names one.
fn container_named_exception(container: &str) -> Option<&'static str> {
    match container {
        "promptforge" => Some("promptforge-api-runtime"),
        _ => None,
    }
}

/// The container's public member: the one crate inside the container that
/// crates under the parent scope may name.
fn container_public_member(container: &str) -> Option<&'static str> {
    match container {
        "gateway/stt" => Some("gateway-stt"),
        _ => None,
    }
}

/// The crate a container-privacy violation names as the legal way in: the
/// named outside exception, or the public member when there is none.
fn container_face(container: &str) -> Option<&'static str> {
    container_named_exception(container).or_else(|| container_public_member(container))
}

/// Every workspace crate's package name, manifest directory, and dependency
/// package names, plus violations for manifests that could not be read or
/// parsed. A directory under `crates/` containing a `Cargo.toml` is a crate
/// and is not descended into; any other directory is a container and the
/// walk descends, so containers may nest (`crates/gateway/stt/`).
fn workspace_crates(root: &Path) -> (Vec<CrateInfo>, Vec<String>) {
    let mut crates = Vec::new();
    let mut violations = Vec::new();
    let crates_dir = root.join("crates");
    walk_crates(root, &crates_dir, &mut crates, &mut violations);
    (crates, violations)
}

/// Walk one directory level: crates are read, manifestless containers are
/// descended into.
fn walk_crates(root: &Path, dir: &Path, crates: &mut Vec<CrateInfo>, violations: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            violations.push(format!(
                "{}: unreadable crates directory: {error}",
                dir.display()
            ));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                violations.push(format!(
                    "{}: unreadable directory entry: {error}",
                    dir.display()
                ));
                continue;
            }
        };
        if !entry.path().is_dir() {
            continue;
        }
        let sub = entry.path();
        if sub.join("Cargo.toml").exists() {
            read_crate(root, &sub, crates, violations);
        } else {
            walk_crates(root, &sub, crates, violations);
        }
    }
}

/// Read one crate's manifest into `crates`; failures land in `violations`.
fn read_crate(root: &Path, dir: &Path, crates: &mut Vec<CrateInfo>, violations: &mut Vec<String>) {
    let manifest_path = dir.join("Cargo.toml");
    let text = match fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => {
            violations.push(format!(
                "{}: unreadable manifest: {error}",
                manifest_path.display()
            ));
            return;
        }
    };
    let manifest = match toml::from_str::<toml::Value>(&text) {
        Ok(manifest) => manifest,
        Err(error) => {
            violations.push(format!(
                "{}: unparseable manifest: {error}",
                manifest_path.display()
            ));
            return;
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
        return;
    };
    crates.push(CrateInfo {
        package: package.to_owned(),
        dir: dir.strip_prefix(root).unwrap_or(dir).to_path_buf(),
        deps: manifest_dependencies(&manifest),
    });
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
#[path = "product-tests.rs"]
mod tests;
