//! Engine dependency guard: no Engine crate depends on an async runtime,
//! an HTTP client, or an OS entropy source, directly or transitively.
//!
//! The Engine (the `promptforge` and `promptforge-*` crates) is a
//! deterministic state machine: every wait becomes an effect the Harness
//! performs, and every pseudo-random value comes from a seed. Its crates,
//! tests included, therefore never depend on `tokio`, `tokio-util`,
//! `async-trait`, `reqwest`, `getrandom` (the OS entropy source), or
//! `fastrand` (whose global generator seeds itself). There is no
//! exemption.
//!
//! Two checks enforce it:
//!
//! - The manifest scan reads every dependency table an Engine manifest
//!   declares, `[dependencies]`, `[build-dependencies]`, and
//!   `[dev-dependencies]`, directly and under `[target.<cfg>]`, and
//!   resolves `package = ...` renames.
//! - The resolved-graph check runs `cargo tree` for each Engine crate with
//!   all features over its normal, build, and dev edges, so a forbidden
//!   crate is reported when it arrives through another crate or behind an
//!   optional dependency. Each crate is resolved on its own, because
//!   `cargo metadata` resolves features for the whole workspace at once,
//!   and a feature another workspace crate turns on must not flag an
//!   Engine crate.
//!
//! The calls that would break determinism without a dependency, a sleep, a
//! clock read, a timed wait, or an OS-seeded hasher, are clippy bans that
//! `engine_clippy` keeps in force.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The crates no Engine crate may depend on, directly or transitively.
const FORBIDDEN: [&str; 6] = [
    "tokio",
    "tokio-util",
    "async-trait",
    "reqwest",
    "getrandom",
    "fastrand",
];

/// The dependency tables the manifest scan reads, directly and under
/// `[target]`.
const CHECKED_KINDS: [&str; 3] = ["dependencies", "build-dependencies", "dev-dependencies"];

/// The `cargo tree` edge kinds the resolved-graph check follows.
const GRAPH_EDGES: &str = "normal,build,dev";

/// One finding from checking an Engine crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Violation {
    /// A forbidden crate declared in an Engine manifest.
    Forbidden {
        /// The manifest scanned.
        manifest: PathBuf,
        /// The table the entry sits in, as it would head the section in the
        /// manifest (`dependencies`, `target.'cfg(windows)'.build-dependencies`).
        table: String,
        /// The forbidden package name, after resolving `package = ...` renames.
        package: String,
    },
    /// A forbidden crate in an Engine crate's resolved graph.
    Reached {
        /// The Engine crate's package name.
        engine_crate: String,
        /// The forbidden package reached.
        package: String,
    },
    /// The manifest could not be read or parsed.
    Unreadable {
        /// The manifest scanned.
        manifest: PathBuf,
        /// What went wrong.
        error: String,
    },
    /// `cargo tree` could not resolve an Engine crate's graph, or the
    /// crate's manifest names no package to resolve.
    Unresolved {
        /// The Engine crate's package name, or its manifest path when the
        /// manifest names none.
        engine_crate: String,
        /// What went wrong.
        error: String,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rule = format!(
            "Engine crates never depend on {}, directly or transitively",
            FORBIDDEN.join(", ")
        );
        match self {
            Self::Forbidden {
                manifest,
                table,
                package,
            } => write!(
                f,
                "{}: [{table}] declares {package}; {rule}",
                manifest.display()
            ),
            Self::Reached {
                engine_crate,
                package,
            } => write!(
                f,
                "{engine_crate}: the resolved graph (all features; {GRAPH_EDGES} edges) reaches {package}; {rule}"
            ),
            Self::Unreadable { manifest, error } => {
                write!(f, "{}: {error}", manifest.display())
            }
            Self::Unresolved {
                engine_crate,
                error,
            } => write!(
                f,
                "{engine_crate}: the resolved graph is unchecked; required: a graph that resolves; actual: {error}"
            ),
        }
    }
}

/// Scans one Engine manifest for forbidden dependencies. A manifest that
/// cannot be read or parsed yields one [`Violation::Unreadable`].
#[must_use]
pub(crate) fn forbidden_engine_dependencies(manifest: &Path) -> Vec<Violation> {
    let value = match read_manifest(manifest) {
        Ok(value) => value,
        Err(error) => {
            return vec![Violation::Unreadable {
                manifest: manifest.to_path_buf(),
                error,
            }];
        }
    };
    let mut violations = Vec::new();
    for (table, entries) in crate::manifest::dependency_tables(&value, &CHECKED_KINDS) {
        for (key, entry) in entries {
            let package = entry
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(key);
            if FORBIDDEN.contains(&package) {
                violations.push(Violation::Forbidden {
                    manifest: manifest.to_path_buf(),
                    table: table.clone(),
                    package: package.to_owned(),
                });
            }
        }
    }
    violations
}

/// Resolves the graph of the Engine crate in `crate_dir` with `cargo tree`,
/// run in the workspace at `root` by the cargo running this process, and
/// reports each forbidden package it reaches once. A root without a
/// `Cargo.toml` has no workspace to resolve, a crate whose manifest cannot
/// be read or parsed is left to the manifest scan, which reports it, and a
/// manifest that names no package yields one [`Violation::Unresolved`].
#[must_use]
pub(crate) fn forbidden_engine_graph(root: &Path, crate_dir: &Path) -> Vec<Violation> {
    let workspace = root.join("Cargo.toml");
    if !workspace.is_file() {
        return Vec::new();
    }
    let manifest = crate_dir.join("Cargo.toml");
    let Ok(value) = read_manifest(&manifest) else {
        return Vec::new();
    };
    let Some(engine_crate) = package_name(&value) else {
        return vec![Violation::Unresolved {
            engine_crate: manifest.display().to_string(),
            error: "the manifest names no package, so cargo tree has none to resolve".to_owned(),
        }];
    };
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["tree", "--quiet", "--manifest-path"])
        .arg(&workspace)
        .args(["--package", &engine_crate, "--all-features", "--edges"])
        .args([GRAPH_EDGES, "--prefix", "none"])
        .output();
    let stdout = match output {
        Ok(output) if output.status.success() => output.stdout,
        Ok(output) => {
            return vec![Violation::Unresolved {
                engine_crate,
                error: format!(
                    "cargo tree failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            }];
        }
        Err(error) => {
            return vec![Violation::Unresolved {
                engine_crate,
                error: format!("cargo did not start: {error}"),
            }];
        }
    };
    let stdout = String::from_utf8_lossy(&stdout);
    let reached: BTreeSet<&str> = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|package| FORBIDDEN.contains(package))
        .collect();
    reached
        .into_iter()
        .map(|package| Violation::Reached {
            engine_crate: engine_crate.clone(),
            package: package.to_owned(),
        })
        .collect()
}

/// The package name a parsed manifest declares, if it names one.
fn package_name(manifest: &toml::Value) -> Option<String> {
    manifest
        .get("package")?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

/// Reads and parses one manifest, or says why it could not.
fn read_manifest(manifest: &Path) -> Result<toml::Value, String> {
    let text =
        fs::read_to_string(manifest).map_err(|error| format!("unreadable manifest: {error}"))?;
    toml::from_str(&text).map_err(|error| format!("unparseable manifest: {error}"))
}

#[cfg(test)]
#[path = "engine_deps-tests.rs"]
mod tests;
