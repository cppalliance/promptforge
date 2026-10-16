//! Engine clippy guard: every Engine crate lints under one of the three
//! Engine `clippy.toml` files, each of them holds every root entry and
//! every determinism ban, and no Engine source suppresses the bans.
//!
//! Clippy reads the nearest `.clippy.toml` or `clippy.toml` walking up from
//! a crate's manifest directory and never merges it with the root file. So
//! `crates/runvil-internal/clippy.toml` binds the six internal crates,
//! `crates/runvil/clippy.toml` and `crates/runvil-plugin/clippy.toml`
//! bind the two root crates, and each repeats the root file's settings and
//! `disallowed-methods` entries before banning the std calls that sleep,
//! read the clock, wait with a timeout, or seed a hasher from the OS. The
//! gate's clippy runs `--all-targets`, so the bans bind tests, benches, and
//! examples too.
//!
//! The guard fails in three cases:
//!
//! - An Engine crate resolves, from its manifest directory up to the
//!   workspace root, to another file or to none.
//! - One of the three files is unreadable, or lacks a root setting with
//!   its value, a root `disallowed-methods` entry with its reason, or a
//!   determinism ban with a reason.
//! - A `.rs` file under an Engine crate's `src`, `tests`, `benches`, or
//!   `examples` allows or expects `clippy::disallowed_methods`, or a group
//!   that holds it, at any depth, through `cfg_attr` included. Comments and
//!   doc text are not attributes, so prose naming the lint passes.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;

/// The Engine `clippy.toml` files, relative to the workspace root.
const ENGINE_CLIPPY_TOMLS: [&str; 3] = [
    "crates/runvil-internal/clippy.toml",
    "crates/runvil/clippy.toml",
    "crates/runvil-plugin/clippy.toml",
];

/// The file names clippy looks for in each directory, in its order.
const CONFIG_NAMES: [&str; 2] = [".clippy.toml", "clippy.toml"];

/// The key whose entries are compared one by one rather than whole.
const METHODS_KEY: &str = "disallowed-methods";

/// The determinism bans every Engine `clippy.toml` holds.
pub(crate) const DETERMINISM_BANS: [&str; 8] = [
    "std::thread::sleep",
    "std::time::SystemTime::now",
    "std::time::Instant::now",
    "std::collections::hash_map::RandomState::new",
    "std::sync::Condvar::wait_timeout",
    "std::sync::Condvar::wait_timeout_while",
    "std::sync::mpsc::Receiver::recv_timeout",
    "std::thread::park_timeout",
];

/// `clippy::disallowed_methods` and the clippy groups that hold it, any of
/// which an `allow` or `expect` would use to reopen the bans.
const SUPPRESSING_LINTS: [&str; 3] = ["disallowed_methods", "style", "all"];

/// The Engine crate directories whose `.rs` files the suppression scan
/// reads.
const SOURCE_DIRS: [&str; 4] = ["src", "tests", "benches", "examples"];

/// Runs the guard over every Engine crate: where each crate's clippy
/// config resolves, what the three Engine files hold, and every source
/// under the scanned directories.
#[must_use]
pub(crate) fn engine_clippy_violations(root: &Path) -> Vec<String> {
    let crates = crate::engine_guards::engine_crates(root);
    let mut violations = Vec::new();
    let root_file = root.join("clippy.toml");
    let root_config = match read_config(&root_file) {
        Ok(config) => Some(config),
        Err(error) => {
            violations.push(format!(
                "{}: required: a root clippy.toml whose entries the Engine files repeat; found: {error}",
                root_file.display()
            ));
            None
        }
    };
    let required: Vec<PathBuf> = ENGINE_CLIPPY_TOMLS
        .iter()
        .map(|path| at(root, path))
        .collect();
    for dir in &crates {
        violations.extend(resolution_violation(root, dir, &required));
    }
    for file in &required {
        match read_config(file) {
            Ok(config) => {
                if let Some(root_config) = &root_config {
                    violations.extend(root_entry_violations(file, root_config, &config));
                }
                violations.extend(ban_violations(file, &config));
            }
            Err(error) => violations.push(format!(
                "{}: required: an Engine clippy.toml holding every root entry and determinism ban; found: {error}",
                file.display()
            )),
        }
    }
    let mut files: Vec<PathBuf> = crates
        .iter()
        .flat_map(|dir| SOURCE_DIRS.iter().map(move |sub| dir.join(sub)))
        .flat_map(|dir| crate::tidy::rust_files(&dir))
        .collect();
    files.sort();
    for file in &files {
        match fs::read_to_string(file) {
            Ok(text) => violations.extend(suppression_violations(file, &text)),
            Err(error) => violations.push(format!(
                "{}: required: a source the suppression scan can read; found: {error}",
                file.display()
            )),
        }
    }
    violations
}

/// The finding for an Engine crate whose nearest clippy config is not one
/// of `required`. A directory without a manifest is left to the dependency
/// guard, which reports it.
fn resolution_violation(root: &Path, dir: &Path, required: &[PathBuf]) -> Option<String> {
    let manifest = dir.join("Cargo.toml");
    if !manifest.is_file() {
        return None;
    }
    let resolved = dir
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(root))
        .flat_map(|ancestor| CONFIG_NAMES.iter().map(move |name| ancestor.join(name)))
        .find(|candidate| candidate.is_file());
    if resolved
        .as_ref()
        .is_some_and(|file| required.contains(file))
    {
        return None;
    }
    let found = resolved.map_or_else(
        || "no clippy.toml at or below the workspace root".to_owned(),
        |file| file.display().to_string(),
    );
    Some(format!(
        "{}: clippy reads only the nearest clippy.toml above a crate; required: the nearest clippy.toml is one of {}; found: {found}",
        manifest.display(),
        ENGINE_CLIPPY_TOMLS.join(", ")
    ))
}

/// The root settings and `disallowed-methods` entries `config` lacks or
/// holds with another value, because clippy does not merge it with the
/// root file.
fn root_entry_violations(file: &Path, root: &toml::Table, config: &toml::Table) -> Vec<String> {
    let mut violations = Vec::new();
    for (key, value) in root.iter().filter(|(key, _)| key.as_str() != METHODS_KEY) {
        let found = match config.get(key) {
            Some(held) if held == value => continue,
            Some(held) => format!("`{key} = {held}`"),
            None => format!("no `{key}`"),
        };
        violations.push(format!(
            "{}: required: the root clippy.toml's setting `{key} = {value}`; found: {found}",
            file.display()
        ));
    }
    let held = methods(config);
    for entry in methods(root) {
        if held.contains(&entry) {
            continue;
        }
        let path = entry_path(entry).unwrap_or_default();
        let found = held
            .iter()
            .find(|held| entry_path(held) == Some(path))
            .map_or_else(
                || format!("no entry for `{path}`"),
                |held| format!("`{held}`"),
            );
        violations.push(format!(
            "{}: required: the root clippy.toml's {METHODS_KEY} entry `{entry}`; found: {found}",
            file.display()
        ));
    }
    violations
}

/// The determinism bans `config` lacks, or holds without a reason.
fn ban_violations(file: &Path, config: &toml::Table) -> Vec<String> {
    let held = methods(config);
    DETERMINISM_BANS
        .iter()
        .filter_map(|ban| {
            let entry = held.iter().find(|entry| entry_path(entry) == Some(*ban));
            let found = match entry {
                Some(entry) if has_reason(entry) => return None,
                Some(_) => "an entry with no reason",
                None => "no entry",
            };
            Some(format!(
                "{}: required: the determinism ban `{ban}` with a reason that names the replacement; found: {found}",
                file.display()
            ))
        })
        .collect()
}

/// The `disallowed-methods` entries in `config`, in order.
fn methods(config: &toml::Table) -> Vec<&toml::Value> {
    config
        .get(METHODS_KEY)
        .and_then(toml::Value::as_array)
        .map(|entries| entries.iter().collect())
        .unwrap_or_default()
}

/// The path a `disallowed-methods` entry bans: the entry itself when it is
/// a string, else its `path`.
fn entry_path(entry: &toml::Value) -> Option<&str> {
    entry
        .as_str()
        .or_else(|| entry.get("path").and_then(toml::Value::as_str))
}

/// Whether a `disallowed-methods` entry gives a non-blank reason.
fn has_reason(entry: &toml::Value) -> bool {
    entry
        .get("reason")
        .and_then(toml::Value::as_str)
        .is_some_and(|reason| !reason.trim().is_empty())
}

/// Reads and parses one clippy config, or says why it could not.
fn read_config(file: &Path) -> Result<toml::Table, String> {
    let text =
        fs::read_to_string(file).map_err(|error| format!("unreadable clippy.toml: {error}"))?;
    toml::from_str(&text).map_err(|error| format!("unparseable clippy.toml: {error}"))
}

/// `path`, written with `/`, under `root` with the platform's separators.
fn at(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_path_buf(), |dir, part| dir.join(part))
}

/// The attributes in one source file that suppress the bans, labeled with
/// `file` and the line each opens on.
fn suppression_violations(file: &Path, text: &str) -> Vec<String> {
    let tokens: TokenStream = match text.parse() {
        Ok(tokens) => tokens,
        Err(error) => {
            return vec![format!(
                "{}:{}: required: a source that lexes as Rust, so the suppression scan can read it; found: {error}",
                file.display(),
                error.span().start().line
            )];
        }
    };
    let mut lines = Vec::new();
    crate::unsafe_allowlist::collect_relaxations(tokens, &suppressing_lint, &mut lines);
    let source: Vec<&str> = text.lines().collect();
    lines
        .into_iter()
        .map(|line| {
            let found = line
                .checked_sub(1)
                .and_then(|index| source.get(index))
                .map_or("", |text| text.trim());
            format!(
                "{}:{line}: required: no allow or expect of clippy::disallowed_methods, or of a group that holds it, in an Engine crate, because it reopens the determinism bans; found: `{found}`",
                file.display()
            )
        })
        .collect()
}

/// Whether a lint path is `clippy::` followed by one of
/// [`SUPPRESSING_LINTS`].
fn suppressing_lint(path: &syn::Path) -> bool {
    let segments: Vec<String> = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    matches!(segments.as_slice(), [tool, lint]
        if tool == "clippy" && SUPPRESSING_LINTS.contains(&lint.as_str()))
}

#[cfg(test)]
#[path = "engine_clippy-tests.rs"]
mod tests;
