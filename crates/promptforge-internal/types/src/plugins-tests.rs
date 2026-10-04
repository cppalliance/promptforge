//! Tests for the capability identity vocabulary.

use super::{PluginId, PluginIdErrorKind};
use crate::tools::ToolId;

#[test]
fn plugin_id_requires_exactly_two_segments() {
    let id = PluginId::parse("promptforge/web").expect("two segments parse");
    assert_eq!(id.to_string(), "promptforge/web");

    let three = PluginId::parse("promptforge/web/fetch")
        .expect_err("three segments are a tool id, not a Plugin id");
    assert_eq!(three.kind(), PluginIdErrorKind::SegmentCount);

    let one = PluginId::parse("promptforge").expect_err("one segment names nothing");
    assert_eq!(one.kind(), PluginIdErrorKind::SegmentCount);
}

#[test]
fn plugin_id_rejects_empty_and_control_segments() {
    let empty = PluginId::parse("promptforge/").expect_err("an empty segment is rejected");
    assert_eq!(empty.kind(), PluginIdErrorKind::Empty);

    let upper = PluginId::parse("Promptforge/web")
        .expect_err("comparison is case-sensitive: uppercase is outside the charset");
    assert_eq!(upper.kind(), PluginIdErrorKind::Control);

    let versioned =
        PluginId::parse("promptforge/web@2").expect_err("v1 is unversioned: '@' is a parse error");
    assert_eq!(versioned.kind(), PluginIdErrorKind::Control);
}

#[test]
fn plugin_id_exposes_namespace_and_name() {
    let id = PluginId::parse("org.rustalliance/core").expect("a reverse-DNS namespace parses");
    assert_eq!(id.namespace(), "org.rustalliance");
    assert_eq!(id.name(), "core");
}

#[test]
fn plugin_id_contains_exactly_the_tools_under_it() {
    let web = PluginId::parse("promptforge/web").expect("a static valid id");
    let fetch = ToolId::parse("promptforge/web/fetch").expect("a static valid id");
    assert!(web.contains(&fetch));
    let stray = ToolId::parse("promptforge/other/fetch").expect("a static valid id");
    assert!(!web.contains(&stray));
    // Containment is by identity, not by prefix text: a pack whose name
    // merely extends this one is not contained.
    let extended = ToolId::parse("promptforge/web2/fetch").expect("a static valid id");
    assert!(!web.contains(&extended));
}

#[test]
fn plugin_id_serializes_as_its_string_form() {
    let id = PluginId::parse("promptforge/web").expect("a static valid id");
    let json = serde_json::to_string(&id).expect("serializes");
    assert_eq!(json, "\"promptforge/web\"");
    let back: PluginId = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back, id);
    assert!(
        serde_json::from_str::<PluginId>("\"promptforge/web/fetch\"").is_err(),
        "a 3-segment string is a tool id, never a Plugin id"
    );
}
