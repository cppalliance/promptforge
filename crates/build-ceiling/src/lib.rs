//! The compile-time file-size check: every workspace crate's build script
//! calls [`check`], which fails the build when a Rust file in the crate is
//! over 500 lines.
//!
//! This file compiles on std alone, because the crates that may not name a
//! `build-*` crate include it from their build scripts with `#[path]`.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The most physical lines a Rust file may have.
const LIMIT: usize = 500;

/// The directories under the crate root whose `.rs` files are counted, at
/// any depth, beside the root's own `build.rs`.
const TREES: [&str; 4] = ["src", "tests", "benches", "examples"];

/// The split rule, one `help:` line each.
const HELP: [&str; 6] = [
    "split the file only into private child modules of that file",
    "declare each child inside the file as `#[path = \"<stem>-<topic>.rs\"] mod <topic>;`, or \
     lay it out as `<stem>/<topic>.rs`, choosing between the two forms by the flat-directory \
     rule in AGENTS.md",
    "open each child module with a `//!` line naming its one concern",
    "move an inline `#[cfg(test)]` module to `<stem>-tests.rs` first, when one exists",
    "reach the parent's private items through `super::`, and give a moved item `pub(super)` \
     only where the parent uses it",
    "never widen anything to `pub(crate)` or `pub` to make a split compile",
];

/// The sentence the failure message ends with.
const STOP: &str = "If no cohesive group splits out without widening visibility, stop and say why.";

/// Counts every physical line of every `.rs` file in the calling crate,
/// found from `CARGO_MANIFEST_DIR`, and asks Cargo to rerun the build
/// script when any of those files changes or a file is added beside them.
///
/// A tree that does not exist yet is not watched, so the first file in a
/// new `tests/`, `benches/`, or `examples/` directory is counted only at the
/// next rerun: an edit to a watched path, or a build from a fresh checkout,
/// which is every CI build.
///
/// # Errors
///
/// Returns every file over 500 lines and every file or directory the scan
/// could not read.
pub fn check() -> Result<(), Violations> {
    let Some(root) = std::env::var_os("CARGO_MANIFEST_DIR") else {
        return Err(Violations {
            findings: vec![Finding::Unreadable {
                path: PathBuf::from("CARGO_MANIFEST_DIR"),
                error: "the variable is not set; run the build script through cargo".to_owned(),
            }],
        });
    };
    let Scan { watched, findings } = scan(Path::new(&root));
    for path in watched {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    if findings.is_empty() {
        Ok(())
    } else {
        Err(Violations { findings })
    }
}

/// Every file over the limit and every file the scan could not read. Its
/// `Display` and `Debug` render the same failure message, because a `main`
/// that returns a `Result` prints its error through `Debug`.
pub struct Violations {
    findings: Vec<Finding>,
}

/// One path the check failed.
enum Finding {
    /// A file over the limit, with its physical line count.
    Oversized { path: PathBuf, lines: usize },
    /// A file or directory the scan could not read, so it could not be
    /// shown to be within the limit.
    Unreadable { path: PathBuf, error: String },
}

/// What one scan of a crate root found.
struct Scan {
    /// The trees and the build script that exist, for `rerun-if-changed`.
    /// A path that does not exist is left out, because Cargo reruns a
    /// build script on every build while a watched path is missing.
    watched: Vec<PathBuf>,
    findings: Vec<Finding>,
}

/// Scans the crate at `root`: every `.rs` file under [`TREES`] and the
/// root's `build.rs`, in path order.
fn scan(root: &Path) -> Scan {
    let mut watched = Vec::new();
    let mut files = Vec::new();
    let mut findings = Vec::new();
    for tree in TREES {
        let dir = root.join(tree);
        if dir.exists() {
            collect(&dir, &mut files, &mut findings);
            watched.push(dir);
        }
    }
    let build = root.join("build.rs");
    if build.exists() {
        files.push(build.clone());
        watched.push(build);
    }
    files.sort();
    for path in files {
        match fs::read_to_string(&path) {
            Ok(text) => {
                let lines = text.lines().count();
                if lines > LIMIT {
                    findings.push(Finding::Oversized { path, lines });
                }
            }
            Err(error) => findings.push(unreadable(path, &error)),
        }
    }
    Scan { watched, findings }
}

/// Adds every `.rs` file under `dir`, at any depth, to `files`.
fn collect(dir: &Path, files: &mut Vec<PathBuf>, findings: &mut Vec<Finding>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            findings.push(unreadable(dir.to_path_buf(), &error));
            return;
        }
    };
    for entry in entries {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, files, findings);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    files.push(path);
                }
            }
            Err(error) => findings.push(unreadable(dir.to_path_buf(), &error)),
        }
    }
}

fn unreadable(path: PathBuf, error: &io::Error) -> Finding {
    Finding::Unreadable {
        path,
        error: error.to_string(),
    }
}

impl fmt::Display for Violations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut oversized = false;
        for (index, finding) in self.findings.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            match finding {
                Finding::Oversized { path, lines } => {
                    oversized = true;
                    write!(
                        f,
                        "{} has {lines} lines, over the limit of {LIMIT}",
                        path.display()
                    )?;
                }
                Finding::Unreadable { path, error } => write!(
                    f,
                    "{} could not be read, so its lines could not be counted against the \
                     limit of {LIMIT}: {error}",
                    path.display()
                )?,
            }
        }
        if oversized {
            for help in HELP {
                write!(f, "\nhelp: {help}")?;
            }
            write!(f, "\n{STOP}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Violations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for Violations {}

#[cfg(test)]
#[path = "lib-tests.rs"]
mod tests;
