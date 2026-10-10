//! Tests for the `plugins` key and the retired `tools` key: Plugin names,
//! and the refusal of a leftover tool slot map.

use super::*;

#[test]
fn a_plugin_id_must_be_exactly_one_segment() {
    for id in ["promptforge/web", "web/fetch", "web/", "promptforge//web"] {
        let yaml = format!("name: x\ndescription: d\nplugins:\n  - {id}\n");
        let error = parse(&yaml).expect_err("a bad Plugin id must be rejected");
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{id}: {error}");
    }
    let error = parse("name: x\ndescription: d\nplugins:\n  - promptforge/web\n")
        .expect_err("a vendor/name pair is not a Plugin id");
    assert!(
        error
            .to_string()
            .contains("a Plugin id is one segment, such as `web`"),
        "the refusal states the one-segment rule: {error}"
    );
}

#[test]
fn an_at_sign_in_a_plugin_id_is_rejected() {
    // A Plugin id carries no version, so `@` is a parse error.
    let error = parse("name: x\ndescription: d\nplugins:\n  - web@1\n")
        .expect_err("a `@` version pin must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_plugin_id_with_uppercase_is_rejected() {
    let error = parse("name: x\ndescription: d\nplugins:\n  - Web\n")
        .expect_err("uppercase is outside the segment charset");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_plugin_entry_must_be_a_plain_name() {
    let error = parse("name: x\ndescription: d\nplugins:\n  - 42\n")
        .expect_err("a numeric Plugin entry must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn the_map_form_of_a_plugin_entry_is_refused_naming_the_plain_name_form() {
    for entry in [
        "  - ref: web\n",
        "  - ref: web\n    optional: true\n",
        "  - ref: web\n    config: { depth: 1 }\n",
    ] {
        let error = parse(&format!("name: x\ndescription: d\nplugins:\n{entry}"))
            .expect_err("the map form must be refused");
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
        assert_eq!(error.line(), Some(5), "the entry's own line: {error}");
    }
}

#[test]
fn a_leftover_tools_key_is_refused_at_parse() {
    // Tools are named by canonical id from Lua, so a slot map is an
    // unknown frontmatter key, whatever its entries hold.
    for entries in ["  search: web/search\n", "  open: true\n", "  {}\n"] {
        let error = parse(&format!(
            "name: x\ndescription: d\nplugins:\n  - web\ntools:\n{entries}"
        ))
        .expect_err("a leftover `tools:` key must be refused");
        assert_eq!(
            error.kind(),
            ParseErrorKind::Frontmatter,
            "{entries}: {error}"
        );
        assert!(
            error.to_string().contains("unknown field `tools`"),
            "the refusal names the key: {error}"
        );
    }
}

#[test]
fn a_plugin_declared_twice_is_refused() {
    for entries in ["  - web\n  - mcp\n  - web\n", "  - web\n  - web\n"] {
        let error = parse(&format!("name: x\ndescription: d\nplugins:\n{entries}"))
            .expect_err("a Plugin declared twice must be rejected");
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
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "plugins:\n",
        "  - web\n",
        "  - web2\n",
        "  - web-search\n",
    ))
    .expect("distinct Plugins parse");
    assert_eq!(prompt.frontmatter().plugins().len(), 3);
}

#[test]
fn the_plugins_key_takes_plain_names_and_the_old_key_is_refused() {
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "plugins:\n",
        "  - web\n",
        "  - mcp\n",
    ))
    .expect("plain names parse under `plugins`");
    let plugins = prompt.frontmatter().plugins();
    assert_eq!(plugins.len(), 2);
    assert_eq!(plugins[0].to_string(), "web");
    assert_eq!(plugins[1].to_string(), "mcp");

    let error = parse("name: x\ndescription: d\ncapabilities:\n  - web\n")
        .expect_err("the retired `capabilities` key must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{error}");
    assert!(
        error.to_string().contains("plugins"),
        "the refusal names the `plugins` key: {error}"
    );
}
