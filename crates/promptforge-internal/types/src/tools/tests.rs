//! Tests for `ToolId`, `ToolDescriptor`, `ToolCatalog`, and tool output trust.

use serde_json::json;

use super::{ToolCatalog, ToolCatalogErrorKind, ToolDescriptor, ToolId};
use crate::plugins::PluginId;
use crate::test_support::{assert_send_sync, assert_wire, round_trip};

fn inspect_id() -> ToolId {
    ToolId::parse("fixtures/tools/inspect").expect("fixture id is valid")
}

/// The fixture descriptor: the `inspect` tool as data.
fn inspect_descriptor() -> ToolDescriptor {
    ToolDescriptor::new(
        inspect_id(),
        "Inspect a fixture.",
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }),
    )
}

/// A catalog fixture descriptor under `fixtures/tools/<id_name>`, described
/// as `id_name`.
fn catalog_descriptor(id_name: &str) -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse(&format!("fixtures/tools/{id_name}")).expect("fixture id is valid"),
        id_name,
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
    assert_send_sync::<ToolCatalog>();
}

#[test]
fn tool_error_classifies_and_hides_source() {
    use super::{ToolError, ToolErrorKind};
    assert_send_sync::<ToolError>();

    let plain = ToolError::message("model-safe");
    assert_eq!(plain.kind(), ToolErrorKind::Other);
    assert_eq!(plain.to_string(), "model-safe");
    assert!(!plain.is_cancelled() && !plain.is_retryable());

    let cancelled = ToolError::message("stopped").with_kind(ToolErrorKind::Cancelled);
    assert!(cancelled.is_cancelled());

    let retry = ToolError::message("net").with_kind(ToolErrorKind::Transport);
    assert!(retry.is_retryable());

    let sourced = ToolError::message("wrap").with_source(std::io::Error::other("cause"));
    assert!(std::error::Error::source(&sourced).is_some());
    assert!(
        !sourced.to_string().contains("cause"),
        "Display must not expose the tool error source: {sourced}"
    );
}

#[test]
fn a_tool_error_takes_its_kind_at_construction_and_a_source_as_a_builder() {
    use super::{ToolError, ToolErrorKind};
    let err = ToolError::new(ToolErrorKind::Backend, "backend failed")
        .with_source(std::io::Error::other("boom"));
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert_eq!(err.to_string(), "backend failed");
    assert!(std::error::Error::source(&err).is_some());

    let kept = ToolError::new(ToolErrorKind::Transport, "net down")
        .with_source(std::io::Error::other("reset"));
    assert_eq!(
        kept.kind(),
        ToolErrorKind::Transport,
        "attaching a source keeps the kind the error was built with"
    );
}

#[test]
fn an_empty_catalog_builds_and_holds_no_tools() {
    let catalog = ToolCatalog::new([]).expect("an empty catalog builds");
    assert!(catalog.tools().is_empty());
}

#[test]
fn a_descriptor_has_the_tools_surface_and_round_trips_through_serde() {
    // The descriptor is the tool as data: identity, description, schema,
    // and the output kind, so a catalog built from descriptors holds no
    // implementation and round-trips through serde.
    let descriptor = inspect_descriptor();
    assert_eq!(descriptor.id, inspect_id());
    assert_eq!(descriptor.description, "Inspect a fixture.");
    assert_eq!(descriptor.parameters_schema["required"], json!(["path"]));
    assert!(
        !descriptor.structured_output,
        "a descriptor that does not declare structured output stays plain text"
    );
    assert_eq!(round_trip(&descriptor), descriptor);
}

#[test]
fn a_descriptor_logged_without_survives_stop_reads_as_not_surviving() {
    let wire = r#"{"id":"web/fetch","wire_name":"fetch","description":"d",
        "parameters_schema":{"type":"object"},"structured_output":false}"#;
    let descriptor: ToolDescriptor =
        serde_json::from_str(wire).expect("a descriptor from before the flag deserializes");
    assert!(
        !descriptor.survives_stop,
        "an absent flag reads as false, so a stop still reaches the call"
    );
}

#[test]
fn survives_stop_sets_the_flag_and_round_trips_through_serde() {
    assert!(
        !inspect_descriptor().survives_stop,
        "a new descriptor does not survive a stop"
    );
    let descriptor = inspect_descriptor().survives_stop(true);
    assert!(descriptor.survives_stop);
    assert_eq!(round_trip(&descriptor), descriptor);
}

#[test]
fn a_tool_call_origin_keeps_its_logged_shape() {
    use super::{ToolCallOrigin, ToolCaller};
    let origin = ToolCallOrigin::new("run-1", "Chat", ToolCaller::Model);
    let line = r#"{"execution":"run-1","section":"Chat","caller":"model"}"#;
    assert_wire(&origin, line);
}

#[test]
fn catalog_lookup_matches_the_whole_tool_id() {
    let catalog = ToolCatalog::new([inspect_descriptor()]).expect("unique catalog");

    let found = catalog
        .get(&inspect_id())
        .expect("the stable identity should resolve");
    assert_eq!(found.description, "Inspect a fixture.");
    assert!(
        catalog
            .get(&ToolId::parse("other/inspect").expect("valid id"))
            .is_none(),
        "a tool's last segment alone must not resolve it"
    );
}

#[test]
fn catalog_preserves_order_and_first_match_lookup() {
    let catalog = ToolCatalog::new([
        catalog_descriptor("inspect"),
        catalog_descriptor("summarize"),
    ])
    .expect("distinct identities build a catalog");

    assert_eq!(
        catalog
            .tools()
            .iter()
            .map(|tool| tool.id.to_string())
            .collect::<Vec<_>>(),
        ["fixtures/tools/inspect", "fixtures/tools/summarize"]
    );
    assert_eq!(catalog.tools().len(), 2);
    assert_eq!(
        catalog
            .get(&inspect_id())
            .expect("the identity should resolve")
            .description,
        "inspect",
    );
}

#[test]
fn catalog_rejects_duplicate_tool_ids() {
    let error = ToolCatalog::new([catalog_descriptor("inspect"), catalog_descriptor("inspect")])
        .expect_err("a repeated tool identity must be rejected at catalog construction");
    assert_eq!(error.kind(), ToolCatalogErrorKind::DuplicateId);
    assert_eq!(
        error.duplicate_id(),
        Some(&inspect_id()),
        "the error must name the duplicated identity"
    );
    assert_eq!(
        error.to_string(),
        "duplicate tool identity `fixtures/tools/inspect` in the tool catalog"
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
fn an_uppercase_segment_is_rejected_as_a_charset_error_because_comparison_is_case_sensitive() {
    use super::ToolIdErrorKind;
    assert_eq!(tool_id_error_kind("Web/fetch"), ToolIdErrorKind::Charset);
}

#[test]
fn an_at_sign_or_non_ascii_segment_is_rejected_as_a_charset_error() {
    use super::ToolIdErrorKind;
    assert_eq!(tool_id_error_kind("web@2/fetch"), ToolIdErrorKind::Charset);
    assert_eq!(
        tool_id_error_kind("web/f\u{e9}tch"),
        ToolIdErrorKind::Charset
    );
}

#[test]
fn a_tool_id_from_str_parses_like_parse() {
    use super::ToolIdErrorKind;
    let parsed: ToolId = "web/fetch".parse().expect("a valid tool id parses");
    assert_eq!(parsed, ToolId::parse("web/fetch").expect("a valid tool id"));
    let error = "web"
        .parse::<ToolId>()
        .expect_err("one segment is rejected");
    assert_eq!(error.kind(), ToolIdErrorKind::SegmentCount);
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
    assert_wire(&id, "\"web/fetch\"");
}

#[test]
fn deserializing_an_invalid_tool_id_is_a_data_error() {
    assert!(serde_json::from_str::<ToolId>("\"web_fetch\"").is_err());
    assert!(serde_json::from_str::<ToolId>("\"web//fetch\"").is_err());
}
