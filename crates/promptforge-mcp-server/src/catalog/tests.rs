//! Unit tests for the catalog's own surface: what an entry exposes, the order
//! entries come back in, the content hash, and the handle that swaps them.
//!
//! The resolution pass has its own tests beside it, in `resolve.rs` and
//! `resolve/blocks.rs`.

use tempfile::TempDir;

use super::*;
use crate::catalog::fixture::{config_at, prompt_source, unparsable_source, write, write_prompt};

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
fn a_name_only_edit_moves_the_hash() {
    let dir = TempDir::new().expect("temporary prompts directory");
    let root = dir.path();
    write_prompt(root, "p.md", "research_person", "One unchanged description");
    let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
    let before = Catalog::resolve(&config, OnBroken::Reject).expect("resolves");

    // The description is left exactly as it was; only the name changes. The
    // hash must still move, because retrieval ranks on the name too.
    write_prompt(root, "p.md", "research_org", "One unchanged description");
    let after = Catalog::resolve(&config, OnBroken::Reject).expect("resolves");
    assert_ne!(
        before.hash(),
        after.hash(),
        "a name-only edit must move the hash so retrieval rebuilds"
    );
}

#[test]
fn a_healthy_entry_exposes_its_source_and_a_broken_one_does_not() {
    let dir = TempDir::new().expect("temporary prompts directory");
    let root = dir.path();
    write_prompt(root, "healthy.md", "healthy_one", "A healthy prompt");
    // Declares `promptforge:` but omits a required field, so it parses as a
    // broken entry that a reload retains.
    write(root, "broken.md", unparsable_source());
    let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
    let catalog =
        Catalog::resolve(&config, OnBroken::Retain).expect("resolves with a retained broken entry");

    let expected = prompt_source("healthy_one", "A healthy prompt");
    let healthy = catalog.find("healthy_one").expect("the healthy entry");
    assert_eq!(
        healthy.source(),
        Some(expected.as_str()),
        "a healthy entry keeps its exact validated source"
    );

    let broken = catalog.find("broken").expect("the retained broken entry");
    assert_eq!(broken.source(), None, "a broken entry carries no source");
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
    assert!(held.catalog().find("first").is_some());

    write_prompt(root, "p.md", "second", "The replacement");
    handle.store(Generation::new(
        Catalog::resolve(&config, OnBroken::Reject).expect("re-resolves"),
        Retrieval::idle(),
    ));

    assert!(
        held.catalog().find("first").is_some(),
        "a run in flight keeps its snapshot"
    );
    assert!(
        held.catalog().find("second").is_none(),
        "the held snapshot predates the replacement and never gains its entries"
    );

    let current = handle.load();
    assert!(
        current.catalog().find("second").is_some(),
        "the live catalog is the replacement"
    );
    assert!(
        current.catalog().find("first").is_none(),
        "the swap replaced the catalog whole rather than merging into it"
    );
}
