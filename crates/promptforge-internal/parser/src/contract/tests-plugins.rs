//! Tests for the `plugins` and `tools` keys: Plugin ids, tool slot
//! paths and aliases, and the slots an optional Plugin cannot back.

use super::*;

#[test]
fn a_plugin_id_must_have_exactly_two_segments() {
    for id in ["web", "promptforge/web/fetch", "promptforge//web"] {
        let yaml = format!("name: x\ndescription: d\nplugins:\n  - {id}\n");
        let error = parse(&yaml).expect_err("a bad Plugin id must be rejected");
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{id}: {error}");
    }
}

#[test]
fn an_at_sign_in_a_plugin_id_is_rejected() {
    // A Plugin id carries no version, so `@` is a parse error.
    let error = parse("name: x\ndescription: d\nplugins:\n  - promptforge/web@1\n")
        .expect_err("a `@` version pin must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_plugin_id_with_uppercase_is_rejected() {
    let error = parse("name: x\ndescription: d\nplugins:\n  - Promptforge/web\n")
        .expect_err("uppercase is outside the segment charset");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_plugin_is_required_unless_flagged_optional() {
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "plugins:\n",
        "  - promptforge/web\n",
        "  - ref: io.github.corp/mcp\n",
        "    optional: true\n",
    ))
    .expect("Plugin entries must parse");
    let caps = prompt.frontmatter().plugins();
    assert_eq!(caps.len(), 2);
    assert!(!caps[0].is_optional(), "a plain string entry is required");
    assert!(caps[1].is_optional());
}

#[test]
fn a_plugin_entry_must_be_a_string_or_a_ref_map() {
    let error = parse("name: x\ndescription: d\nplugins:\n  - 42\n")
        .expect_err("a numeric Plugin entry must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
}

#[test]
fn a_map_valued_tool_slot_is_rejected_naming_the_exact_path_expectation() {
    // Exact paths are the only slot form, so a map-valued slot fails to
    // parse.
    let error = parse(concat!(
        "name: x\ndescription: d\n",
        "tools:\n",
        "  wiki:\n",
        "    want: searches private wikis\n",
        "    optional: true\n",
    ))
    .expect_err("a map-valued tool slot must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{error}");
    assert!(
        error.to_string().contains("an exact tool path string"),
        "the error must name the exact-path expectation: {error}"
    );
}

#[test]
fn a_malformed_exact_tool_path_is_a_parse_error() {
    for path in [
        "promptforge/web",
        "web",
        "promptforge/Web/fetch",
        "promptforge/web/",
    ] {
        let yaml = format!("name: x\ndescription: d\ntools:\n  search: {path}\n");
        let error = parse(&yaml).expect_err("a malformed exact path must be rejected");
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{path}: {error}");
    }
}

#[test]
fn the_reserved_open_tool_slot_key_is_rejected() {
    // `open` is reserved even though it satisfies the alias grammar.
    let error = parse("name: x\ndescription: d\ntools:\n  open: true\n")
        .expect_err("the reserved `open` key must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
    assert!(
        error.to_string().contains("reserved"),
        "the error must name the reservation: {error}"
    );
}

#[test]
fn tool_slot_aliases_must_match_the_alias_grammar() {
    for alias in ["1search", "has space", "has/slash", "has.dot"] {
        let yaml =
            format!("name: x\ndescription: d\ntools:\n  '{alias}': promptforge/web/search\n");
        let error = parse(&yaml).expect_err("a bad alias must be rejected");
        assert_eq!(
            error.kind(),
            ParseErrorKind::Frontmatter,
            "{alias}: {error}"
        );
    }
    // The length boundary: 64 characters pass, 65 fail.
    let longest_ok = format!("a{}", "b".repeat(63));
    let too_long = format!("a{}", "b".repeat(64));
    let yaml = format!("name: x\ndescription: d\ntools:\n  {longest_ok}: promptforge/web/search\n");
    parse(&yaml).expect("a 64-character alias must parse");
    let yaml = format!("name: x\ndescription: d\ntools:\n  {too_long}: promptforge/web/search\n");
    parse(&yaml).expect_err("a 65-character alias must be rejected");
}

#[test]
fn a_tool_slot_backed_by_an_optional_plugin_is_refused() {
    let error = parse(concat!(
        "name: x\ndescription: d\n",
        "plugins:\n",
        "  - promptforge/web\n",
        "  - ref: io.github.corp/mcp\n",
        "    optional: true\n",
        "tools:\n",
        "  search: promptforge/web/search\n",
        "  probe: io.github.corp/mcp/probe\n",
        "  lookup: io.github.corp/mcp/lookup\n",
    ))
    .expect_err("a slot on an optional Plugin must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{error}");
    assert_eq!(
        error.to_string(),
        "invalid frontmatter: tool alias 'lookup' names io.github.corp/mcp/lookup, whose \
         Plugin io.github.corp/mcp is declared optional; a tool slot requires its Plugin",
        "the refusal names the first offending alias in sorted order"
    );
    assert_eq!(error.line(), None, "the refusal spans two keys: {error}");
}

#[test]
fn an_optional_plugin_without_a_tool_slot_still_parses() {
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "plugins:\n",
        "  - ref: promptforge/web\n",
        "    optional: false\n",
        "  - ref: io.github.corp/mcp\n",
        "    optional: true\n",
        "tools:\n",
        "  search: promptforge/web/search\n",
    ))
    .expect("an optional Plugin that backs no slot parses");
    let caps = prompt.frontmatter().plugins();
    assert_eq!(caps.len(), 2);
    assert!(caps[1].is_optional());
    assert!(prompt.frontmatter().tools().get("search").is_some());
}

#[test]
fn a_slot_on_a_required_plugin_parses_beside_optional_ones_sharing_one_segment() {
    // Each optional Plugin matches the slot's Plugin in exactly one
    // segment, so a check comparing only the namespace or only the Plugin
    // segment would refuse this prompt.
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "plugins:\n",
        "  - promptforge/web\n",
        "  - ref: io.github.corp/web\n",
        "    optional: true\n",
        "  - ref: promptforge/mcp\n",
        "    optional: true\n",
        "tools:\n",
        "  search: promptforge/web/search\n",
    ))
    .expect("a slot whose Plugin is required parses");
    assert_eq!(prompt.frontmatter().plugins().len(), 3);
    assert!(prompt.frontmatter().tools().get("search").is_some());
}

#[test]
fn a_plugin_declared_twice_is_refused_whatever_the_entry_forms() {
    for entries in [
        "  - promptforge/web\n  - io.github.corp/mcp\n  - promptforge/web\n",
        "  - ref: promptforge/web\n    optional: true\n  - ref: promptforge/web\n    optional: true\n",
        "  - promptforge/web\n  - ref: promptforge/web\n    optional: true\n",
        "  - ref: promptforge/web\n    config: { depth: 1 }\n  - ref: promptforge/web\n    config: { depth: 2 }\n",
    ] {
        let error = parse(&format!("name: x\ndescription: d\nplugins:\n{entries}"))
            .expect_err("a Plugin declared twice must be rejected");
        assert_eq!(
            error.kind(),
            ParseErrorKind::Frontmatter,
            "{entries}: {error}"
        );
        assert_eq!(
            error.to_string(),
            "invalid frontmatter: Plugin promptforge/web is declared more than once under \
             plugins",
            "{entries}"
        );
    }
}

#[test]
fn a_duplicate_plugin_that_also_backs_a_slot_reports_the_duplicate() {
    let error = parse(concat!(
        "name: x\ndescription: d\n",
        "plugins:\n",
        "  - ref: promptforge/web\n",
        "    optional: true\n",
        "  - ref: promptforge/web\n",
        "    optional: true\n",
        "tools:\n",
        "  fetch: promptforge/web/fetch\n",
    ))
    .expect_err("a duplicate Plugin must be rejected");
    assert_eq!(
        error.to_string(),
        "invalid frontmatter: Plugin promptforge/web is declared more than once under \
         plugins"
    );
}

#[test]
fn a_list_of_distinct_plugins_still_parses() {
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "plugins:\n",
        "  - promptforge/web\n",
        "  - promptforge/web2\n",
        "  - ref: io.github.corp/web\n",
        "    optional: true\n",
    ))
    .expect("distinct Plugins parse");
    assert_eq!(prompt.frontmatter().plugins().len(), 3);
}

#[test]
fn the_plugins_key_takes_both_entry_forms_and_the_old_key_is_refused() {
    let prompt = parse(concat!(
        "name: x\ndescription: d\n",
        "plugins:\n",
        "  - promptforge/web\n",
        "  - ref: io.github.corp/mcp\n",
        "    optional: true\n",
        "    config: { depth: 1 }\n",
    ))
    .expect("both entry forms parse under `plugins`");
    let plugins = prompt.frontmatter().plugins();
    assert_eq!(plugins.len(), 2);
    assert_eq!(plugins[0].id().to_string(), "promptforge/web");
    assert!(
        !plugins[0].is_optional(),
        "a plain string entry is required"
    );
    assert!(plugins[0].config().is_none());
    assert_eq!(plugins[1].id().to_string(), "io.github.corp/mcp");
    assert!(plugins[1].is_optional());
    assert!(
        plugins[1].config().is_some(),
        "the map form keeps its config"
    );

    let error = parse("name: x\ndescription: d\ncapabilities:\n  - promptforge/web\n")
        .expect_err("the retired `capabilities` key must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter, "{error}");
    assert!(
        error.to_string().contains("plugins"),
        "the refusal names the `plugins` key: {error}"
    );
}
