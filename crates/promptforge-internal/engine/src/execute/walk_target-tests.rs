//! Tests for library resolution: what a `require` builds from a section,
//! the key naming its position, and the reach a keyed lookup resolves from.

use super::*;
use crate::lua::LuaProgram;
use promptforge_parser::test_support::{prose_block, synthetic_section};

/// `## Grand` holds `### Parent` and a `### Helpers`, `### Parent` holds
/// `#### Sibling` and `#### Self`, `## Top` holds a second `### Helpers`,
/// and `## Lib` is laid out as prose, Lua, Lua, prose, Lua, then trailing
/// prose.
const TREE: &str = concat!(
    "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# Title\n\n",
    "## Top\n\nx\n\n",
    "### Helpers\n\nx\n\n",
    "## Grand\n\nx\n\n",
    "### Parent\n\nx\n\n",
    "#### Sibling\n\nx\n\n",
    "##### Niece\n\nx\n\n",
    "#### Self\n\nx\n\n",
    "### Helpers\n\nx\n\n",
    "## Lib\n\nIntro.\n\n",
    "```lua\nfirst = 1\n```\n\n",
    "```lua\nsecond = 2\n```\n\n",
    "Later.\n\n",
    "```lua\nthird = 3\n```\n\n",
    "Trailing.\n",
);

fn tree() -> Prompt {
    Prompt::parse(TREE, "library-test")
        .0
        .expect("the tree parses")
}

/// `#### Self`: index 1 of `### Parent`'s children, which sit under
/// `## Grand`.
fn self_origin() -> RequireOrigin {
    RequireOrigin::Section(SlicePath::root().child(1).child(0), 1)
}

fn program(source: &str) -> LuaProgram {
    LuaProgram::compile_internal(source, "library-test").expect("the block compiles")
}

fn sources(library: &Library) -> Vec<&str> {
    library
        .blocks
        .iter()
        .map(|block| block.program.source())
        .collect()
}

fn templates(library: &Library) -> Vec<&str> {
    library
        .blocks
        .iter()
        .map(|block| block.prose.as_str())
        .collect()
}

#[test]
fn a_library_pairs_each_program_with_the_prose_right_before_it() {
    let section = synthetic_section(
        "Lib",
        2,
        vec![
            prose_block("Intro.".to_owned()),
            Block::Lua(program("first = 1")),
            Block::Lua(program("second = 2")),
            prose_block("Later.".to_owned()),
            Block::Lua(program("third = 3")),
        ],
    );
    let key = LibraryKey::new("2".to_owned());
    let library = build_library(&section, key.clone()).expect("the library builds");
    assert_eq!(library.key, key);
    assert_eq!(library.heading, "## Lib");
    assert_eq!(sources(&library), ["first = 1", "second = 2", "third = 3"]);
    assert_eq!(templates(&library), ["Intro.", "", "Later."]);
}

#[test]
fn resolve_library_builds_a_parsed_section_without_its_trailing_prose() {
    let prompt = tree();
    let library =
        resolve_library(&prompt, &RequireOrigin::H1, "## Lib", None).expect("## Lib resolves");
    assert_eq!(library.heading, "## Lib");
    assert_eq!(sources(&library), ["first = 1", "second = 2", "third = 3"]);
    assert_eq!(templates(&library), ["Intro.", "", "Later."]);
}

#[test]
fn a_key_maps_back_to_its_section_position() {
    let prompt = tree();
    let library =
        resolve_library(&prompt, &self_origin(), "#### Sibling", None).expect("a sibling resolves");
    assert_eq!(
        RequireOrigin::from_key(&library.key).expect("the key parses"),
        RequireOrigin::Section(SlicePath::root().child(1).child(0), 0)
    );
    let top = resolve_library(&prompt, &RequireOrigin::H1, "## Top", None)
        .expect("a top-level section resolves");
    assert_eq!(
        RequireOrigin::from_key(&top.key).expect("the key parses"),
        RequireOrigin::Section(SlicePath::root(), 0)
    );
}

#[test]
fn one_heading_from_two_positions_names_two_libraries() {
    let prompt = tree();
    let near = resolve_library(&prompt, &self_origin(), "### Helpers", None)
        .expect("the parent's sibling resolves");
    let top = RequireOrigin::Section(SlicePath::root(), 0);
    let far = resolve_library(&prompt, &top, "### Helpers", None)
        .expect("a top-level section's child resolves");
    assert_eq!(near.heading, far.heading);
    assert_ne!(near.key, far.key, "each position keys its own library");
}

#[test]
fn a_keyed_lookup_resolves_from_the_librarys_reach() {
    let prompt = tree();
    let sibling =
        resolve_library(&prompt, &self_origin(), "#### Sibling", None).expect("a sibling resolves");
    resolve_library(&prompt, &RequireOrigin::H1, "##### Niece", None)
        .expect_err("the H1 cannot reach a nested section");
    let niece = resolve_library(
        &prompt,
        &RequireOrigin::H1,
        "##### Niece",
        Some(&sibling.key),
    )
    .expect("the library reaches its own child");
    assert_eq!(niece.heading, "##### Niece");
    resolve_library(&prompt, &RequireOrigin::H1, "## Grand", Some(&sibling.key))
        .expect_err("the key replaces the caller's reach: ## Grand is the library's ancestor");
}

#[test]
fn a_key_that_names_no_position_is_an_internal_error() {
    for identity in ["", "x", "1.", "1..2"] {
        let error = RequireOrigin::from_key(&LibraryKey::new(identity.to_owned()))
            .expect_err("a foreign key must not parse");
        assert!(
            matches!(error, Error::Internal { .. }),
            "{identity:?} gave {error}"
        );
    }
}
