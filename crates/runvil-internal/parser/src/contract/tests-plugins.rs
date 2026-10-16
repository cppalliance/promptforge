//! Tests for the `plugins` key: Plugin ids, entry forms, and duplicates.

use super::*;

#[test]
fn a_plugin_id_must_be_exactly_one_segment() {
    for id in ["runvil/web", "web/fetch", "web/", "runvil//web"] {
        let error = parse_err(&prompt(&format!("plugins: [{id}]"), &[]));
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{id}: {error}");
    }
    assert_parse_error(
        &prompt("plugins: [runvil/web]", &[]),
        ParseErrorKind::Frontmatter,
        "a Plugin id is one segment, such as `web`",
    );
}

#[test]
fn an_at_sign_in_a_plugin_id_is_rejected() {
    // A Plugin id carries no version, so `@` is a parse error.
    let error = parse_err(&prompt("plugins: [web@1]", &[]));
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_plugin_id_with_uppercase_is_rejected() {
    let error = parse_err(&prompt("plugins: [Web]", &[]));
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_plugin_entry_must_be_a_plain_name() {
    let error = parse_err(&prompt("plugins: [42]", &[]));
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn the_map_form_of_a_plugin_entry_is_refused_naming_the_plain_name_form() {
    for entry in [
        "  - ref: web\n",
        "  - ref: web\n    optional: true\n",
        "  - ref: web\n    config: { depth: 1 }\n",
    ] {
        let src =
            format!("---\nname: x\ndescription: d\nplugins:\n{entry}---\n\n# T\n\n## S\n\np\n");
        let error = parse_err(&src);
        assert_eq!(
            error.kind(),
            ParseErrorKind::Frontmatter,
            "{entry}: {error}"
        );
        assert!(
            error
                .to_string()
                .contains("expected a Plugin's plain name, such as `web`"),
            "the refusal names the plain-name form: {error}"
        );
        assert_eq!(
            error.line(),
            NonZeroU32::new(5),
            "the entry's own line: {error}"
        );
    }
}

#[test]
fn a_plugin_declared_twice_is_refused() {
    for entries in ["[web, mcp, web]", "[web, web]"] {
        let error = parse_err(&prompt(&format!("plugins: {entries}"), &[]));
        assert_eq!(
            error.kind(),
            ParseErrorKind::Frontmatter,
            "{entries}: {error}"
        );
        assert_eq!(
            error.to_string(),
            "invalid frontmatter: Plugin web is declared more than once under \
             plugins",
            "{entries}"
        );
    }
}

#[test]
fn a_list_of_distinct_plugins_still_parses() {
    let prompt = parse_ok(&prompt("plugins: [web, web2, web-search]", &[]));
    assert_eq!(prompt.frontmatter().plugins().len(), 3);
}

#[test]
fn the_plugins_key_takes_plain_names() {
    let prompt = parse_ok(&prompt("plugins: [web, mcp]", &[]));
    let plugins = prompt.frontmatter().plugins();
    assert_eq!(plugins.len(), 2);
    assert_eq!(plugins[0].to_string(), "web");
    assert_eq!(plugins[1].to_string(), "mcp");
}
