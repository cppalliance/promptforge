//! Tests for the Plugin identity vocabulary.

use super::{PluginId, PluginIdErrorKind};
use crate::tools::ToolId;

#[test]
fn plugin_id_requires_exactly_one_segment() {
    let id = PluginId::parse("web").expect("one segment parses");
    assert_eq!(id.to_string(), "web");

    let two = PluginId::parse("promptforge/web")
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
    assert_eq!(upper.kind(), PluginIdErrorKind::Control);

    let versioned = PluginId::parse("web@2").expect_err("v1 is unversioned: '@' is a parse error");
    assert_eq!(versioned.kind(), PluginIdErrorKind::Control);
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
    let json = serde_json::to_string(&id).expect("serializes");
    assert_eq!(json, "\"user-input\"");
    let back: PluginId = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back, id);
    assert!(
        serde_json::from_str::<PluginId>("\"promptforge/user-input\"").is_err(),
        "a 2-segment string is never a Plugin id"
    );
}
