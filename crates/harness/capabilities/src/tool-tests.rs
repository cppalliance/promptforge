//! Tests for the `Tool` trait: dyn-compatibility, the descriptor
//! derivation, and catalog assembly from described implementations.

use std::sync::Arc;

use promptforge_api_types::tools::{
    ToolCatalog, ToolCatalogErrorKind, ToolDescriptor, ToolError, ToolId, ToolOutput,
};
use serde_json::{Value, json};

use super::Tool;

/// Describes every fixture implementation in `tools`, in order, the way
/// `activation::assemble` does per tool when it builds a run's catalog.
fn describe_all(tools: &[Arc<dyn Tool>]) -> Vec<ToolDescriptor> {
    tools.iter().map(|tool| tool.descriptor()).collect()
}

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
fn a_descriptor_carries_the_tools_surface_and_never_the_implementation() {
    // The descriptor is the tool as data: identity, wire name, description,
    // schema, and the output kind, so a catalog built from descriptors holds
    // no implementation.
    let tool: Arc<dyn Tool> = Arc::new(FixtureTool);
    let descriptor = tool.descriptor();
    assert_eq!(descriptor.id, inspect_id());
    assert_eq!(descriptor.wire_name, "inspect_wire");
    assert_eq!(descriptor.description, "Inspect a fixture.");
    assert_eq!(descriptor.parameters_schema["required"], json!(["path"]));
    assert!(!descriptor.structured_output);
    assert!(descriptor.conflicts.is_empty());
}

#[test]
fn catalog_lookup_uses_stable_identity_not_wire_name() {
    let tool: Arc<dyn Tool> = Arc::new(FixtureTool);
    let catalog =
        ToolCatalog::new(&describe_all(std::slice::from_ref(&tool))).expect("unique catalog");

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
    let catalog =
        ToolCatalog::new(&describe_all(&tools)).expect("distinct identities build a catalog");

    assert_eq!(
        catalog
            .tools()
            .iter()
            .map(|tool| tool.wire_name.as_str())
            .collect::<Vec<_>>(),
        ["first_inspect", "summarize"]
    );
    assert_eq!(catalog.tools().len(), 2);
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
    let error = ToolCatalog::new(&describe_all(&tools))
        .expect_err("a repeated tool identity must be rejected at catalog construction");
    assert_eq!(error.kind(), ToolCatalogErrorKind::DuplicateId);
    assert_eq!(
        error.duplicate_id(),
        Some(&inspect_id()),
        "the error must name the duplicated identity"
    );
}

#[tokio::test]
async fn dynamic_dispatch_reaches_the_implementation() {
    let tool: Arc<dyn Tool> = Arc::new(FixtureTool);
    let output = tool
        .call(json!({}))
        .await
        .expect("the fixture call succeeds");
    assert_eq!(output.text(), "");
}
