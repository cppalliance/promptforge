//! Product-boundary check: codifies the AGENTS.md dependency matrix.
//!
//! Every workspace crate is classified by package name into a product
//! family, and its dependencies of every kind (normal, dev, build, and
//! target-specific) are checked against the matrix:
//!
//! - `promptforge`/`promptforge-*` crates must not depend on gateway,
//!   workshop, or harness crates.
//! - `gateway`/`gateway-*` crates must not depend on promptforge,
//!   workshop, or harness crates.
//! - `workshop`/`workshop-*` crates must not depend on gateway crates,
//!   except the family's public pair (`gateway-api-types`,
//!   `gateway-api-discovery`), and may depend on `harness-*` only through
//!   `harness-api`.
//! - `harness-*` crates must not depend on workshop crates, and may depend
//!   on gateway crates only through the public pair.
//! - `shared-*` crates must not depend on any product crate.
//! - Public API: a crate outside the promptforge family may depend on
//!   the family only through `promptforge`, `promptforge-api-runtime`, or
//!   `promptforge-api-types`.
//! - Container privacy: the manifestless `crates/promptforge-internal/`,
//!   `crates/gateway/`, `crates/workshop/`, and `crates/harness/`
//!   directories are private to their families; only the crates inside a
//!   container and the container's named outside exception (`promptforge`
//!   for `crates/promptforge-internal/`, `harness-api` for
//!   `crates/harness/`; the gateway and workshop containers name none) may
//!   depend on the crates it holds. `TRANSITIONAL_CONTAINER_EXCEPTIONS`
//!   admits `promptforge-api-runtime` into `crates/promptforge-internal/`
//!   as well, until the runtime moves inside it. Containers nest:
//!   `crates/gateway/stt/` is a subsystem private to the gateway family,
//!   with `gateway-stt` as its public member - the one crate inside the
//!   family outside the subsystem may name.
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
    Harness,
    Shared,
    Build,
    /// Named after no product family; has no matrix rules of its own.
    Unaffiliated,
}

/// Classifies a package name into its product family.
fn family(package: &str) -> Family {
    if package == "promptforge" || package.starts_with("promptforge-") {
        Family::Promptforge
    } else if package == "gateway" || package.starts_with("gateway-") {
        Family::Gateway
    } else if package == "workshop" || package.starts_with("workshop-") {
        Family::Workshop
    } else if package.starts_with("harness-") {
        Family::Harness
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
pub(crate) struct CrateInfo {
    pub(crate) package: String,
    pub(crate) dir: PathBuf,
    deps: Vec<String>,
}

/// Every crate under `crates/`, as the one enumeration the architecture
/// checks share.
pub(crate) struct CrateWalk {
    /// The crates whose manifests named a package.
    pub(crate) crates: Vec<CrateInfo>,
    /// Crate directories, relative to the workspace root, whose manifest
    /// could not be read, parsed, or named. A crate that cannot be read
    /// cannot be shown exempt, so the checks that bind by package name
    /// bind these too.
    pub(crate) unread: Vec<PathBuf>,
    /// Every read failure, in walk order.
    pub(crate) violations: Vec<String>,
}

/// Checks every workspace manifest against the product-boundary matrix.
///
/// The walk's read failures belong to `tidy::marker_violations`, the one
/// owner: both checks share the walk, so reporting them here too would
/// name every unreadable manifest twice in `tidy::all_violations`.
#[must_use]
pub(crate) fn product_boundary_violations(root: &Path) -> Vec<String> {
    let CrateWalk { crates, .. } = workspace_crates(root);
    let mut violations = Vec::new();
    for package in &crates {
        for dep_name in &package.deps {
            // Only workspace members are bound by the matrix; a crates.io
            // package that happens to start with a product prefix is not.
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
const PUBLIC_PROMPTFORGE: [&str; 3] = [
    "promptforge",
    "promptforge-api-runtime",
    "promptforge-api-types",
];
/// `(container, crate)` pairs admitted into a container beside its named
/// exception. Temporary: the runtime depends on the engine container's
/// crates from outside it until it moves in, which leaves exactly one
/// named outside crate per container again.
const TRANSITIONAL_CONTAINER_EXCEPTIONS: [(&str, &str); 1] =
    [("promptforge-internal", "promptforge-api-runtime")];
/// The gateway family's public pair: the only gateway crates workshop
/// crates may name.
const PUBLIC_GATEWAY: [&str; 2] = ["gateway-api-types", "gateway-api-discovery"];
/// The harness family's public crate: the only harness crate workshop
/// crates may name, and the one outside crate permitted into
/// `crates/harness/`.
const PUBLIC_HARNESS: &str = "harness-api";

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
        let transitional = TRANSITIONAL_CONTAINER_EXCEPTIONS
            .contains(&(container.as_str(), package.package.as_str()));
        // The build-* crates are meta tooling, not a product family: they
        // may depend into any container.
        let meta = family(&package.package) == Family::Build;
        // A container's public member is visible one level higher: to the
        // crates under the container's parent scope only.
        let public = container_public_member(&container) == Some(dep.package.as_str())
            && match parent_scope(&container) {
                Some(parent) => package.dir.starts_with(Path::new("crates").join(parent)),
                None => package.dir.starts_with("crates"),
            };
        if !inside && !named && !transitional && !meta && !public {
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
    let public_gateway = PUBLIC_GATEWAY.contains(&dep.package.as_str());
    let family_rule = match (from, to) {
        (Family::Promptforge, Family::Gateway | Family::Workshop | Family::Harness) => {
            Some("promptforge crates must not depend on gateway, workshop, or harness crates")
        }
        (Family::Gateway, Family::Promptforge | Family::Workshop | Family::Harness) => {
            Some("gateway crates must not depend on promptforge, workshop, or harness crates")
        }
        (Family::Workshop, Family::Gateway) if !public_gateway => {
            Some("workshop crates must not depend on gateway crates")
        }
        (Family::Workshop, Family::Harness) if dep.package != PUBLIC_HARNESS => {
            Some("workshop crates may depend on harness-* only through harness-api")
        }
        (Family::Harness, Family::Workshop) => {
            Some("harness crates must not depend on workshop crates")
        }
        (Family::Harness, Family::Gateway) if !public_gateway => Some(
            "harness crates may depend on gateway-* only through gateway-api-types and gateway-api-discovery",
        ),
        (
            Family::Shared,
            Family::Promptforge | Family::Gateway | Family::Workshop | Family::Harness,
        ) => Some("shared crates must not depend on product crates"),
        _ => None,
    };
    family_rule.map(str::to_owned).or_else(|| {
        if from != Family::Promptforge
            && to == Family::Promptforge
            && !PUBLIC_PROMPTFORGE.contains(&dep.package.as_str())
        {
            Some(
                "outside crates may depend on the promptforge family only through promptforge, promptforge-api-runtime, and promptforge-api-types"
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
        "promptforge-internal" => Some("promptforge"),
        "harness" => Some(PUBLIC_HARNESS),
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
/// package names, plus the directories and violations for manifests that
/// could not be read or parsed. A directory under `crates/` containing a
/// `Cargo.toml` is a crate and is not descended into; any other directory
/// is a container and the walk descends, so containers may nest
/// (`crates/gateway/stt/`).
pub(crate) fn workspace_crates(root: &Path) -> CrateWalk {
    let mut walk = CrateWalk {
        crates: Vec::new(),
        unread: Vec::new(),
        violations: Vec::new(),
    };
    let crates_dir = root.join("crates");
    walk_crates(root, &crates_dir, &mut walk);
    walk
}

/// Walks one directory level: crates are read, manifestless containers are
/// descended into.
fn walk_crates(root: &Path, dir: &Path, walk: &mut CrateWalk) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            walk.violations.push(format!(
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
                walk.violations.push(format!(
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
            read_crate(root, &sub, walk);
        } else {
            walk_crates(root, &sub, walk);
        }
    }
}

/// Reads one crate's manifest into the walk; a failure names the crate in
/// `unread` and the failure mode in `violations`.
fn read_crate(root: &Path, dir: &Path, walk: &mut CrateWalk) {
    let relative = dir.strip_prefix(root).unwrap_or(dir).to_path_buf();
    let manifest_path = dir.join("Cargo.toml");
    let text = match fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => {
            walk.unread.push(relative);
            walk.violations.push(format!(
                "{}: unreadable manifest: {error}",
                manifest_path.display()
            ));
            return;
        }
    };
    let manifest = match toml::from_str::<toml::Value>(&text) {
        Ok(manifest) => manifest,
        Err(error) => {
            walk.unread.push(relative);
            walk.violations.push(format!(
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
        walk.unread.push(relative);
        walk.violations.push(format!(
            "{}: manifest has no package name",
            manifest_path.display()
        ));
        return;
    };
    walk.crates.push(CrateInfo {
        package: package.to_owned(),
        dir: relative,
        deps: manifest_dependencies(&manifest),
    });
}

/// Every dependency package name declared in a manifest, across normal,
/// dev, build, and target-specific tables, resolving `package` renames.
fn manifest_dependencies(manifest: &toml::Value) -> Vec<String> {
    let mut names = Vec::new();
    for (_, table) in crate::manifest::dependency_tables(manifest, &DEP_KINDS) {
        collect_deps(table, &mut names);
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
#[path = "product-container-tests.rs"]
mod container_tests;
#[cfg(test)]
#[path = "product-test-support.rs"]
pub(crate) mod test_support;
#[cfg(test)]
#[path = "product-tests.rs"]
mod tests;
