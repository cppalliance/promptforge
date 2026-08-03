//! The resolution pass: expand the globs, subtract the excludes, apply the
//! named-block exceptions, and validate what is left.
//!
//! One pass serves both boot and reload. Every check runs in the same order for
//! both; [`OnBroken`] decides only what a prompt that fails one costs.

mod blocks;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};
use promptforge_core::parser::Prompt;
use promptforge_core::promptforge_version;

use crate::catalog::{Catalog, Entry, OnBroken};
use crate::config::Config;
use crate::error::{CatalogError, Fault};
use crate::tools::{CHECK_RUN, LIST_PROMPTS, NEED_PROMPT, RUN_PROMPT};

/// The longest a derived tool name may be, per `^[a-z][a-z0-9_]{0,47}$`.
const MAX_TOOL_NAME_LEN: usize = 48;

/// The names the built-ins own, all four legal under the name regex.
///
/// No prompt is published as a tool, so the collision is no longer structural.
/// It is still refused, because "run `check_run`" is ambiguous to a person and
/// to a model alike, and a boot refusal naming the file is the one version of
/// that a prompt author can act on.
/// `need_prompt` is reserved whether or not the `picker` feature publishes it,
/// since a name that is legal in one build and not in another is worse than a
/// name that is never legal.
const RESERVED_NAMES: [&str; 4] = [LIST_PROMPTS, RUN_PROMPT, NEED_PROMPT, CHECK_RUN];

/// Resolves the catalog. See [`Catalog::resolve`] for the contract.
pub(crate) fn resolve(config: &Config, on_broken: OnBroken) -> Result<Catalog, CatalogError> {
    let root = config.paths.prompts.as_path();
    let mut faults: Vec<Fault> = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();

    for path in globbed_files(config, root, &mut faults) {
        let source = match read(&path) {
            Ok(source) => source,
            Err(detail) => {
                entries.push(Entry::broken(stem_name(&path), path, detail));
                continue;
            }
        };
        // A glob names a directory, not a prompt, so a file that is not one is
        // not the operator's mistake and is skipped without comment.
        if promptforge_version(&source).is_none() {
            continue;
        }
        entries.push(match parse(&source) {
            Ok(prompt) => admit(path, prompt, None),
            Err(detail) => Entry::broken(stem_name(&path), path, detail),
        });
    }

    blocks::apply(config, root, &mut entries, &mut faults);

    if on_broken == OnBroken::Reject {
        for entry in &entries {
            if let Some(problem) = entry.problem() {
                faults.push(Fault::new(
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
            None,
            None,
            "no prompts resolved; check [catalog].include and [prompts.*]",
        ));
    }

    if faults.is_empty() {
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
        match expand(root, pattern) {
            Ok(paths) => matched.extend(paths),
            Err(detail) => faults.push(Fault::new(
                None,
                None,
                format!("include {pattern:?}: {detail}"),
            )),
        }
    }

    let mut excludes: Vec<Pattern> = Vec::with_capacity(config.catalog.exclude.len());
    for pattern in &config.catalog.exclude {
        match Pattern::new(pattern) {
            Ok(compiled) => excludes.push(compiled),
            Err(e) => faults.push(Fault::new(None, None, format!("exclude {pattern:?}: {e}"))),
        }
    }

    matched
        .into_iter()
        .filter(|path| path.is_file() && !is_excluded(path, root, &excludes))
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
fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("unreadable: {e}"))
}

/// Parses one candidate file, reporting the failure as a validation detail.
fn parse(source: &str) -> Result<Prompt, String> {
    Prompt::parse(source).map_err(|e| format!("does not parse: {e}"))
}

/// Turns a parsed prompt into an entry, checking the frontmatter name and, for
/// a named block, that the name is the one the block was keyed on.
fn admit(path: PathBuf, prompt: Prompt, block_key: Option<&str>) -> Entry {
    let name = prompt.frontmatter.name.clone();
    if !is_valid_tool_name(&name) {
        let detail = format!(
            "tool name {name:?} is not ^[a-z][a-z0-9_]{{0,{}}}$",
            MAX_TOOL_NAME_LEN - 1
        );
        let fallback = block_key.map_or_else(|| stem_name(&path), ToString::to_string);
        return Entry::broken(fallback, path, detail);
    }
    if RESERVED_NAMES.contains(&name.as_str()) {
        let detail = format!(
            "prompt name {name:?} is reserved: a built-in already answers to it, so \"run {name}\" is ambiguous"
        );
        // The broken entry falls back to the file stem rather than keeping the
        // reserved name, so a reload that retains it cannot offer a prompt
        // under a built-in's own name.
        return Entry::broken(stem_name(&path), path, detail);
    }
    if let Some(key) = block_key
        && key != name
    {
        let detail = format!("frontmatter name {name:?} does not match its [prompts.{key}] block");
        return Entry::broken(key.to_string(), path, detail);
    }
    Entry::healthy(path, prompt)
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::catalog::fixture::{config_at, fault_text, unparsable_source, write, write_prompt};
    use crate::catalog::{Catalog, Entry, OnBroken};

    #[test]
    fn a_recursive_pattern_reaches_a_nested_prompt_and_a_flat_one_does_not() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "top.md", "top", "The top-level prompt");
        write_prompt(root, "governance/deep.md", "deep", "The nested prompt");

        let flat = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
        let catalog = Catalog::resolve(&flat, OnBroken::Reject).expect("the flat catalog resolves");
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.entries()[0].name(), "top");

        let recursive = config_at(root, "[catalog]\ninclude = [\"governance/**/*.md\"]\n");
        let catalog =
            Catalog::resolve(&recursive, OnBroken::Reject).expect("the recursive catalog resolves");
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.entries()[0].name(), "deep");
    }

    #[test]
    fn exclude_beats_include() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "keep.md", "keep", "Kept");
        write_prompt(
            root,
            "_hidden.md",
            "hidden",
            "Excluded by a leading underscore",
        );
        write_prompt(root, "drafts/wip.md", "wip", "Excluded by directory");

        let config = config_at(
            root,
            "[catalog]\ninclude = [\"*.md\", \"drafts/**/*.md\"]\nexclude = [\"_*.md\", \"drafts/**\"]\n",
        );
        let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("the catalog resolves");
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.entries()[0].name(), "keep");
    }

    #[test]
    fn a_markdown_file_that_is_not_a_prompt_is_skipped_silently() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "real.md", "real", "A real prompt");
        write(root, "notes.md", "# Just notes\n\nNo frontmatter at all.\n");
        write(
            root,
            "other.md",
            "---\nname: other\ndescription: d\nversion: 1\n---\n\n## S\n\np\n",
        );

        let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
        let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("the catalog resolves");
        assert_eq!(
            catalog.len(),
            1,
            "only the file declaring promptforge: is a prompt"
        );
        assert_eq!(catalog.entries()[0].name(), "real");
    }

    #[test]
    fn two_files_declaring_one_name_fail_naming_both() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "one.md", "twin", "The first");
        write_prompt(root, "two.md", "twin", "The second");

        let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
        let error = Catalog::resolve(&config, OnBroken::Reject).expect_err("one name, two files");
        assert_eq!(error.faults().len(), 1);
        assert_eq!(error.faults()[0].prompt(), Some("twin"));
        let text = fault_text(&error);
        assert!(text.contains("one.md"), "{text}");
        assert!(text.contains("two.md"), "{text}");
    }

    #[test]
    fn a_broken_file_never_collides_with_a_name_a_prompt_declared() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "good.md", "good", "The one that works");
        write_prompt(
            root,
            "research.md",
            "research_person",
            "Research one person",
        );
        // Two saves in flight, both unparsable, both under a stem that is only a
        // placeholder: one of them is the name a healthy prompt declares.
        write(root, "drafts/research_person.md", unparsable_source());
        write(root, "spikes/research_person.md", unparsable_source());

        let config = config_at(root, "[catalog]\ninclude = [\"**/*.md\"]\n");

        let catalog = Catalog::resolve(&config, OnBroken::Retain).expect("a reload keeps going");
        let serving: Vec<&str> = catalog
            .entries()
            .iter()
            .filter(|entry| entry.prompt().is_some())
            .map(Entry::name)
            .collect();
        assert_eq!(serving, ["good", "research_person"]);
        assert_eq!(catalog.len(), 4, "both broken files keep their place");

        let error = Catalog::resolve(&config, OnBroken::Reject).expect_err("two files are broken");
        assert_eq!(
            error.faults().len(),
            2,
            "each broken file, and no duplicate-name fault: {}",
            fault_text(&error)
        );
    }

    #[test]
    fn an_empty_resolved_catalog_fails() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let config = config_at(dir.path(), "[catalog]\ninclude = [\"*.md\"]\n");
        let error =
            Catalog::resolve(&config, OnBroken::Reject).expect_err("an empty catalog is a fault");
        assert_eq!(error.faults().len(), 1);
        assert!(fault_text(&error).contains("no prompts resolved"));
    }

    #[test]
    fn a_name_that_is_not_a_legal_tool_name_fails() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "ok.md", "fine", "Fine");
        write_prompt(root, "bad.md", "Research-Person", "Uppercase and a hyphen");

        let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
        let error = Catalog::resolve(&config, OnBroken::Reject).expect_err("the name is not legal");
        assert_eq!(error.faults().len(), 1);
        let text = fault_text(&error);
        assert!(text.contains("Research-Person"), "{text}");
        assert!(text.contains("bad.md"), "{text}");
    }

    #[test]
    fn a_prompt_named_for_a_built_in_fails_the_boot_naming_the_collision() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "ok.md", "fine", "Fine");
        for (file, name) in [
            ("lister.md", "list_prompts"),
            ("runner.md", "run_prompt"),
            ("needer.md", "need_prompt"),
            ("checker.md", "check_run"),
        ] {
            write_prompt(root, file, name, "Shadows a built-in");
        }

        let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
        let error =
            Catalog::resolve(&config, OnBroken::Reject).expect_err("a reserved name is a fault");
        assert_eq!(error.faults().len(), 4);
        let text = fault_text(&error);
        for name in ["list_prompts", "run_prompt", "need_prompt", "check_run"] {
            assert!(text.contains(name), "the collision is named: {text}");
        }
        for file in ["lister.md", "runner.md", "needer.md", "checker.md"] {
            assert!(text.contains(file), "the file is named: {text}");
        }
    }

    #[test]
    fn a_reload_keeps_a_built_in_name_collision_to_the_one_prompt() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "good.md", "good", "The one that works");
        write_prompt(root, "lister.md", "list_prompts", "Shadows a built-in");

        let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
        let catalog = Catalog::resolve(&config, OnBroken::Retain)
            .expect("one badly named prompt does not freeze a reload");
        assert!(
            catalog
                .find("good")
                .is_some_and(|entry| entry.prompt().is_some()),
            "every other prompt keeps serving"
        );
        assert!(
            catalog.find("list_prompts").is_none(),
            "the broken entry never takes the built-in's name"
        );
        let broken = catalog.find("lister").expect("the entry keeps its place");
        assert!(broken.prompt().is_none());
        assert!(
            broken.problem().is_some_and(|p| p.contains("reserved")),
            "{:?}",
            broken.problem()
        );
    }

    #[test]
    fn three_independent_faults_are_all_reported_with_prompt_and_path() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "good.md", "good", "The one that works");
        // Declares promptforge:, so it is a prompt, but its frontmatter is short
        // a required field.
        write(root, "unparsable.md", unparsable_source());
        write_prompt(root, "upper.md", "Shouty", "An illegal tool name");
        write(
            root,
            "nosections.md",
            "---\nname: empty\ndescription: d\nversion: 1\npromptforge: 1\n---\n\n# Only a title\n",
        );

        let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
        let error =
            Catalog::resolve(&config, OnBroken::Reject).expect_err("three prompts are broken");
        assert_eq!(error.faults().len(), 3);
        for fault in error.faults() {
            assert!(fault.prompt().is_some(), "{fault}");
            assert!(fault.path().is_some(), "{fault}");
        }
        let text = fault_text(&error);
        for file in ["unparsable.md", "upper.md", "nosections.md"] {
            assert!(text.contains(file), "{text}");
        }
    }

    #[test]
    fn retain_keeps_a_broken_prompt_as_an_entry_and_reject_does_not() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "good.md", "good", "The one that works");
        write(root, "broken.md", unparsable_source());
        let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");

        assert!(Catalog::resolve(&config, OnBroken::Reject).is_err());

        let catalog = Catalog::resolve(&config, OnBroken::Retain).expect("a reload keeps going");
        assert_eq!(catalog.len(), 2);
        let good = catalog.find("good").expect("the healthy prompt");
        assert!(good.problem().is_none());
        assert!(good.prompt().is_some());
        // The frontmatter never parsed, so the entry falls back to the file stem.
        let broken = catalog
            .find("broken")
            .expect("the broken prompt keeps its place");
        assert!(broken.prompt().is_none());
        assert!(
            broken
                .problem()
                .is_some_and(|p| p.contains("does not parse")),
            "{:?}",
            broken.problem()
        );
    }
}
