//! List section tests: list detection and the bullet parser.

use super::*;
use crate::list::parse_bullet_items;

#[test]
fn mixed_prose_with_one_bullet_is_not_a_list() {
    // An incidental bullet line in ordinary prose must not force strict
    // list parsing; the section stays prose.
    let src = "---\nname: p\ndescription: d\n---\n\n# T\n\n## S\n\nHere is context.\n- one incidental bullet\nMore prose follows.\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert!(!section.is_list_only(), "mixed prose is not a list");
    assert!(section.items().is_empty());
    assert!(section.prose().contains("incidental bullet"));
}

#[test]
fn pure_list_section_parses_items() {
    let src = "---\nname: p\ndescription: d\n---\n\n# T\n\n## S\n\n- alpha\n- beta\n3. gamma\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert!(section.is_list_only());
    assert_eq!(section.items(), ["alpha", "beta", "gamma"]);
}

#[test]
fn all_marker_list_with_empty_item_is_rejected() {
    // Every nonblank line is a marker, so it is a list; the empty marker is
    // then a hard error rather than a detector miss.
    let src = "---\nname: p\ndescription: d\n---\n\n# T\n\n## S\n\n- alpha\n1.\n- beta\n";
    let error = parse(src).expect_err("empty item must fail");
    assert_eq!(error.kind(), ParseErrorKind::List);
}

#[test]
fn list_error_kind_does_not_depend_on_the_section_name() {
    for section in ["frontmatter", "fence"] {
        let src =
            format!("---\nname: p\ndescription: d\n---\n\n# T\n\n## {section}\n\n- alpha\n1.\n");
        let error = parse(&src).expect_err("an empty list item must fail");
        assert_eq!(error.kind(), ParseErrorKind::List);
    }
}

#[test]
fn bullet_parser_strips_unordered_markers() {
    let items = parse_bullet_items("- alpha\n* beta\n- gamma", "test").unwrap();
    assert_eq!(items, vec!["alpha", "beta", "gamma"]);
}

#[test]
fn bullet_parser_strips_ordered_markers() {
    let items = parse_bullet_items("1. first\n2. second\n3) third", "test").unwrap();
    assert_eq!(items, vec!["first", "second", "third"]);
}

#[test]
fn bullet_parser_ignores_blank_lines() {
    let items = parse_bullet_items("- alpha\n\n- beta\n  \n- gamma", "test").unwrap();
    assert_eq!(items, vec!["alpha", "beta", "gamma"]);
}

#[test]
fn bullet_parser_rejects_non_list_content() {
    let err = parse_bullet_items("- alpha\nnot a bullet\n- gamma", "test")
        .expect_err("non-list content must error");
    assert!(
        err.to_string().contains("non-list content"),
        "error was: {err}"
    );
}

#[test]
fn bullet_parser_rejects_empty_list() {
    let err = parse_bullet_items("", "test").expect_err("empty list must error");
    assert!(err.to_string().contains("no items"), "error was: {err}");
}

#[test]
fn bullet_parser_rejects_empty_item() {
    let err =
        parse_bullet_items("- alpha\n- \n- gamma", "test").expect_err("empty item must error");
    assert!(
        err.to_string().contains("empty bullet item"),
        "error was: {err}"
    );
}

#[test]
fn list_h3_parses_items_at_load_time() {
    let src = prompt_src("## Parent\n\np\n\n### Items\n\n- alpha\n- beta\n");
    let p = parse(&src).unwrap();
    let items_section = &p.sections[0].children[0];
    assert_eq!(items_section.name, "Items");
    assert_eq!(items_section.items, vec!["alpha", "beta"]);
}

#[test]
fn non_list_h3_has_empty_items() {
    let src = prompt_src(
        "## Parent\n\np\n\n### Worker\n\n```lua\nreturn item\n```\n\nDo work on {{ item }}.\n",
    );
    let p = parse(&src).unwrap();
    let worker = &p.sections[0].children[0];
    assert_eq!(worker.name, "Worker");
    assert!(worker.items.is_empty());
}
