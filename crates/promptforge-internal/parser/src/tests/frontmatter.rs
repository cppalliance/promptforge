//! Frontmatter tests: required keys and delimiters, the tool-loop cap,
//! version detection, and declared files.

use super::*;

#[test]
fn name_and_description_are_sufficient_frontmatter_for_parsing() {
    let src = prompt_src("## S\n\np\n");
    let prompt = parse(&src).expect("minimum frontmatter must parse");
    assert_eq!(prompt.frontmatter.name, "x");
}

#[test]
fn missing_frontmatter_delimiter_errors() {
    let src = "# T\n\n## S\n\np\n";
    assert!(parse(src).is_err());
}

#[test]
fn an_indented_delimiter_in_a_block_scalar_does_not_close_the_frontmatter() {
    let src = "---\nname: x\ndescription: |\n  first\n  ---\n  last\n---\n\n# T\n\n## S\n\np\n";
    let prompt = parse(src).expect("an indented --- is block scalar text");
    assert_eq!(prompt.frontmatter.description(), "first\n---\nlast\n");
}

#[test]
fn unknown_frontmatter_field_is_rejected() {
    let src = "---\nname: x\ndescription: d\nnot_a_real_field: 1\n---\n\n# T\n\n## S\n\np\n";
    let err = parse(src).expect_err("an unknown frontmatter field must be rejected");
    assert!(
        err.to_string().contains("not_a_real_field") || err.to_string().contains("unknown field"),
        "expected an unknown-field error, got: {err}"
    );
    // A known-field-only frontmatter still parses.
    let ok = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    assert!(parse(ok).is_ok());
}

#[test]
fn max_tool_iterations_parses_positive_and_defaults_when_absent() {
    let declared =
        "---\nname: x\ndescription: d\nmax_tool_iterations: 20\n---\n\n# T\n\n## S\n\np\n";
    let p = parse(declared).unwrap();
    assert_eq!(
        p.frontmatter.max_tool_iterations,
        MaxToolIterations::Limit(std::num::NonZeroU32::new(20).unwrap())
    );

    let absent = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    let p = parse(absent).unwrap();
    assert_eq!(
        p.frontmatter.max_tool_iterations,
        MaxToolIterations::Default
    );
}

#[test]
fn max_tool_iterations_rejects_zero_negative_and_overflow() {
    let body = |value: &str| {
        format!(
            "---\nname: x\ndescription: d\nmax_tool_iterations: {value}\n---\n\n# T\n\n## S\n\np\n"
        )
    };
    for bad in ["0", "-1", "1001", "100000000000"] {
        let error =
            parse(&body(bad)).expect_err(&format!("max_tool_iterations {bad} must be rejected"));
        assert_eq!(
            error.kind(),
            ParseErrorKind::Frontmatter,
            "value {bad}: {error}"
        );
    }
}

#[test]
fn max_tool_iterations_accepts_the_upper_boundary() {
    let body = format!(
        "---\nname: x\ndescription: d\nmax_tool_iterations: {MAX_TOOL_ITERATIONS}\n---\n\n# T\n\n## S\n\np\n"
    );
    let p = parse(&body).unwrap();
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
fn detection_reads_promptforge_major() {
    let src = "---\nname: x\ndescription: d\npromptforge: 0\n---\n\n## S\n\np\n";
    assert_eq!(promptforge_version(src), Some(0));
}

#[test]
fn detection_needs_only_the_promptforge_key() {
    // No name or description, but the key is present.
    let src = "---\npromptforge: 2\n---\n\n## S\n\np\n";
    assert_eq!(promptforge_version(src), Some(2));
}

#[test]
fn detection_absent_key_is_none() {
    let src = "---\nname: x\ndescription: d\n---\n\n## S\n\np\n";
    assert_eq!(promptforge_version(src), None);
}

#[test]
fn detection_no_frontmatter_is_none() {
    let src = "# Just a title\n\nPlain prose with no frontmatter block at all.\n";
    assert_eq!(promptforge_version(src), None);
}

#[test]
fn detection_malformed_frontmatter_is_none() {
    // Opening delimiter but never closed.
    let unclosed = "---\npromptforge: 0\nname: x\n\n## S\n\np\n";
    assert_eq!(promptforge_version(unclosed), None);

    // Closed, but not valid YAML.
    let bad_yaml = "---\npromptforge: 0\n  : : oops\n---\n\n## S\n\np\n";
    assert_eq!(promptforge_version(bad_yaml), None);
}

#[test]
fn frontmatter_exposes_promptforge_field() {
    let with = "---\nname: x\ndescription: d\npromptforge: 0\n---\n\n# T\n\n## S\n\np\n";
    let p = parse(with).unwrap();
    assert_eq!(p.frontmatter.promptforge, Some(0));

    let without = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    let p = parse(without).unwrap();
    assert_eq!(p.frontmatter.promptforge, None);
}

#[test]
fn frontmatter_parses_input_and_output() {
    let source = concat!(
        "---\n",
        "name: test\n",
        "description: d\n",
        "promptforge: 0\n",
        "input:\n",
        "  path: paper.md\n",
        "  description: The input paper\n",
        "output:\n",
        "  path: report.md\n",
        "  description: The output report\n",
        "---\n\n",
        "# Title\n\n## Only\n\ndone\n",
    );
    let prompt = parse(source).unwrap();
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
    let source = concat!(
        "---\n",
        "name: simple\n",
        "description: no files\n",
        "promptforge: 0\n",
        "---\n\n",
        "# Title\n\n## Only\n\ndone\n",
    );
    let prompt = parse(source).unwrap();
    assert!(prompt.frontmatter().input().is_none());
    assert!(prompt.frontmatter().output().is_none());
}

#[test]
fn promptforge_zero_is_accepted() {
    // `promptforge: 0` is the active engine major: it parses, is exposed on
    // the frontmatter, and is reported by version detection.
    let src = "---\nname: x\ndescription: d\npromptforge: 0\n---\n\n# T\n\n## S\n\np\n";
    let prompt = parse(src).expect("promptforge: 0 must parse");
    assert_eq!(prompt.frontmatter().promptforge(), Some(0));
    assert_eq!(promptforge_version(src), Some(0));
}
