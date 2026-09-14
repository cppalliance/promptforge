use std::sync::Arc;

use serde_json::{Value, json};

use super::{Tool, ToolCatalog, ToolCatalogErrorKind, ToolError, ToolId, ToolOutput};
use crate::names::GlobalName;

fn inspect_id() -> ToolId {
    ToolId::parse("fixtures/tools/inspect").expect("fixture id is valid")
}

struct FixtureTool;

#[async_trait::async_trait]
impl Tool for FixtureTool {
    fn id(&self) -> ToolId {
        inspect_id()
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn wire_name(&self) -> &str {
        "inspect_wire"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "Inspect a fixture."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    }

    async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted(String::new()))
    }
}

struct CatalogFixtureTool {
    id_name: &'static str,
    wire_name: &'static str,
}

#[async_trait::async_trait]
impl Tool for CatalogFixtureTool {
    fn id(&self) -> ToolId {
        ToolId::parse(&format!("fixtures/tools/{}", self.id_name)).expect("fixture id is valid")
    }

    fn wire_name(&self) -> &str {
        self.wire_name
    }

    fn description(&self) -> &str {
        self.wire_name
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted(String::new()))
    }
}

#[test]
fn trait_is_dyn_compatible() {
    let tools: Vec<Box<dyn Tool>> = Vec::new();
    assert!(tools.is_empty());
}

#[test]
fn tool_output_carries_mandatory_trust() {
    use super::{OutputTrust, ToolOutput};
    assert_eq!(ToolOutput::trusted("a").trust(), OutputTrust::Trusted);
    assert_eq!(ToolOutput::untrusted("b").trust(), OutputTrust::Untrusted);
    assert_eq!(ToolOutput::trusted("a").text(), "a");
}

#[test]
fn tool_catalog_is_send_and_sync() {
    // The public dyn-bearing catalog must stay `Send + Sync` so downstream
    // callers can share it across tasks; a representation change that dropped
    // either auto trait would fail to compile here (tools.rs F6).
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
fn descriptor_surface_preserves_identity_description_and_schema() {
    let tool = FixtureTool;

    assert_eq!(tool.id(), inspect_id());
    assert_eq!(tool.wire_name(), "inspect_wire");
    assert_eq!(tool.description(), "Inspect a fixture.");
    assert_eq!(
        tool.parameters_schema(),
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    );
}

#[test]
fn structured_output_defaults_to_plain_text() {
    // Every existing implementation predates the method, so the default
    // must be plain text; a structured tool opts in explicitly.
    let tool = FixtureTool;
    assert!(
        !tool.structured_output(),
        "a tool that does not declare structured output stays plain text"
    );
}

#[test]
fn catalog_lookup_uses_stable_identity_not_wire_name() {
    let tool: Arc<dyn Tool> = Arc::new(FixtureTool);
    let catalog = ToolCatalog::new(std::slice::from_ref(&tool)).expect("unique catalog");

    let found = catalog
        .get(&inspect_id())
        .expect("the stable identity should resolve");
    assert_eq!(found.wire_name(), "inspect_wire");
    assert!(
        catalog
            .get(&ToolId::parse("fixtures/tools/inspect_wire").expect("valid id"))
            .is_none(),
        "the transport name must not become identity"
    );
}

#[test]
fn catalog_preserves_order_and_first_match_lookup() {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(CatalogFixtureTool {
            id_name: "inspect",
            wire_name: "first_inspect",
        }),
        Arc::new(CatalogFixtureTool {
            id_name: "summarize",
            wire_name: "summarize",
        }),
    ];
    let catalog = ToolCatalog::new(&tools).expect("distinct identities build a catalog");

    assert_eq!(
        catalog
            .tools()
            .iter()
            .map(|tool| tool.wire_name())
            .collect::<Vec<_>>(),
        ["first_inspect", "summarize"]
    );
    assert_eq!(catalog.tools().len(), 2);
    assert_eq!(
        catalog
            .get(&inspect_id())
            .expect("the identity should resolve")
            .wire_name(),
        "first_inspect",
    );
}

#[test]
fn catalog_rejects_duplicate_tool_ids() {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(CatalogFixtureTool {
            id_name: "inspect",
            wire_name: "first_inspect",
        }),
        Arc::new(CatalogFixtureTool {
            id_name: "inspect",
            wire_name: "second_inspect",
        }),
    ];
    let error = ToolCatalog::new(&tools)
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
fn a_three_segment_tool_id_parses_and_exposes_its_name() {
    let id = ToolId::parse("promptforge/web/fetch").expect("a valid tool id");
    assert_eq!(id.name(), "fetch");
}

#[test]
fn a_tool_ids_capability_is_always_its_two_segment_prefix() {
    let id = ToolId::parse("promptforge/web/fetch").expect("a valid tool id");
    assert_eq!(
        id.capability(),
        GlobalName::parse("promptforge/web").expect("a valid capability name"),
        "dropping the last segment must yield the contributing capability's id"
    );
}

#[test]
fn containment_holds_for_a_reverse_dns_namespace() {
    let id = ToolId::parse("org.rustalliance/core/search").expect("a valid tool id");
    assert_eq!(id.name(), "search");
    assert_eq!(id.capability().to_string(), "org.rustalliance/core");
}

#[test]
fn a_two_segment_capability_name_is_rejected_as_a_tool_id() {
    use super::ToolIdErrorKind;
    assert_eq!(
        tool_id_error_kind("promptforge/web"),
        ToolIdErrorKind::SegmentCount
    );
}

#[test]
fn a_single_segment_is_rejected_as_a_tool_id() {
    use super::ToolIdErrorKind;
    assert_eq!(
        tool_id_error_kind("promptforge"),
        ToolIdErrorKind::SegmentCount
    );
}

#[test]
fn four_segments_are_rejected_as_a_tool_id() {
    use super::ToolIdErrorKind;
    assert_eq!(
        tool_id_error_kind("promptforge/web/fetch/extra"),
        ToolIdErrorKind::SegmentCount
    );
}

#[test]
fn an_empty_segment_is_rejected_as_an_empty_error() {
    use super::ToolIdErrorKind;
    assert_eq!(
        tool_id_error_kind("promptforge//fetch"),
        ToolIdErrorKind::Empty
    );
}

#[test]
fn a_control_character_is_rejected_as_a_control_error() {
    use super::ToolIdErrorKind;
    assert_eq!(
        tool_id_error_kind("promptforge/we\tb/fetch"),
        ToolIdErrorKind::Control
    );
    assert_eq!(
        tool_id_error_kind("promptforge/web/fe\u{7f}tch"),
        ToolIdErrorKind::Control
    );
}

#[test]
fn an_uppercase_segment_is_rejected_because_comparison_is_case_sensitive() {
    use super::ToolIdErrorKind;
    assert_eq!(
        tool_id_error_kind("Promptforge/web/fetch"),
        ToolIdErrorKind::Control
    );
}

#[test]
fn from_validated_builds_a_static_id_without_revalidating() {
    let id = ToolId::from_validated("promptforge/web/search");
    assert_eq!(id.name(), "search");
    assert_eq!(id.capability().to_string(), "promptforge/web");
}

#[test]
fn the_migrated_built_in_ids_parse() {
    // The built-ins moved from 2-part server/name onto the global grammar:
    // promptforge/web_fetch -> promptforge/web/fetch and
    // promptforge/web_search -> promptforge/web/search.
    assert!(ToolId::parse("promptforge/web/fetch").is_ok());
    assert!(ToolId::parse("promptforge/web/search").is_ok());
}

#[test]
fn a_tool_id_serializes_as_its_global_name_string() {
    let id = ToolId::parse("promptforge/web/fetch").expect("a valid tool id");
    assert_eq!(
        serde_json::to_string(&id).expect("serialize"),
        "\"promptforge/web/fetch\""
    );
    let parsed: ToolId = serde_json::from_str("\"promptforge/web/fetch\"").expect("deserialize");
    assert_eq!(parsed, id);
}

#[test]
fn deserializing_an_invalid_tool_id_is_a_data_error() {
    assert!(serde_json::from_str::<ToolId>("\"promptforge/web_fetch\"").is_err());
    assert!(serde_json::from_str::<ToolId>("\"promptforge/web/fetch/extra\"").is_err());
}

#[test]
fn catalog_rejects_illegal_wire_name() {
    struct BadWire;

    #[async_trait::async_trait]
    impl Tool for BadWire {
        fn id(&self) -> ToolId {
            ToolId::parse("fixtures/tools/bad_wire").expect("valid id")
        }
        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn wire_name(&self) -> &str {
            "bad/name"
        }
        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn description(&self) -> &str {
            "bad"
        }
        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::trusted(String::new()))
        }
    }

    let bad: Arc<dyn Tool> = Arc::new(BadWire);
    let error = ToolCatalog::new(std::slice::from_ref(&bad))
        .expect_err("an illegal wire name must be rejected at catalog construction");
    assert_eq!(error.kind(), ToolCatalogErrorKind::InvalidWireName);
    assert!(error.duplicate_id().is_none());
}
