//! Regression coverage for the `promptforge_api_runtime::tools` compatibility
//! re-exports: the contract vocabulary sits in `promptforge-api-types`'s
//! `tools` module, and these tests pin that the re-exported path resolves
//! to the same types rather than a lookalike.

use serde_json::json;

use promptforge_api_types::tools::ToolDescriptor;

use crate::tools::ToolCatalog;

/// The fixture descriptor, built through the defining crate's path on
/// purpose: if the re-export ever stopped being the same type, the catalog
/// construction below would fail to compile.
fn reexport_descriptor() -> ToolDescriptor {
    ToolDescriptor::new(
        promptforge_api_types::tools::ToolId::parse("fixtures/tools/reexport")
            .expect("fixture id is valid"),
        "reexport_wire",
        "Exercise the re-exported contract path.",
        json!({"type": "object"}),
    )
}

#[test]
fn reexported_identity_looks_up_in_reexported_catalog() {
    let catalog = ToolCatalog::new(&[reexport_descriptor()]).expect("unique catalog");

    let id = crate::tools::ToolId::parse("fixtures/tools/reexport").expect("valid id");
    let found = catalog
        .get(&id)
        .expect("the stable identity should resolve");
    assert_eq!(found.wire_name, "reexport_wire");
    assert!(
        catalog
            .get(&crate::tools::ToolId::parse("fixtures/tools/reexport_wire").expect("valid id"))
            .is_none(),
        "the transport name must not become identity through the re-export either"
    );
}

#[test]
fn reexported_types_are_the_contract_types() {
    // A function written against the defining crate's types accepts values
    // produced through the re-exported path only when both names denote the
    // same type.
    fn takes_contract_id(id: &promptforge_api_types::tools::ToolId) -> &str {
        id.name()
    }
    fn takes_contract_catalog(catalog: &promptforge_api_types::tools::ToolCatalog) -> usize {
        catalog.tools().len()
    }

    let id = crate::tools::ToolId::parse("fixtures/tools/reexport").expect("valid id");
    assert_eq!(takes_contract_id(&id), "reexport");

    let catalog = ToolCatalog::new(&[reexport_descriptor()]).expect("unique catalog");
    assert_eq!(takes_contract_catalog(&catalog), 1);
}

#[test]
fn reexported_output_reports_the_contract_trust() {
    let output = crate::tools::ToolOutput::trusted("reexport-ok");
    assert_eq!(output.text(), "reexport-ok");
    assert_eq!(output.trust(), crate::tools::OutputTrust::Trusted);
    let error = crate::tools::ToolError::message("refused")
        .with_kind(crate::tools::ToolErrorKind::Cancelled);
    assert!(error.is_cancelled());
}
