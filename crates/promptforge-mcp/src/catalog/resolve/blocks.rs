//! The `[prompts.NAME]` blocks: the per-prompt exceptions the globs alone
//! cannot express.
//!
//! A block reaches a file no glob matches, or drops one a glob caught. It runs
//! after the globs, so whatever it says is the last word on the prompt it
//! names.

use std::collections::BTreeSet;
use std::path::Path;

use super::{admit, parse, read};
use crate::catalog::Entry;
use crate::config::Config;
use crate::error::Fault;

/// Applies the `[prompts.NAME]` blocks: a `file` reaches a prompt no glob
/// matches, and `enabled = false` drops one. A block with no `file` that
/// matches no globbed prompt is a stale override and a fault, so a rename never
/// leaves a silent no-op behind.
pub(super) fn apply(
    config: &Config,
    root: &Path,
    entries: &mut Vec<Entry>,
    faults: &mut Vec<Fault>,
) {
    let mut disabled: BTreeSet<&str> = BTreeSet::new();

    for (key, block) in &config.prompts {
        if !block.enabled {
            disabled.insert(key.as_str());
        }
        match &block.file {
            Some(file) => {
                if !block.enabled {
                    continue;
                }
                let path = root.join(file);
                // A block names one file deliberately, so whatever is there has
                // to parse: the silent skip is a property of globbing alone.
                let entry = match read(&path).and_then(|source| parse(&source)) {
                    Ok(prompt) => admit(path.clone(), prompt, Some(key)),
                    Err(detail) => Entry::broken(key.clone(), path.clone(), detail),
                };
                match entries.iter().position(|held| held.path() == path) {
                    Some(index) => entries[index] = entry,
                    None => entries.push(entry),
                }
            }
            None => {
                if !entries.iter().any(|held| held.name() == key) {
                    faults.push(Fault::new(
                        Some(key.clone()),
                        None,
                        "[prompts] block matches no prompt; it names no file and no glob found it",
                    ));
                }
            }
        }
    }

    entries.retain(|entry| !disabled.contains(entry.name()));
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::catalog::fixture::{config_at, fault_text, write_prompt};
    use crate::catalog::{Catalog, OnBroken};

    #[test]
    fn a_named_block_drops_one_globbed_prompt_and_leaves_the_rest() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(
            root,
            "research.md",
            "research_person",
            "Research one person",
        );
        write_prompt(root, "scratch.md", "scratch_test", "A scratch prompt");
        write_prompt(root, "plain.md", "plain", "Left to the globs");

        let config = config_at(
            root,
            "[catalog]\ninclude = [\"*.md\"]\n\n\
             [prompts.scratch_test]\nenabled = false\n",
        );
        let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("the catalog resolves");

        assert_eq!(catalog.len(), 2);
        assert!(catalog.find("scratch_test").is_none());
        assert!(catalog.find("research_person").is_some());
        assert!(catalog.find("plain").is_some());
    }

    #[test]
    fn a_named_block_with_a_file_reaches_a_prompt_no_glob_matches() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "top.md", "top", "Globbed");
        write_prompt(
            root,
            "experiments/staker-v3.md",
            "staker",
            "Reached by name",
        );

        let config = config_at(
            root,
            "[catalog]\ninclude = [\"*.md\"]\n\n\
             [prompts.staker]\nfile = \"experiments/staker-v3.md\"\n",
        );
        let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("the catalog resolves");
        assert_eq!(catalog.len(), 2);
        let staker = catalog.find("staker").expect("the file-reached prompt");
        assert_eq!(staker.description(), "Reached by name");
    }

    #[test]
    fn a_named_block_whose_file_a_glob_already_matched_replaces_that_entry() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "top.md", "top", "Globbed and then named");

        let config = config_at(
            root,
            "[catalog]\ninclude = [\"*.md\"]\n\n\
             [prompts.top]\nfile = \"top.md\"\n",
        );
        let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("the catalog resolves");
        assert_eq!(
            catalog.len(),
            1,
            "the block replaces the globbed entry rather than adding a second"
        );
        assert_eq!(
            catalog.find("top").expect("the named prompt").description(),
            "Globbed and then named"
        );
    }

    #[test]
    fn a_named_block_whose_file_declares_another_name_fails() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "top.md", "top", "Globbed");
        write_prompt(root, "experiments/staker.md", "stalker", "Misnamed");

        let config = config_at(
            root,
            "[catalog]\ninclude = [\"*.md\"]\n\n\
             [prompts.staker]\nfile = \"experiments/staker.md\"\n",
        );
        let error = Catalog::resolve(&config, OnBroken::Reject)
            .expect_err("the mismatched name is a fault");
        assert_eq!(error.faults().len(), 1);
        let text = fault_text(&error);
        assert!(text.contains("stalker"), "{text}");
        assert!(text.contains("staker"), "{text}");
    }

    #[test]
    fn a_named_block_matching_nothing_is_a_stale_override() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "top.md", "top", "Globbed");

        let config = config_at(
            root,
            "[catalog]\ninclude = [\"*.md\"]\n\n[prompts.renamed_away]\nenabled = true\n",
        );
        let error =
            Catalog::resolve(&config, OnBroken::Reject).expect_err("the stale override is a fault");
        assert_eq!(error.faults().len(), 1);
        assert_eq!(error.faults()[0].prompt(), Some("renamed_away"));
        assert!(fault_text(&error).contains("matches no prompt"));
    }

    #[test]
    fn a_catalog_level_fault_stops_a_reload_too() {
        let dir = TempDir::new().expect("temporary prompts directory");
        let root = dir.path();
        write_prompt(root, "top.md", "top", "Globbed");
        let config = config_at(
            root,
            "[catalog]\ninclude = [\"*.md\"]\n\n[prompts.renamed_away]\nenabled = true\n",
        );
        assert!(
            Catalog::resolve(&config, OnBroken::Retain).is_err(),
            "only a prompt's own validation is softened on reload"
        );
    }
}
