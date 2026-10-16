//! Tests for heading lookup over a visible set.

use super::*;

#[test]
fn resolve_visible_finds_exact_match() {
    let sections = vec![sibling("Worker", 3), sibling("Topics", 3)];
    let found = resolve_visible("### Worker", Visible::all(&sections)).expect("must resolve");
    assert_eq!(found.name(), "Worker");
}

#[test]
fn resolve_visible_missing_heading_lists_available() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_visible("### Missing", Visible::all(&sections))
        .expect_err("missing heading must error");
    assert!(err.to_string().contains("### Worker"), "error was: {err}");
}

#[test]
fn resolve_visible_bare_name_errors() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_visible("Worker", Visible::all(&sections))
        .expect_err("bare name without ### must error");
    assert!(err.to_string().contains("### markers"), "error was: {err}");
}

fn sibling(name: &str, level: u8) -> Section {
    runvil_parser::test_support::synthetic_section(
        name,
        level,
        vec![runvil_parser::test_support::prose_block(String::new())],
    )
}

#[test]
fn resolve_visible_requires_whitespace_after_markers() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_visible("###Worker", Visible::all(&sections))
        .expect_err("no whitespace after markers must error");
    assert!(err.to_string().contains("whitespace"), "error was: {err}");
}

#[test]
fn resolve_visible_marker_only_heading_errors_as_nameless() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_visible("### ", Visible::all(&sections))
        .expect_err("a marker-only heading must error");
    assert!(err.to_string().contains("has no name"), "error was: {err}");
}

#[test]
fn resolve_visible_requires_exact_level() {
    let sections = vec![sibling("Worker", 3)];
    // Same name, wrong marker level, must not resolve.
    let err = resolve_visible("## Worker", Visible::all(&sections))
        .expect_err("a level mismatch must not resolve by name alone");
    assert!(err.to_string().contains("not found"), "error was: {err}");
    // The exact address resolves.
    let ok =
        resolve_visible("### Worker", Visible::all(&sections)).expect("exact address resolves");
    assert_eq!(ok.name(), "Worker");
}

#[test]
fn resolve_visible_rejects_more_than_one_match() {
    let sections = vec![sibling("Worker", 3), sibling("Worker", 3)];
    let err = resolve_visible("### Worker", Visible::all(&sections))
        .expect_err("two identical siblings must be rejected as ambiguous");
    assert!(err.to_string().contains("ambiguous"), "error was: {err}");
}

#[test]
fn find_visible_returns_none_for_an_address_not_in_the_set() {
    let sections = vec![sibling("Worker", 3)];
    let missing = find_visible("### Missing", Visible::all(&sections))
        .expect("a missing name is not an error");
    assert!(missing.is_none(), "a missing name must be None");
    let wrong_level = find_visible("## Worker", Visible::all(&sections))
        .expect("a level mismatch is not an error");
    assert!(wrong_level.is_none(), "a level mismatch must be None");
    let found =
        find_visible("### Worker", Visible::all(&sections)).expect("exact address resolves");
    assert_eq!(found.map(Section::name), Some("Worker"));
}

/// A tree around `#### Self`, four levels deep: `## Grand` holds
/// `### Uncle` and `### Parent`, and `### Parent` holds `#### Sibling` and
/// `#### Self`.
const TREE: &str = concat!(
    "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n# Title\n\n",
    "## Top\n\nx\n\n",
    "## Grand\n\nx\n\n",
    "### Uncle\n\nx\n\n",
    "#### Cousin\n\nx\n\n",
    "### Parent\n\nx\n\n",
    "#### Sibling\n\nx\n\n",
    "##### Niece\n\nx\n\n",
    "#### Self\n\nx\n\n",
    "##### Child\n\nx\n\n",
    "###### Grandchild\n\nx\n\n",
    "## Last\n\nx\n",
);

/// `#### Self`'s home slice: `## Grand`'s children, then `### Parent`'s.
const SELF_HOME: &[usize] = &[1, 1];

fn tree() -> crate::parser::Prompt {
    crate::parser::Prompt::parse(TREE, "reach-test")
        .0
        .expect("the tree parses")
}

fn from_self(root: &[Section]) -> Reachable<'_> {
    Reachable::of(root, SELF_HOME, 1)
}

#[test]
fn a_nested_section_reaches_its_children_siblings_and_each_ancestors_siblings() {
    let prompt = tree();
    let root = runvil_parser::detail::sections(&prompt);
    let expected: [(&str, &[usize], usize); 5] = [
        ("##### Child", &[1, 1, 1], 0),
        ("#### Sibling", &[1, 1], 0),
        ("### Uncle", &[1], 0),
        ("## Top", &[], 0),
        ("## Last", &[], 2),
    ];
    for (heading, path, index) in expected {
        let reached = resolve_reachable(heading, from_self(root))
            .unwrap_or_else(|error| panic!("{heading} must resolve: {error}"));
        assert_eq!(section_heading(reached.section), heading);
        assert_eq!(
            (reached.path.as_slice(), reached.index),
            (path, index),
            "{heading} sits at its own position"
        );
    }
}

#[test]
fn a_nested_section_reaches_no_grandchild_niece_cousin_itself_or_ancestor() {
    let prompt = tree();
    let root = runvil_parser::detail::sections(&prompt);
    for heading in [
        "###### Grandchild",
        "##### Niece",
        "#### Cousin",
        "#### Self",
        "### Parent",
        "## Grand",
    ] {
        let found = find_reachable(heading, from_self(root)).expect("a miss is not an error");
        assert!(found.is_none(), "{heading} must be out of reach");
    }
}

#[test]
fn the_h1_reaches_every_top_level_section_and_no_nested_one() {
    let prompt = tree();
    let root = runvil_parser::detail::sections(&prompt);
    for heading in ["## Top", "## Grand", "## Last"] {
        let reached = resolve_reachable(heading, Reachable::top(root))
            .unwrap_or_else(|error| panic!("{heading} must resolve: {error}"));
        assert!(reached.path.is_empty(), "{heading} is top-level");
    }
    for heading in ["### Uncle", "### Parent", "#### Self"] {
        let found = find_reachable(heading, Reachable::top(root)).expect("a miss is not an error");
        assert!(found.is_none(), "{heading} must be out of the H1's reach");
    }
}

#[test]
fn a_reachable_name_at_another_level_does_not_resolve() {
    let prompt = tree();
    let root = runvil_parser::detail::sections(&prompt);
    for heading in ["### Sibling", "## Uncle", "#### Child", "### Top"] {
        let found = find_reachable(heading, from_self(root)).expect("a miss is not an error");
        assert!(
            found.is_none(),
            "{heading} names a reachable section at the wrong level"
        );
    }
}

#[test]
fn a_reach_miss_lists_only_the_reachable_sections() {
    let prompt = tree();
    let root = runvil_parser::detail::sections(&prompt);
    let error = resolve_reachable("## Missing", from_self(root))
        .expect_err("an unknown heading must error");
    assert_eq!(
        error.to_string(),
        "section heading `## Missing` not found; available sections: \
         #### Sibling, ##### Child, ### Uncle, ## Top, ## Last"
    );
}

#[test]
fn the_visible_set_stays_narrower_than_the_reach() {
    let prompt = tree();
    let root = runvil_parser::detail::sections(&prompt);
    let home = root[1].children()[1].children();
    let visible = find_visible("### Uncle", Visible::of(home, 1)).expect("a miss is not an error");
    assert!(visible.is_none(), "call and jump keep their narrower set");
    assert!(
        find_reachable("### Uncle", from_self(root))
            .expect("a miss is not an error")
            .is_some(),
        "require reaches the parent's sibling"
    );
}
