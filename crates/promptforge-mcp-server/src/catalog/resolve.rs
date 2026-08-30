//! The resolution pass: expand the globs, subtract the excludes, apply the
//! named-block exceptions, and validate what is left.
//!
//! One pass serves both boot and reload. Every check runs in the same order for
//! both; [`OnBroken`] decides only what a prompt that fails one costs.

mod blocks;
mod path;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};
use promptforge_core::observe::NullObserver;
use promptforge_core::parser::Prompt;
use promptforge_core::promptforge_version;
use promptforge_progress::ProgressHandle;

use crate::catalog::{Catalog, Entry, OnBroken};
use crate::config::Config;
use crate::error::{CatalogError, Fault, FaultKind};
use crate::tools::reserved_names;

/// The longest a derived tool name may be, per `^[a-z][a-z0-9_]{0,47}$`.
const MAX_TOOL_NAME_LEN: usize = 48;

/// The most a single prompt file may weigh before the pass refuses to read it.
///
/// A prompt is a short command file, not a data blob; a multi-megabyte match is
/// a misconfigured include or a hostile file, not a prompt, and reading it whole
/// into memory per resolution pass - boot and every reload - is the cost this
/// cap bounds. Two megabytes is far above any real prompt and far below a size
/// that would strain the pass.
const MAX_PROMPT_BYTES: u64 = 2 * 1024 * 1024;

// The names the built-ins own, all four legal under the name regex, come from
// one shared source: [`crate::tools::reserved_names`], itself derived from the
// built-in definitions. Resolution reserves exactly the set dispatch and
// listing are keyed on, so the two cannot drift.
//
// No prompt is published as a tool, so the collision is no longer structural.
// It is still refused, because "run `check_run`" is ambiguous to a person and
// to a model alike, and a boot refusal naming the file is the one version of
// that a prompt author can act on. `need_prompt` is reserved whether or not the
// `picker` feature publishes it, since a name legal in one build and not in
// another is worse than a name that is never legal.

/// Resolves the catalog. See [`Catalog::resolve`] for the contract.
///
/// A `progress` leaf advances one file-count step per globbed file as the pass
/// reaches it and completes when the pass succeeds; a failed pass leaves the
/// leaf unfinished, since the boot it reports into is being refused.
pub(crate) fn resolve(
    config: &Config,
    on_broken: OnBroken,
    progress: Option<&ProgressHandle>,
) -> Result<Catalog, CatalogError> {
    let root = config.paths.prompts.as_path();
    let mut faults: Vec<Fault> = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();

    let files = globbed_files(config, root, &mut faults);
    let total = files.len() as u64;
    for (reached, path) in files.into_iter().enumerate() {
        if let Some(handle) = progress {
            handle.set_units(reached as u64 + 1, total);
        }
        let source = match read(&path) {
            Ok(source) => source,
            Err(detail) => {
                entries.push(Entry::broken_as(
                    stem_name(&path),
                    path,
                    FaultKind::Unreadable,
                    detail,
                ));
                continue;
            }
        };
        if promptforge_version(&source).is_none() {
            // A well-formed frontmatter that simply lacks `promptforge:` is not
            // a prompt - a glob names a directory, not a prompt catalog - and is
            // skipped without comment. A file that opened a frontmatter block
            // and named the marker, but whose block will not parse, was an
            // attempt at a prompt: it becomes a broken entry rather than
            // vanishing, so a malformed prompt produces the actionable fault
            // `OnBroken::Retain` promises rather than silently disappearing.
            if malformed_prompt_candidate(&source) {
                entries.push(Entry::broken_as(
                    stem_name(&path),
                    path,
                    FaultKind::Unparsable,
                    "declares a promptforge frontmatter that does not parse",
                ));
            }
            continue;
        }
        entries.push(match parse(&source) {
            Ok(prompt) => admit(path, source, prompt, None),
            Err(detail) => Entry::broken_as(stem_name(&path), path, FaultKind::Unparsable, detail),
        });
    }

    blocks::apply(config, root, &mut entries, &mut faults);

    if on_broken == OnBroken::Reject {
        for entry in &entries {
            if let Some(problem) = entry.problem() {
                faults.push(Fault::new(
                    entry.problem_kind().unwrap_or(FaultKind::Unparsable),
                    Some(entry.name().to_string()),
                    Some(entry.path().to_path_buf()),
                    problem,
                ));
            }
        }
    }

    faults.extend(duplicate_faults(&entries));
    if entries.is_empty() {
        faults.push(Fault::new(
            FaultKind::Empty,
            None,
            None,
            "no prompts resolved; check [catalog].include and [prompts.*]",
        ));
    }

    if faults.is_empty() {
        if let Some(handle) = progress {
            handle.complete();
        }
        Ok(Catalog::new(entries))
    } else {
        Err(CatalogError::new(faults))
    }
}

/// Every file the include patterns reach and the exclude patterns keep.
///
/// The set is sorted and deduplicated, so two patterns matching one file yield
/// one entry and the pass does not depend on the filesystem's own ordering. A
/// pattern that matches a directory rather than a file is ignored: a glob names
/// prompts, and a directory is not one.
fn globbed_files(config: &Config, root: &Path, faults: &mut Vec<Fault>) -> Vec<PathBuf> {
    let mut matched: BTreeSet<PathBuf> = BTreeSet::new();
    for pattern in &config.catalog.include {
        // The pattern is already shape-validated and known to compile: it
        // crossed the config boundary as a `GlobPattern`. Only the join to this
        // root can still fail, so that is the only fault raised here.
        match expand(root, pattern.as_str()) {
            Ok(paths) => matched.extend(paths),
            Err(detail) => faults.push(Fault::new(
                FaultKind::Pattern,
                None,
                None,
                format!("include {:?}: {detail}", pattern.as_str()),
            )),
        }
    }

    // Compiled at the config boundary too, so the error arm is unreachable in
    // practice; it is kept as a fault rather than a panic so a lib path never
    // unwraps.
    let mut excludes: Vec<Pattern> = Vec::with_capacity(config.catalog.exclude.len());
    for pattern in &config.catalog.exclude {
        match Pattern::new(pattern.as_str()) {
            Ok(compiled) => excludes.push(compiled),
            Err(e) => faults.push(Fault::new(
                FaultKind::Pattern,
                None,
                None,
                format!("exclude {:?}: {e}", pattern.as_str()),
            )),
        }
    }

    // Confinement is by canonical ancestry, so a symlink under the root that
    // points outside it is resolved to its target and dropped rather than
    // served as if it were a prompt in the catalog.
    matched
        .into_iter()
        .filter(|p| p.is_file() && path::confined(root, p) && !is_excluded(p, root, &excludes))
        .collect()
}

/// Expands one include pattern against the prompts directory.
fn expand(root: &Path, pattern: &str) -> Result<Vec<PathBuf>, String> {
    let joined = root.join(pattern);
    let text = joined
        .to_str()
        .ok_or_else(|| "pattern path is not valid UTF-8".to_string())?;
    let paths = glob::glob_with(text, match_options()).map_err(|e| e.to_string())?;
    paths
        .map(|entry| entry.map_err(|e| e.to_string()))
        .collect()
}

/// Whether any exclude pattern matches `path` taken relative to the prompts
/// directory, which is what makes `drafts/**` mean what it reads as.
fn is_excluded(path: &Path, root: &Path, excludes: &[Pattern]) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    excludes
        .iter()
        .any(|pattern| pattern.matches_path_with(relative, match_options()))
}

/// `*` stops at a separator and `**` crosses one, which is what the
/// configuration promises.
fn match_options() -> MatchOptions {
    MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    }
}

/// Reads one candidate file, reporting the failure as a validation detail.
///
/// The read is capped at [`MAX_PROMPT_BYTES`]: a file larger than that is not a
/// prompt, and reading it whole per pass is a cost the cap bounds. The metadata
/// check rejects an oversized file up front, but is not trusted alone - a file
/// can grow between that stat and the read - so the read itself is bounded
/// through [`Read::take`](std::io::Read::take), matching `Config::load`.
fn read(path: &Path) -> Result<String, String> {
    use std::io::Read as _;

    if let Ok(metadata) = std::fs::metadata(path)
        && metadata.len() > MAX_PROMPT_BYTES
    {
        return Err(format!(
            "is {} bytes, over the {MAX_PROMPT_BYTES}-byte prompt limit",
            metadata.len()
        ));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("unreadable: {e}"))?;
    // Read one byte past the cap so an exactly-limit file still loads while a
    // larger one - or one that grew since the stat above - is caught without
    // ever pulling the whole file into memory.
    let mut source = String::new();
    file.take(MAX_PROMPT_BYTES + 1)
        .read_to_string(&mut source)
        .map_err(|e| format!("unreadable: {e}"))?;
    if source.len() as u64 > MAX_PROMPT_BYTES {
        return Err(format!("is over the {MAX_PROMPT_BYTES}-byte prompt limit"));
    }
    Ok(source)
}

/// Whether a file that carries no readable `promptforge:` version nonetheless
/// attempted to be a prompt, so it is a malformed candidate rather than an
/// ordinary non-prompt file.
///
/// [`promptforge_version`] returns `None` both for a well-formed frontmatter
/// that omits the marker and for a frontmatter that is malformed, unclosed, or
/// carries a non-numeric marker. This distinguishes the two the only way a
/// caller of that lossy probe can: a file that opens a `---` frontmatter block
/// and names `promptforge` inside it meant to be a prompt.
fn malformed_prompt_candidate(source: &str) -> bool {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut lines = source.lines();
    if !matches!(lines.next(), Some(line) if line.trim() == "---") {
        return false;
    }
    // Scan the opened block for the marker. An unclosed block scans to the end,
    // which is correct: a block that never closed but named the marker was an
    // attempt at a prompt.
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if line.trim_start().starts_with("promptforge") {
            return true;
        }
    }
    false
}

/// Parses one candidate file, reporting the failure as a validation detail.
fn parse(source: &str) -> Result<Prompt, String> {
    Prompt::parse(source, "catalog", &NullObserver::default())
        .map_err(|e| format!("does not parse: {e}"))
}

/// Turns a parsed prompt into an entry, checking the frontmatter name and, for
/// a named block, that the name is the one the block was keyed on.
fn admit(path: PathBuf, source: String, prompt: Prompt, block_key: Option<&str>) -> Entry {
    let name = prompt.frontmatter().name().to_owned();
    if !is_valid_tool_name(&name) {
        let detail = format!(
            "tool name {name:?} is not ^[a-z][a-z0-9_]{{0,{}}}$",
            MAX_TOOL_NAME_LEN - 1
        );
        let fallback = block_key.map_or_else(|| stem_name(&path), ToString::to_string);
        return Entry::broken_as(
            safe_placeholder(fallback),
            path,
            FaultKind::InvalidName,
            detail,
        );
    }
    if reserved_names().any(|reserved| reserved == name.as_str()) {
        let detail = format!(
            "prompt name {name:?} is reserved: a built-in already answers to it, so \"run {name}\" is ambiguous"
        );
        // The broken entry falls back to the file stem rather than keeping the
        // reserved name, and the fallback is itself made non-reserved, so a
        // reload that retains it cannot offer a prompt under a built-in's own
        // name even when the file stem happens to equal a built-in.
        return Entry::broken_as(
            safe_placeholder(stem_name(&path)),
            path,
            FaultKind::InvalidName,
            detail,
        );
    }
    if let Some(key) = block_key
        && key != name
    {
        let detail = format!("frontmatter name {name:?} does not match its [prompts.{key}] block");
        return Entry::broken_as(key.to_string(), path, FaultKind::InvalidName, detail);
    }
    Entry::healthy(path, source, prompt)
}

/// One fault per name two or more healthy prompts declare, each naming every
/// file that declared it.
///
/// A broken entry takes no part. Its name is a placeholder the pass invented
/// from the file stem or the `[prompts.NAME]` key, not one the file declared,
/// so a placeholder that happened to equal a healthy prompt's name would
/// otherwise abort the whole pass over a collision nothing declared - the
/// reload-wide freeze decision 15 forbids. A broken entry is identified by its
/// path, which is the only thing about it the pass actually knows.
fn duplicate_faults(entries: &[Entry]) -> Vec<Fault> {
    let mut by_name: BTreeMap<&str, Vec<&Path>> = BTreeMap::new();
    for entry in entries.iter().filter(|entry| entry.problem().is_none()) {
        by_name.entry(entry.name()).or_default().push(entry.path());
    }
    by_name
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(name, paths)| {
            let files: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
            Fault::new(
                FaultKind::Duplicate,
                Some(name.to_string()),
                None,
                format!("declared by {} prompts: {}", files.len(), files.join(", ")),
            )
        })
        .collect()
}

/// The name a broken entry falls back to when no frontmatter name is available.
fn stem_name(path: &Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// A broken entry's placeholder name, guaranteed not to equal a built-in's.
///
/// A file stem can itself be a reserved name (a file literally called
/// `list_prompts.md`). Left alone it would let a retained broken entry sit under
/// a built-in's own name, the very ambiguity admission refuses, so a reserved
/// placeholder is suffixed into a name no tool answers to.
fn safe_placeholder(name: String) -> String {
    if reserved_names().any(|reserved| reserved == name.as_str()) {
        format!("{name} (broken)")
    } else {
        name
    }
}

/// Whether a frontmatter name is usable verbatim as an MCP tool name.
///
/// The check is the whole derivation: a transform would let two distinct
/// frontmatter names collide in `tools/list`, and the absence of uppercase and
/// of `-` is what makes the lenient name resolution a caller gets from
/// `run_prompt` unable to merge two legal names.
fn is_valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name.len() <= MAX_TOOL_NAME_LEN
        && first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
