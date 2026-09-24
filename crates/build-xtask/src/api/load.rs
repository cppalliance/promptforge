//! Builds rustdoc JSON for the facade and every crate under
//! `crates/promptforge-internal/` with default features, and parses it.
//!
//! One `cargo doc` invocation selects the facade and every internal
//! crate, so each internal crate is documented with the features the
//! facade activates. Selecting an internal crate as a root also turns on
//! its `default` feature, which the facade's own graph turns on too,
//! since no internal crate declares one. The JSON lands under
//! `target/xtask-api/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use rustdoc_types::{Crate, FORMAT_VERSION};

use super::toolchain::PINNED;

/// The facade's package and crate name.
pub(crate) const FACADE: &str = "promptforge";

/// The rustdoc flags every build passes. Hidden items are documented
/// because hosts can still name them, and the transport codec is still
/// hidden in its defining crate. Lints are capped at warn: documenting
/// hidden items lints docs the workspace docs gate never renders, and
/// link integrity is that gate's to deny - an unresolved link never
/// reaches the JSON `links` table this command reads.
const RUSTDOC_FLAGS: [&str; 6] = [
    "-Zunstable-options",
    "--output-format",
    "json",
    "--document-hidden-items",
    "--cap-lints",
    "warn",
];

/// The parsed rustdoc JSON of the facade and its internal crates.
#[derive(Debug)]
pub(crate) struct Loaded {
    /// The facade crate.
    pub(crate) facade: Crate,
    /// Every internal crate, keyed by its crate name (`promptforge_engine`).
    pub(crate) internal: BTreeMap<String, Crate>,
}

/// The package names of every crate under the internal container, sorted.
/// A manifest that cannot be read or names no package is an error: a
/// crate the check cannot name cannot be documented or matched.
pub(crate) fn internal_packages(root: &Path) -> Result<Vec<String>, String> {
    let container = root
        .join("crates")
        .join(crate::engine_guards::ENGINE_CONTAINER);
    let mut dirs = Vec::new();
    crate::engine_guards::collect_crates(&container, &mut dirs);
    let mut packages = Vec::new();
    for dir in dirs {
        let manifest = dir.join("Cargo.toml");
        let name = std::fs::read_to_string(&manifest)
            .ok()
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
            .and_then(|value| {
                value
                    .get("package")
                    .and_then(|package| package.get("name"))
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            })
            .ok_or_else(|| format!("{}: no readable package name", manifest.display()))?;
        packages.push(name);
    }
    if packages.is_empty() {
        return Err(format!("{}: no internal crates found", container.display()));
    }
    packages.sort();
    Ok(packages)
}

/// Builds and parses the rustdoc JSON for the workspace at `root`.
pub(crate) fn load(root: &Path) -> Result<Loaded, String> {
    let packages = internal_packages(root)?;
    let target = root.join("target").join("xtask-api");
    run_cargo_doc(root, &target, &packages)?;
    let doc = target.join("doc");
    let facade = read_crate(&doc, FACADE)?;
    let mut internal = BTreeMap::new();
    for package in &packages {
        let name = package.replace('-', "_");
        let krate = read_crate(&doc, &name)?;
        internal.insert(name, krate);
    }
    Ok(Loaded { facade, internal })
}

/// A cargo command in `root` on the pinned nightly: the cargo that runs
/// this process when there is one, with rustup's proxies told the pinned
/// toolchain so the rustc and rustdoc it spawns match.
pub(crate) fn cargo(root: &Path) -> Command {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .env("RUSTUP_TOOLCHAIN", PINNED.nightly);
    command
}

fn run_cargo_doc(root: &Path, target: &Path, packages: &[String]) -> Result<(), String> {
    let mut command = cargo(root);
    command
        .env("CARGO_ENCODED_RUSTDOCFLAGS", RUSTDOC_FLAGS.join("\u{1f}"))
        .env_remove("RUSTDOCFLAGS")
        .args(["doc", "--locked", "--no-deps", "--lib", "--target-dir"])
        .arg(target)
        .args(["-p", FACADE]);
    for package in packages {
        command.args(["-p", package]);
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = stderr.lines().rev().take(40).collect();
    Err(format!(
        "`cargo doc` for rustdoc JSON failed ({}):\n{}",
        output.status,
        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
    ))
}

/// Parses `<doc>/<name>.json`. A file in another `format_version` is
/// reported as that, naming the pinned nightly, rather than as whatever
/// field the parse tripped on first.
fn read_crate(doc: &Path, name: &str) -> Result<Crate, String> {
    let path: PathBuf = doc.join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{}: unreadable rustdoc JSON: {error}", path.display()))?;
    let wrong_version = |found: u64| {
        format!(
            "{}: required rustdoc JSON format_version {FORMAT_VERSION} (from `{}`, read by \
             rustdoc-types {}), found {found}",
            path.display(),
            PINNED.nightly,
            PINNED.rustdoc_types
        )
    };
    let krate = serde_json::from_str::<Crate>(&text).map_err(|error| {
        let version = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("format_version")
                    .and_then(serde_json::Value::as_u64)
            });
        match version {
            Some(found) if found != u64::from(FORMAT_VERSION) => wrong_version(found),
            _ => format!("{}: unparseable rustdoc JSON: {error}", path.display()),
        }
    })?;
    if krate.format_version != FORMAT_VERSION {
        return Err(wrong_version(krate.format_version.into()));
    }
    Ok(krate)
}

#[cfg(test)]
#[path = "load-tests.rs"]
mod tests;
