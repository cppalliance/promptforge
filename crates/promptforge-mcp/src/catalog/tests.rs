//! Unit tests for the catalog's own surface: what an entry exposes, the order
//! entries come back in, the content hash, and the handle that swaps them.
//!
//! The resolution pass has its own tests beside it, in `resolve.rs` and
//! `resolve/blocks.rs`.

use tempfile::TempDir;

use super::*;
use crate::catalog::fixture::{config_at, prompt_source, write, write_prompt};

#[test]
fn embed_text_is_the_name_and_the_description() {
    let dir = TempDir::new().expect("temporary prompts directory");
    let root = dir.path();
    write_prompt(root, "p.md", "research_person", "Research one person");
    let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
    let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("the catalog resolves");
    assert_eq!(
        catalog.entries()[0].embed_text(),
        "research person. Research one person"
    );
}

#[test]
fn hash_follows_the_description_and_is_stable_across_a_re_resolve() {
    let dir = TempDir::new().expect("temporary prompts directory");
    let root = dir.path();
    write_prompt(root, "p.md", "research_person", "Research one person");
    write_prompt(root, "q.md", "other", "Something else");
    let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");

    let first = Catalog::resolve(&config, OnBroken::Reject).expect("resolves");
    let again = Catalog::resolve(&config, OnBroken::Reject).expect("resolves again");
    assert_eq!(
        first.hash(),
        again.hash(),
        "unchanged files, unchanged hash"
    );

    // A body-only edit leaves the hash alone; the description moves it.
    write(
        root,
        "p.md",
        &prompt_source("research_person", "Research one person")
            .replace("return args", "return args .. \"!\""),
    );
    let body_edit = Catalog::resolve(&config, OnBroken::Reject).expect("resolves");
    assert_eq!(first.hash(), body_edit.hash());

    write_prompt(root, "p.md", "research_person", "Research one organization");
    let described = Catalog::resolve(&config, OnBroken::Reject).expect("resolves");
    assert_ne!(first.hash(), described.hash());
}

#[test]
fn entries_are_ordered_by_name_whatever_the_files_are_called() {
    let dir = TempDir::new().expect("temporary prompts directory");
    let root = dir.path();
    write_prompt(root, "zulu.md", "alpha", "First by name");
    write_prompt(root, "alpha.md", "zulu", "Last by name");
    let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
    let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("resolves");
    let names: Vec<&str> = catalog.entries().iter().map(Entry::name).collect();
    assert_eq!(names, ["alpha", "zulu"]);
}

#[test]
fn the_handle_swaps_whole_and_a_held_snapshot_is_unaffected() {
    let dir = TempDir::new().expect("temporary prompts directory");
    let root = dir.path();
    write_prompt(root, "p.md", "first", "The original");
    let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
    let handle = CatalogHandle::new(Catalog::resolve(&config, OnBroken::Reject).expect("resolves"));

    let held = handle.load();
    assert!(held.find("first").is_some());

    write_prompt(root, "p.md", "second", "The replacement");
    handle.store(Catalog::resolve(&config, OnBroken::Reject).expect("re-resolves"));

    assert!(
        held.find("first").is_some(),
        "a run in flight keeps its snapshot"
    );
    assert!(handle.load().find("second").is_some());
}
