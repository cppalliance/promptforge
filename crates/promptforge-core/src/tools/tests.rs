//! Regression coverage for the `promptforge_core::tools` compatibility
//! re-exports: the contract vocabulary moved to `promptforge-tools`, and these
//! tests pin that the re-exported path is the same trait and types, not a
//! lookalike.

use std::sync::Arc;

use serde_json::{Value, json};

// The fixture implements the trait through the defining crate's path on
// purpose: if the re-export ever stopped being the same trait, the `Arc<dyn
// crate::tools::Tool>` coercions below would fail to compile.
use promptforge_tools::{Tool as ContractTool, ToolError, ToolId, ToolOutput};

use crate::tools::{Tool, ToolCatalog};

struct ReexportFixture;

#[async_trait::async_trait]
impl ContractTool for ReexportFixture {
    fn id(&self) -> ToolId {
        ToolId::new("fixtures", "reexport").expect("fixture id is valid")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn wire_name(&self) -> &str {
        "reexport_wire"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "Exercise the re-exported contract path."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted("reexport-ok"))
    }
}

#[test]
fn reexported_identity_looks_up_in_reexported_catalog() {
    let tool: Arc<dyn Tool> = Arc::new(ReexportFixture);
    let catalog = ToolCatalog::new(std::slice::from_ref(&tool)).expect("unique catalog");

    let id = crate::tools::ToolId::new("fixtures", "reexport").expect("valid id");
    let found = catalog
        .get(&id)
        .expect("the stable identity should resolve");
    assert_eq!(found.wire_name(), "reexport_wire");
    assert!(
        catalog
            .get(&crate::tools::ToolId::new("fixtures", "reexport_wire").expect("valid id"))
            .is_none(),
        "the transport name must not become identity through the re-export either"
    );
}

#[test]
fn reexported_types_are_the_contract_types() {
    // A function written against the defining crate's types accepts values
    // produced through the re-exported path only when both names denote the
    // same type.
    fn takes_contract_id(id: &promptforge_tools::ToolId) -> &str {
        id.name()
    }
    fn takes_contract_catalog(catalog: &promptforge_tools::ToolCatalog) -> usize {
        catalog.tools().len()
    }

    let id = crate::tools::ToolId::new("fixtures", "reexport").expect("valid id");
    assert_eq!(takes_contract_id(&id), "reexport");

    let tool: Arc<dyn Tool> = Arc::new(ReexportFixture);
    let catalog = ToolCatalog::new(std::slice::from_ref(&tool)).expect("unique catalog");
    assert_eq!(takes_contract_catalog(&catalog), 1);
}

#[tokio::test]
async fn dynamic_dispatch_works_through_the_reexported_path() {
    let tool: Arc<dyn Tool> = Arc::new(ReexportFixture);
    let output = tool
        .call(json!({}))
        .await
        .expect("the fixture call succeeds");
    assert_eq!(output.text(), "reexport-ok");
    assert_eq!(output.trust(), crate::tools::OutputTrust::Trusted);
}

#[test]
fn reexported_web_search_is_the_provider_type() {
    // A function written against the provider crate's type accepts a value
    // named through the re-exported path only when both names denote the same
    // type: if the re-export ever became a lookalike, this would not compile.
    fn takes_provider(
        tool: &promptforge_web_search::WebSearch,
    ) -> &promptforge_web_search::WebSearch {
        tool
    }

    let tool =
        crate::tools::WebSearch::new("http://localhost", "tok").expect("valid configuration");
    let _ = takes_provider(&tool);
}
