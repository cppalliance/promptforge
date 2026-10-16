//! Frontmatter tests: required keys and delimiters, the tool-loop cap,
//! version detection, and declared files.

use super::*;

#[test]
fn name_and_description_are_sufficient_frontmatter_for_parsing() {
    let prompt = parse_ok("---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n");
    assert_eq!(prompt.frontmatter.name, "x");
}

#[test]
fn missing_frontmatter_delimiter_errors() {
    assert_eq!(
        parse_err("# T\n\n## S\n\np\n").kind(),
        ParseErrorKind::Frontmatter
    );
}

#[test]
fn an_indented_delimiter_in_a_block_scalar_does_not_close_the_frontmatter() {
    let src = "---\nname: x\ndescription: |\n  first\n  ---\n  last\n---\n\n# T\n\n## S\n\np\n";
    let prompt = parse_ok(src);
    assert_eq!(prompt.frontmatter.description(), "first\n---\nlast\n");
}

#[test]
fn unknown_frontmatter_field_is_rejected() {
    let src = prompt("not_a_real_field: 1", &[]);
    assert_parse_error(&src, ParseErrorKind::Frontmatter, "not_a_real_field");
    // A known-field-only frontmatter still parses.
    parse_ok(&prompt("", &[]));
}

#[test]
fn max_tool_iterations_parses_positive_and_defaults_when_absent() {
    let p = parse_ok(&prompt("max_tool_iterations: 20", &[]));
    assert_eq!(
        p.frontmatter.max_tool_iterations,
        MaxToolIterations::Limit(std::num::NonZeroU32::new(20).unwrap())
    );

    let p = parse_ok(&prompt("", &[]));
    assert_eq!(
        p.frontmatter.max_tool_iterations,
        MaxToolIterations::Default
    );
}

#[test]
fn max_tool_iterations_rejects_zero_negative_and_overflow() {
    for bad in ["0", "-1", "1001", "100000000000"] {
        let error = parse_err(&prompt(&format!("max_tool_iterations: {bad}"), &[]));
        assert_eq!(
            error.kind(),
            ParseErrorKind::Frontmatter,
            "value {bad}: {error}"
        );
    }
}

#[test]
fn max_tool_iterations_accepts_the_upper_boundary() {
    let p = parse_ok(&prompt(
        &format!("max_tool_iterations: {MAX_TOOL_ITERATIONS}"),
        &[],
    ));
    assert_eq!(
        p.frontmatter.max_tool_iterations,
        MaxToolIterations::Limit(std::num::NonZeroU32::new(MAX_TOOL_ITERATIONS).unwrap())
    );
}

#[test]
fn max_tool_iterations_resolve_uses_default_only_when_absent() {
    assert_eq!(MaxToolIterations::Default.resolve(24), 24);
    assert_eq!(
        MaxToolIterations::Limit(std::num::NonZeroU32::new(3).unwrap()).resolve(24),
        3
    );
}

#[test]
fn detection_reads_runvil_major() {
    let src = "---\nname: x\ndescription: d\nrunvil: 0\n---\n\n## S\n\np\n";
    assert_eq!(runvil_version(src), Some(0));
}

#[test]
fn detection_needs_only_the_runvil_key() {
    // No name or description, but the key is present.
    let src = "---\nrunvil: 2\n---\n\n## S\n\np\n";
    assert_eq!(runvil_version(src), Some(2));
}

#[test]
fn detection_absent_key_is_none() {
    let src = "---\nname: x\ndescription: d\n---\n\n## S\n\np\n";
    assert_eq!(runvil_version(src), None);
}

#[test]
fn detection_no_frontmatter_is_none() {
    let src = "# Just a title\n\nPlain prose with no frontmatter block at all.\n";
    assert_eq!(runvil_version(src), None);
}

#[test]
fn detection_malformed_frontmatter_is_none() {
    // Opening delimiter but never closed.
    let unclosed = "---\nrunvil: 0\nname: x\n\n## S\n\np\n";
    assert_eq!(runvil_version(unclosed), None);

    // Closed, but not valid YAML.
    let bad_yaml = "---\nrunvil: 0\n  : : oops\n---\n\n## S\n\np\n";
    assert_eq!(runvil_version(bad_yaml), None);
}

#[test]
fn frontmatter_exposes_runvil_field() {
    let with = "---\nname: x\ndescription: d\nrunvil: 0\n---\n\n# T\n\n## S\n\np\n";
    assert_eq!(parse_ok(with).frontmatter.runvil, Some(0));

    let without = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    assert_eq!(parse_ok(without).frontmatter.runvil, None);
}

#[test]
fn frontmatter_parses_input_and_output() {
    let prompt = parse_ok(&prompt(
        "
        input: { path: paper.md, description: The input paper }
        output: { path: report.md, description: The output report }
        ",
        &[],
    ));
    let fm = prompt.frontmatter();
    let input = fm.input().expect("input declared");
    assert_eq!(input.path(), "paper.md");
    assert_eq!(input.description(), "The input paper");
    let output = fm.output().expect("output declared");
    assert_eq!(output.path(), "report.md");
    assert_eq!(output.description(), "The output report");
}

#[test]
fn frontmatter_without_input_output_still_parses() {
    let prompt = parse_ok(&prompt("", &[]));
    assert!(prompt.frontmatter().input().is_none());
    assert!(prompt.frontmatter().output().is_none());
}

#[test]
fn runvil_zero_is_accepted() {
    // `runvil: 0` is the active Engine major: it parses, is exposed on
    // the frontmatter, and is reported by version detection.
    let src = "---\nname: x\ndescription: d\nrunvil: 0\n---\n\n# T\n\n## S\n\np\n";
    assert_eq!(parse_ok(src).frontmatter().runvil(), Some(0));
    assert_eq!(runvil_version(src), Some(0));
}
