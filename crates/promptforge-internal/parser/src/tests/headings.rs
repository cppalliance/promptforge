//! Heading tests: the H1, section nesting, container headings, and heading
//! refusals.

use super::*;

#[test]
fn h1_only_prompt_parses_with_empty_sections() {
    let prompt = parse_ok(&PromptSource::new().h1_prose("Text.").build());
    assert!(prompt.sections.is_empty());
}

#[test]
fn empty_h1_title_errors() {
    let src = "---\nname: x\ndescription: d\n---\n\n#\n\n## S\n\np\n";
    assert_parse_error(src, ParseErrorKind::Structure, "title must not be empty");
}

#[test]
fn preface_before_h1_is_ignored() {
    let src = "---\nname: x\ndescription: d\n---\n\nIgnored preface.\n\n```text\nalso ignored\n```\n\n# T\n\nDescription.\n\n## S\n\np\n";
    let prompt = parse_ok(src);
    assert_eq!(prompt.title, "T");
    assert_eq!(prompt.description_text, "Description.");
    assert_eq!(prompt.entry().expect("has sections").name, "S");
}

#[test]
fn recursive_nesting_h2_h3_h4() {
    let src = PromptSource::new()
        .prose("A", "a")
        .prose("### B", "b")
        .prose("#### C", "c")
        .build();
    let p = parse_ok(&src);
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
    let src = PromptSource::new()
        .prose("S", "> ## Quoted\n> kept\n\n- ## Listed\n\ntail")
        .prose("Next", "next")
        .build();
    let prompt = parse_ok(&src);
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
    let src = PromptSource::new()
        .prose("A", "a")
        .prose("#### D", "d")
        .build();
    assert_parse_error(&src, ParseErrorKind::Structure, "orphan");
}

#[test]
fn orphan_top_level_deep_heading_is_rejected() {
    // The first section heading is an H3 with no parent H2: an orphan.
    let src = PromptSource::new().prose("### A", "a").build();
    assert_parse_error(&src, ParseErrorKind::Structure, "orphan");

    // An H4 top-level section (double skip) is likewise rejected.
    let src = PromptSource::new().prose("#### A", "a").build();
    assert_parse_error(&src, ParseErrorKind::Structure, "orphan");
}

#[test]
fn empty_section_heading_is_rejected() {
    let src = PromptSource::new().prose("", "a").build();
    assert_parse_error(&src, ParseErrorKind::Structure, "must not be empty");
}

#[test]
fn duplicate_sibling_section_names_are_rejected() {
    // Two H2 siblings named `S` are ambiguous section targets.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\na\n\n## S\n\nb\n";
    let err = parse_err(src);
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
    parse_ok(
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n### S\n\nx\n\n## B\n\nb\n\n### S\n\ny\n",
    );
}

#[test]
fn section_markdown_is_the_sections_own_content() {
    let own = "  indented first line\n\n```lua\nvar.n = 1\n```\n\ntext\n\n---\n\n{{ x }}";
    let src = format!(
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n\n{own}\n\n### Child\n\nchild text\n\n## Empty\n\n\n"
    );
    let prompt = parse_ok(&src);
    let section = &prompt.sections[0];
    assert_eq!(
        section.markdown(),
        own,
        "a section's Markdown is its own content byte for byte, stopping at its child"
    );
    assert_eq!(section.children[0].markdown(), "child text");
    assert_eq!(prompt.sections[1].markdown(), "");
}

#[test]
fn first_h2_is_entry_regardless_of_name() {
    let src = PromptSource::new()
        .prose("Zebra", "first")
        .prose("Main", "second")
        .build();
    let p = parse_ok(&src);
    assert_eq!(p.entry().expect("has sections").name, "Zebra");
}
