//! Tests for the Plugin identity vocabulary.

use super::{PluginId, PluginIdErrorKind};
use crate::test_support::assert_wire;
use crate::tools::ToolId;

#[test]
fn plugin_id_requires_exactly_one_segment() {
    let id = PluginId::parse("web").expect("one segment parses");
    assert_eq!(id.to_string(), "web");

    let two = PluginId::parse("runvil/web")
        .expect_err("a vendor/name pair is a package name, not a Plugin id");
    assert_eq!(two.kind(), PluginIdErrorKind::SegmentCount);

    let tool = PluginId::parse("web/fetch").expect_err("two segments are a tool id");
    assert_eq!(tool.kind(), PluginIdErrorKind::SegmentCount);
}

#[test]
fn plugin_id_rejects_empty_and_control_segments() {
    let empty = PluginId::parse("").expect_err("an empty id is rejected");
    assert_eq!(empty.kind(), PluginIdErrorKind::Empty);

    let upper = PluginId::parse("Web")
        .expect_err("comparison is case-sensitive: uppercase is outside the charset");
    assert_eq!(upper.kind(), PluginIdErrorKind::Charset);

    let versioned = PluginId::parse("web@2").expect_err("v1 is unversioned: '@' is a parse error");
    assert_eq!(versioned.kind(), PluginIdErrorKind::Charset);

    let accented = PluginId::parse("w\u{e9}b").expect_err("non-ASCII is outside the charset");
    assert_eq!(accented.kind(), PluginIdErrorKind::Charset);

    let control = PluginId::parse("we\tb").expect_err("a control byte is rejected");
    assert_eq!(control.kind(), PluginIdErrorKind::Control);
}

#[test]
fn plugin_id_from_str_parses_like_parse() {
    let parsed: PluginId = "web".parse().expect("one segment parses");
    assert_eq!(parsed, PluginId::parse("web").expect("one segment parses"));
    let error = "web/fetch"
        .parse::<PluginId>()
        .expect_err("two segments are a tool id");
    assert_eq!(error.kind(), PluginIdErrorKind::SegmentCount);
}

#[test]
fn plugin_id_contains_exactly_the_tools_whose_first_segment_it_is() {
    let web = PluginId::parse("web").expect("a static valid id");
    let fetch = ToolId::parse("web/fetch").expect("a static valid id");
    assert!(web.contains(&fetch));
    let deep = ToolId::parse("web/fetch/raw").expect("a static valid id");
    assert!(
        web.contains(&deep),
        "every segment past the first is the Plugin's own"
    );
    let stray = ToolId::parse("other/web").expect("a static valid id");
    assert!(
        !web.contains(&stray),
        "only the first segment names the Plugin"
    );
    // Containment is by identity, not by prefix text: a Plugin whose name
    // merely extends this one is not contained.
    let extended = ToolId::parse("web2/fetch").expect("a static valid id");
    assert!(!web.contains(&extended));
}

#[test]
fn plugin_id_serializes_as_its_string_form() {
    let id = PluginId::parse("user-input").expect("a static valid id");
    assert_wire(&id, "\"user-input\"");
    assert!(
        serde_json::from_str::<PluginId>("\"runvil/user-input\"").is_err(),
        "a 2-segment string is never a Plugin id"
    );
}
