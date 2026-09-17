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
//!   `promptforge-*` only through `promptforge-api-runtime` or
//!   `promptforge-api-types`.
//! - Container privacy: the manifestless `crates/promptforge/` directory is
//!   private to its family; only the crates inside it and the container's
//!   named public crate (`promptforge-api-runtime`) may depend on the
//!   crates it holds.
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
        let inside = container_of(&package.dir) == Some(container);
        let named = container_public_crate(container) == Some(package.package.as_str());
        if !inside && !named {
            return Some(format!(
                "crates/{container} is private to its family; only {} may depend into it",
                container_public_crate(container).unwrap_or("no outside crate")
            ));
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
        (Family::Workshop, Family::Gateway) => {
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

/// The name of the manifestless `crates/<container>/` directory holding the
/// crate whose root-relative manifest directory is `dir`, or `None` for a
/// crate directly under `crates/`.
fn container_of(dir: &Path) -> Option<&str> {
    let mut components = dir.components();
    if components.next()?.as_os_str() != "crates" {
        return None;
    }
    let container = components.next()?.as_os_str().to_str()?;
    if components.next().is_some() {
        Some(container)
    } else {
        None
    }
}

/// The one crate outside a container permitted to depend into it, when the
/// container names one.
fn container_public_crate(container: &str) -> Option<&'static str> {
    match container {
        "promptforge" => Some("promptforge-api-runtime"),
        _ => None,
    }
}

/// Every workspace crate's package name, manifest directory, and dependency
/// package names, plus violations for manifests that could not be read or
/// parsed. A directory under `crates/` containing a `Cargo.toml` is a crate
/// and is not descended into; any other directory is a container and the
/// walk descends one level.
fn workspace_crates(root: &Path) -> (Vec<CrateInfo>, Vec<String>) {
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
        let dir = entry.path();
        if dir.join("Cargo.toml").exists() {
            read_crate(root, &dir, &mut crates, &mut violations);
        } else if let Ok(inner) = fs::read_dir(&dir) {
            // A manifestless container holds its crates one level down.
            for entry in inner.flatten() {
                let sub = entry.path();
                if sub.is_dir() && sub.join("Cargo.toml").exists() {
                    read_crate(root, &sub, &mut crates, &mut violations);
                }
            }
        }
    }
    (crates, violations)
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
