//! Resolution-pass tests: globbing, exclusion, named-block admission, the
//! reserved-name and duplicate rules, and root confinement.

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
        "---\nname: other\ndescription: d\n---\n\n## S\n\np\n",
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
    assert_eq!(
        error.faults().next().expect("one fault").prompt(),
        Some("twin")
    );
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
fn an_include_pattern_that_climbs_above_the_root_is_refused() {
    // Include patterns are `GlobPattern`s, so one that climbs above the prompts
    // directory is refused at the config boundary rather than reaching
    // resolution as a fault.
    let err = crate::config::Config::from_toml_str(
        "[server]\ntoken = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\nkey = \"gw\"\n\n\
         [catalog]\ninclude = [\"../**/*.md\"]\n",
    )
    .expect_err("an include pattern climbing above the root is refused");
    assert!(err.to_string().contains(".."), "the escape is named: {err}");
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
        "---\nname: empty\ndescription: d\npromptforge: 1\n---\n\n# Only a title\n",
    );

    let config = config_at(root, "[catalog]\ninclude = [\"*.md\"]\n");
    let error = Catalog::resolve(&config, OnBroken::Reject).expect_err("three prompts are broken");
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
