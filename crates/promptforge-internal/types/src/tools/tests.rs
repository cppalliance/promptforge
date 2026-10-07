//! Tests for `ToolId`, `ToolDescriptor`, `ToolCatalog`, and tool output trust.

use serde_json::json;

use super::{ToolCatalog, ToolCatalogErrorKind, ToolDescriptor, ToolId};
use crate::plugins::PluginId;

fn inspect_id() -> ToolId {
    ToolId::parse("fixtures/tools/inspect").expect("fixture id is valid")
}

/// The fixture descriptor: the `inspect` tool as data.
fn inspect_descriptor() -> ToolDescriptor {
    ToolDescriptor::new(
        inspect_id(),
        "inspect_wire",
        "Inspect a fixture.",
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }),
    )
}

/// A catalog fixture descriptor under `fixtures/tools/<id_name>` advertised
/// as `wire_name`.
fn catalog_descriptor(id_name: &str, wire_name: &str) -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse(&format!("fixtures/tools/{id_name}")).expect("fixture id is valid"),
        wire_name,
        wire_name,
        json!({"type": "object"}),
    )
}

#[test]
fn tool_output_has_mandatory_trust() {
    use super::{OutputTrust, ToolOutput};
    assert_eq!(ToolOutput::trusted("a").trust(), OutputTrust::Trusted);
    assert_eq!(ToolOutput::untrusted("b").trust(), OutputTrust::Untrusted);
    assert_eq!(ToolOutput::trusted("a").text(), "a");
}

#[test]
fn tool_catalog_is_send_and_sync() {
    // The public catalog must stay `Send + Sync` so downstream callers can
    // share it across tasks; a representation change that dropped either
    // auto trait would fail to compile here.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ToolCatalog>();
}

#[test]
fn tool_error_classifies_and_hides_source() {
    use super::{ToolError, ToolErrorKind};
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<ToolError>();

    let plain = ToolError::message("model-safe");
    assert_eq!(plain.kind(), ToolErrorKind::Other);
    assert_eq!(plain.to_string(), "model-safe");
    assert!(!plain.is_cancelled() && !plain.is_retryable());

    let cancelled = ToolError::message("stopped").with_kind(ToolErrorKind::Cancelled);
    assert!(cancelled.is_cancelled());

    let retry = ToolError::message("net").with_kind(ToolErrorKind::Transport);
    assert!(retry.is_retryable());

    let sourced = ToolError::with_source("wrap", std::io::Error::other("cause"));
    assert!(std::error::Error::source(&sourced).is_some());
    assert!(
        !sourced.to_string().contains("cause"),
        "Display must not expose the tool error source: {sourced}"
    );
}

#[test]
fn a_tool_error_built_with_a_source_is_a_backend_error() {
    use super::{ToolError, ToolErrorKind};
    let err = ToolError::with_source("backend failed", std::io::Error::other("boom"));
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert!(std::error::Error::source(&err).is_some());
}

#[test]
fn an_empty_catalog_builds_and_holds_no_tools() {
    let catalog = ToolCatalog::new(&[]).expect("an empty catalog builds");
    assert!(catalog.tools().is_empty());
}

#[test]
fn a_descriptor_has_the_tools_surface_and_round_trips_through_serde() {
    // The descriptor is the tool as data: identity, wire name, description,
    // schema, and the output kind, so a catalog built from descriptors holds
    // no implementation and round-trips through serde.
    let descriptor = inspect_descriptor();
    assert_eq!(descriptor.id, inspect_id());
    assert_eq!(descriptor.wire_name, "inspect_wire");
    assert_eq!(descriptor.description, "Inspect a fixture.");
    assert_eq!(descriptor.parameters_schema["required"], json!(["path"]));
    assert!(
        !descriptor.structured_output,
        "a descriptor that does not declare structured output stays plain text"
    );
    let wire = serde_json::to_string(&descriptor).expect("the descriptor serializes");
    let back: ToolDescriptor = serde_json::from_str(&wire).expect("the descriptor deserializes");
    assert_eq!(back, descriptor);
}

#[test]
fn catalog_lookup_uses_stable_identity_not_wire_name() {
    let catalog = ToolCatalog::new(&[inspect_descriptor()]).expect("unique catalog");

    let found = catalog
        .get(&inspect_id())
        .expect("the stable identity should resolve");
    assert_eq!(found.wire_name, "inspect_wire");
    assert!(
        catalog
            .get(&ToolId::parse("fixtures/tools/inspect_wire").expect("valid id"))
            .is_none(),
        "the transport name must not become identity"
    );
}

#[test]
fn catalog_preserves_order_and_first_match_lookup() {
    let catalog = ToolCatalog::new(&[
        catalog_descriptor("inspect", "first_inspect"),
        catalog_descriptor("summarize", "summarize"),
    ])
    .expect("distinct identities build a catalog");

    assert_eq!(
        catalog
            .tools()
            .iter()
            .map(|tool| tool.wire_name.as_str())
            .collect::<Vec<_>>(),
        ["first_inspect", "summarize"]
    );
    assert_eq!(catalog.tools().len(), 2);
    assert_eq!(
        catalog
            .get(&inspect_id())
            .expect("the identity should resolve")
            .wire_name,
        "first_inspect",
    );
}

#[test]
fn catalog_rejects_duplicate_tool_ids() {
    let error = ToolCatalog::new(&[
        catalog_descriptor("inspect", "first_inspect"),
        catalog_descriptor("inspect", "second_inspect"),
    ])
    .expect_err("a repeated tool identity must be rejected at catalog construction");
    assert_eq!(error.kind(), ToolCatalogErrorKind::DuplicateId);
    assert_eq!(
        error.duplicate_id(),
        Some(&inspect_id()),
        "the error must name the duplicated identity"
    );
}

fn tool_id_error_kind(input: &str) -> super::ToolIdErrorKind {
    ToolId::parse(input)
        .expect_err("the input must be rejected")
        .kind()
}

#[test]
fn a_two_segment_tool_id_parses_and_exposes_its_name() {
    let id = ToolId::parse("web/fetch").expect("a valid tool id");
    assert_eq!(id.name(), "fetch");
}

#[test]
fn a_tool_ids_plugin_is_its_first_segment() {
    let id = ToolId::parse("web/fetch").expect("a valid tool id");
    assert_eq!(
        id.plugin(),
        PluginId::parse("web").expect("a valid Plugin id"),
        "the first segment must yield the contributing Plugin's local name"
    );
}

#[test]
fn a_deeper_tool_id_names_its_plugin_first_and_its_tool_last() {
    let id = ToolId::parse("github/issues/create").expect("a valid tool id");
    assert_eq!(id.name(), "create");
    assert_eq!(
        id.plugin(),
        PluginId::parse("github").expect("a valid Plugin id")
    );
}

#[test]
fn four_segments_parse_as_a_tool_id() {
    let id = ToolId::parse("github/issues/comments/create").expect("a valid tool id");
    assert_eq!(id.name(), "create");
    assert_eq!(id.to_string(), "github/issues/comments/create");
}

#[test]
fn a_single_segment_is_rejected_as_a_tool_id() {
    use super::ToolIdErrorKind;
    assert_eq!(tool_id_error_kind("web"), ToolIdErrorKind::SegmentCount);
}

#[test]
fn an_empty_tool_id_is_rejected_as_an_empty_error() {
    use super::ToolIdErrorKind;
    assert_eq!(tool_id_error_kind(""), ToolIdErrorKind::Empty);
}

#[test]
fn an_empty_segment_is_rejected_as_an_empty_error() {
    use super::ToolIdErrorKind;
    assert_eq!(tool_id_error_kind("web//fetch"), ToolIdErrorKind::Empty);
}

#[test]
fn a_control_character_is_rejected_as_a_control_error() {
    use super::ToolIdErrorKind;
    assert_eq!(tool_id_error_kind("we\tb/fetch"), ToolIdErrorKind::Control);
    assert_eq!(
        tool_id_error_kind("web/fe\u{7f}tch"),
        ToolIdErrorKind::Control
    );
}

#[test]
fn an_uppercase_segment_is_rejected_because_comparison_is_case_sensitive() {
    use super::ToolIdErrorKind;
    assert_eq!(tool_id_error_kind("Web/fetch"), ToolIdErrorKind::Control);
}

#[test]
fn the_migrated_built_in_ids_parse() {
    assert!(ToolId::parse("web/fetch").is_ok());
    assert!(ToolId::parse("web/search").is_ok());
    assert!(ToolId::parse("user-input/ask").is_ok());
}

#[test]
fn a_tool_id_serializes_as_its_global_name_string() {
    let id = ToolId::parse("web/fetch").expect("a valid tool id");
    assert_eq!(
        serde_json::to_string(&id).expect("serialize"),
        "\"web/fetch\""
    );
    let parsed: ToolId = serde_json::from_str("\"web/fetch\"").expect("deserialize");
    assert_eq!(parsed, id);
}

#[test]
fn deserializing_an_invalid_tool_id_is_a_data_error() {
    assert!(serde_json::from_str::<ToolId>("\"web_fetch\"").is_err());
    assert!(serde_json::from_str::<ToolId>("\"web//fetch\"").is_err());
}

#[test]
fn catalog_rejects_illegal_wire_name() {
    let bad = ToolDescriptor::new(
        ToolId::parse("fixtures/tools/bad_wire").expect("valid id"),
        "bad/name",
        "bad",
        json!({"type": "object"}),
    );
    let error = ToolCatalog::new(&[bad])
        .expect_err("an illegal wire name must be rejected at catalog construction");
    assert_eq!(error.kind(), ToolCatalogErrorKind::InvalidWireName);
    assert!(error.duplicate_id().is_none());
}
