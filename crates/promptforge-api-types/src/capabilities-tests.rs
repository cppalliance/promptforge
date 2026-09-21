//! Tests for the capability identity vocabulary.

use super::{CapabilityId, CapabilityIdErrorKind};
use crate::tools::ToolId;

#[test]
fn capability_id_requires_exactly_two_segments() {
    let id = CapabilityId::parse("promptforge/web").expect("two segments parse");
    assert_eq!(id.to_string(), "promptforge/web");

    let three = CapabilityId::parse("promptforge/web/fetch")
        .expect_err("three segments are a tool id, not a capability id");
    assert_eq!(three.kind(), CapabilityIdErrorKind::SegmentCount);

    let one = CapabilityId::parse("promptforge").expect_err("one segment names nothing");
    assert_eq!(one.kind(), CapabilityIdErrorKind::SegmentCount);
}

#[test]
fn capability_id_rejects_empty_and_control_segments() {
    let empty = CapabilityId::parse("promptforge/").expect_err("an empty segment is rejected");
    assert_eq!(empty.kind(), CapabilityIdErrorKind::Empty);

    let upper = CapabilityId::parse("Promptforge/web")
        .expect_err("comparison is case-sensitive: uppercase is outside the charset");
    assert_eq!(upper.kind(), CapabilityIdErrorKind::Control);

    let versioned = CapabilityId::parse("promptforge/web@2")
        .expect_err("v1 is unversioned: '@' is a parse error");
    assert_eq!(versioned.kind(), CapabilityIdErrorKind::Control);
}

#[test]
fn capability_id_exposes_namespace_and_pack() {
    let id = CapabilityId::parse("org.rustalliance/core").expect("a reverse-DNS namespace parses");
    assert_eq!(id.namespace(), "org.rustalliance");
    assert_eq!(id.pack(), "core");
}

#[test]
fn capability_id_contains_exactly_the_tools_under_it() {
    let web = CapabilityId::parse("promptforge/web").expect("a static valid id");
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
fn capability_id_serializes_as_its_string_form() {
    let id = CapabilityId::parse("promptforge/web").expect("a static valid id");
    let json = serde_json::to_string(&id).expect("serializes");
    assert_eq!(json, "\"promptforge/web\"");
    let back: CapabilityId = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back, id);
    assert!(
        serde_json::from_str::<CapabilityId>("\"promptforge/web/fetch\"").is_err(),
        "a 3-segment string is a tool id, never a capability id"
    );
}

#[test]
fn capability_id_implements_from_str() {
    use std::str::FromStr;
    let id = CapabilityId::from_str("promptforge/web").expect("valid capability id");
    assert_eq!(id.to_string(), "promptforge/web");
    assert_eq!(
        "promptforge/web"
            .parse::<CapabilityId>()
            .expect("valid capability id")
            .to_string(),
        "promptforge/web"
    );
    assert!(CapabilityId::from_str("promptforge/web/fetch").is_err());
}
