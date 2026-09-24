//! `cargo xtask api`: proves the `promptforge` facade's surface closed and
//! snapshots it, from rustdoc JSON built on the pinned nightly.
//!
//! The facade and every internal crate are documented twice, once with
//! the facade's default features and once with `test-support` on. Both
//! builds are checked: each facade `use` must resolve to a single item
//! defined in an internal crate, every path a surface item's signature,
//! fields, bounds, impls, or doc links name must be a facade re-export,
//! std, core, alloc, or an allowlisted crate, and no surface doc text may
//! name an internal crate. The default build's listing is compared with
//! `crates/promptforge/public-api.txt`.
//!
//! - `cargo +<pinned nightly> xtask api` prints every violation and the
//!   listing's difference.
//! - `--check` fails on any violation or any difference.
//! - `--bless` rewrites the listing, and refuses while any violation
//!   remains.

mod closure;
mod doc_text;
mod items;
mod listing;
mod load;
mod render;
mod toolchain;
mod walk;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::process::ExitCode;

use load::Build;

const USAGE: &str = "usage: cargo xtask api [--check | --bless]";

/// One violation: the item, what it mentions, and what was required
/// versus found.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Finding {
    /// The surface item, by its facade path or impl header.
    pub(crate) item: String,
    /// What it mentions, and where on the item.
    pub(crate) mention: String,
    pub(crate) required: String,
    pub(crate) found: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: mentions {}: required {}, found {}",
            self.item, self.mention, self.required, self.found
        )
    }
}

/// What the command was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Report,
    Check,
    Bless,
}

/// The lines to print, and whether the command fails.
#[derive(Debug)]
pub(crate) struct Outcome {
    pub(crate) lines: Vec<String>,
    pub(crate) failed: bool,
}

/// Every build's findings, each with the builds it appears in, and the
/// default build's listing.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) findings: BTreeMap<Finding, BTreeSet<Build>>,
    pub(crate) listing: Vec<String>,
}

/// Runs `cargo xtask api` with the arguments after `api`.
pub(crate) fn run(root: &Path, args: &[String]) -> ExitCode {
    match outcome(root, args, toolchain::active().as_deref()) {
        Ok(outcome) => {
            for line in &outcome.lines {
                println!("{line}");
            }
            if outcome.failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Parses the mode and refuses any toolchain but the pinned nightly
/// before anything is built.
fn outcome(root: &Path, args: &[String], active: Option<&str>) -> Result<Outcome, String> {
    let mode = match args {
        [] => Mode::Report,
        [flag] if flag == "--check" => Mode::Check,
        [flag] if flag == "--bless" => Mode::Bless,
        _ => return Err(USAGE.to_owned()),
    };
    toolchain::require_pinned(active)?;
    execute(root, mode)
}

/// Builds, checks, and compares the workspace at `root` in `mode`.
pub(crate) fn execute(root: &Path, mode: Mode) -> Result<Outcome, String> {
    let report = report(root, &Build::ALL)?;
    let path = listing::path(root);
    let committed = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{}: unreadable listing: {error}", path.display())),
    };
    let difference = listing::difference(committed.as_deref(), &report.listing);
    let violations = report.findings.len();
    let mut lines: Vec<String> = report
        .findings
        .iter()
        .map(|(finding, builds)| {
            let builds: Vec<String> = builds.iter().map(ToString::to_string).collect();
            format!("[{}] {finding}", builds.join(", "))
        })
        .collect();
    if mode == Mode::Bless {
        if violations > 0 {
            lines.push(format!(
                "api: bless refused: {violations} violations remain"
            ));
            return Ok(Outcome {
                lines,
                failed: true,
            });
        }
        std::fs::write(&path, listing::text(&report.listing))
            .map_err(|error| format!("{}: cannot write the listing: {error}", path.display()))?;
        lines.push(format!(
            "api: blessed {} ({} lines)",
            path.display(),
            report.listing.len()
        ));
        return Ok(Outcome {
            lines,
            failed: false,
        });
    }
    let differs = !difference.is_empty();
    lines.extend(difference);
    lines.push(format!(
        "api: {violations} violations; the listing {} public-api.txt",
        if differs { "differs from" } else { "matches" }
    ));
    let failed = mode == Mode::Check && (violations > 0 || differs);
    Ok(Outcome { lines, failed })
}

/// Loads each of `builds` and runs every check over it.
pub(crate) fn report(root: &Path, builds: &[Build]) -> Result<Report, String> {
    let mut report = Report::default();
    for &build in builds {
        let loaded = load::load(root, build)?;
        let (surface, mut findings) = items::Surface::resolve(&loaded);
        let visits = walk::visits(&loaded, &surface);
        findings.extend(closure::findings(&loaded, &surface, &visits));
        findings.extend(doc_text::findings(&loaded, &surface, &visits));
        if build == Build::Default {
            report.listing = listing::lines(&surface, &visits);
        }
        for finding in findings {
            report.findings.entry(finding).or_default().insert(build);
        }
    }
    Ok(report)
}

#[cfg(test)]
#[path = "api/fixture-test-support.rs"]
mod fixture;
