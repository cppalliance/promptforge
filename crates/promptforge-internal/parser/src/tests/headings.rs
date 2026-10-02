//! Heading tests: the H1, section nesting, container headings, and heading
//! refusals.

use super::*;

#[test]
fn h1_only_prompt_parses_with_empty_sections() {
    let src = "---\nname: x\ndescription: d\npromptforge: 0\n---\n\n# Only a title\n\nText.\n";
    let prompt = parse(src).expect("H1-only prompt must parse");
    assert!(prompt.sections.is_empty());
}

#[test]
fn empty_h1_title_errors() {
    let src = "---\nname: x\ndescription: d\n---\n\n#\n\n## S\n\np\n";
    let error = parse(src).expect_err("H1 title must not be empty");
    assert!(error.to_string().contains("title must not be empty"));
}

#[test]
fn preface_before_h1_is_ignored() {
    let src = "---\nname: x\ndescription: d\n---\n\nIgnored preface.\n\n```text\nalso ignored\n```\n\n# T\n\nDescription.\n\n## S\n\np\n";
    let prompt = parse(src).expect("preface is not semantic");
    assert_eq!(prompt.title, "T");
    assert_eq!(prompt.description_text, "Description.");
    assert_eq!(prompt.entry().expect("has sections").name, "S");
}

#[test]
fn recursive_nesting_h2_h3_h4() {
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n### B\n\nb\n\n#### C\n\nc\n";
    let p = parse(src).unwrap();
    let a = &p.sections[0];
    assert_eq!(a.name, "A");
    let b = &a.children[0];
    assert_eq!(b.name, "B");
    assert_eq!(b.level, 3);
    let c = &b.children[0];
    assert_eq!(c.name, "C");
    assert_eq!(c.level, 4);
}

#[test]
fn headings_inside_block_quotes_and_list_items_stay_in_the_section_prose() {
    let src = prompt_src("## S\n\n> ## Quoted\n> kept\n\n- ## Listed\n\ntail\n\n## Next\n\nnext\n");
    let prompt = parse(&src).expect("container headings are prose");
    let names: Vec<&str> = prompt.sections.iter().map(Section::name).collect();
    assert_eq!(
        names,
        ["S", "Next"],
        "container headings are not sections, and a heading after the containers is"
    );
    let section = &prompt.sections[0];
    assert!(section.children.is_empty());
    let prose = section.prose();
    assert!(
        prose.contains("> ## Quoted") && prose.contains("- ## Listed") && prose.contains("tail"),
        "container headings stay in the enclosing prose: {prose}"
    );
}

#[test]
fn skipped_heading_level_is_rejected_as_orphan() {
    // H4 directly under H2 (no intervening H3) is an orphan deep heading:
    // it has no parent H3, so it must be rejected, not reparented to the H2.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n#### D\n\nd\n";
    let err = parse(src).expect_err("an H4 with no parent H3 must be rejected");
    assert!(
        err.to_string().contains("orphan"),
        "expected an orphan-heading error, got: {err}"
    );
}

#[test]
fn orphan_top_level_deep_heading_is_rejected() {
    // The first section heading is an H3 with no parent H2: an orphan.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n### A\n\na\n";
    let err = parse(src).expect_err("an H3 top-level section with no parent H2 must be rejected");
    assert!(
        err.to_string().contains("orphan"),
        "expected an orphan-heading error, got: {err}"
    );

    // An H4 top-level section (double skip) is likewise rejected.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n#### A\n\na\n";
    assert!(
        parse(src).is_err(),
        "an H4 top-level section must be rejected"
    );
}

#[test]
fn empty_section_heading_is_rejected() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## \n\na\n";
    let err = parse(src).expect_err("an empty section heading must be rejected");
    assert!(
        err.to_string().contains("must not be empty"),
        "expected an empty-heading error, got: {err}"
    );
}

#[test]
fn duplicate_sibling_section_names_are_rejected() {
    // Two H2 siblings named `S` are ambiguous section targets.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\na\n\n## S\n\nb\n";
    let err = parse(src).expect_err("duplicate sibling section names must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("duplicate sibling section name"),
        "expected a duplicate-sibling error, got: {err}"
    );
    // Both duplicate locations are named. The first `## S` is at body line 3
    // (line 8 overall) and the second at body line 7 (line 12).
    assert!(
        message.contains("first declared at line 8") && message.contains("again at line 12"),
        "both duplicate locations must be reported, got: {message}"
    );
    // A structured parse error reports a stable kind and a byte span rather
    // than inferring them from the message.
    assert_eq!(err.kind(), ParseErrorKind::Structure);
    let (start, end) = err.span().expect("duplicate section has a span");
    assert!(
        start < end,
        "span must be a non-empty range, got {start}..{end}"
    );

    // The same name under DIFFERENT parents (not siblings) is allowed.
    let ok = "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n### S\n\nx\n\n## B\n\nb\n\n### S\n\ny\n";
    assert!(
        parse(ok).is_ok(),
        "the same name under different parents is not a sibling collision"
    );
}

#[test]
fn first_h2_is_entry_regardless_of_name() {
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## Zebra\n\nfirst\n\n## Main\n\nsecond\n";
    let p = parse(src).unwrap();
    assert_eq!(p.entry().expect("has sections").name, "Zebra");
}
