//! Tests for the tool binding built from a catalog descriptor: the
//! descriptor's data is copied verbatim and its structured-output flag
//! selects the binding's output kind.

use promptforge_types::capabilities::CapabilityId;
use promptforge_types::tools::{ToolDescriptor, ToolId};
use serde_json::json;

use super::{ToolBinding, ToolOutputKind};

/// A descriptor for `tests/tools/fetch` with one declared conflict.
fn descriptor(structured: bool) -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse("tests/tools/fetch").expect("the id is valid"),
        "fetch",
        "Fetch a page",
        json!({"type": "object", "properties": {"url": {"type": "string"}}}),
    )
    .structured(structured)
    .with_conflicts(vec![
        CapabilityId::parse("tests/other").expect("the id is valid"),
    ])
}

#[test]
fn a_structured_descriptor_binds_with_structured_output() {
    let descriptor = descriptor(true);
    let binding = ToolBinding::from_descriptor("fetch_alias", &descriptor);
    assert_eq!(binding.output_kind, ToolOutputKind::Structured);
    assert_eq!(binding.alias(), "fetch_alias");
    assert_eq!(binding.id(), &descriptor.id);
    assert_eq!(binding.description(), "Fetch a page");
    assert_eq!(binding.schema(), &descriptor.parameters_schema);
    assert_eq!(binding.conflicts, descriptor.conflicts);
    assert!(binding.model_description().is_none());
}

#[test]
fn a_plain_descriptor_binds_with_plain_output() {
    let binding = ToolBinding::from_descriptor("fetch_alias", &descriptor(false));
    assert_eq!(binding.output_kind, ToolOutputKind::Plain);
}

#[test]
fn the_test_seam_overrides_only_the_description() {
    let descriptor = descriptor(true);
    let binding = ToolBinding::for_test("fetch_alias", "test double", &descriptor);
    assert_eq!(binding.description(), "test double");
    assert_eq!(
        binding.output_kind,
        ToolOutputKind::Structured,
        "the output kind still follows the descriptor"
    );
}
